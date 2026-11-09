//! Single-instance guard via an exclusive file lock (AC-08).
//!
//! - [`acquire`] takes an exclusive non-blocking `flock` on
//!   `$XDG_RUNTIME_DIR/standup/standup.lock` (fallback: `~/.cache/standup/standup.lock`).
//!   The lock file contains the holder's PID. The returned [`InstanceGuard`]
//!   holds the lock for the app lifetime; dropping it releases the lock.
//! - A second [`acquire`] fails with a clear error; `main.rs` prints it and
//!   exits `0` (no second timer/overlay is ever created).
//! - [`quit_running_instance`] (`standup --quit`) reads the PID, verifies via
//!   `/proc` that it is a live `standup` process owned by our UID, then sends
//!   `SIGTERM`, waiting up to ~2 s for the process to exit. A stale lock
//!   (dead PID, PID reuse, or foreign process) is removed and reported as
//!   "no running instance" instead of signalling the wrong process.
//! - The fallback directory is created with mode `0700`, verified to be owned
//!   by our UID, and the lock file is opened with `O_NOFOLLOW` so a
//!   pre-planted symlink can never make us truncate an arbitrary target.
//!   The `$XDG_RUNTIME_DIR` primary path is UID-isolated by the OS (systemd
//!   creates it `0700` and owned by the user).
//! - No D-Bus is involved.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;

/// Held for the whole app lifetime. Unlocks the instance lock on drop.
#[derive(Debug)]
pub struct InstanceGuard {
    file: File,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Pure path selection (takes the env value as a parameter for testability).
fn lock_path_for(xdg_runtime: Option<&str>) -> PathBuf {
    if let Some(dir) = xdg_runtime.filter(|s| !s.is_empty()) {
        return PathBuf::from(dir).join("standup/standup.lock");
    }
    fallback_lock_path()
}

/// UID-isolated fallback: `~/.cache/standup/standup.lock`, never the shared
/// `/tmp/standup/standup.lock` (which would be one lock for all users and
/// would persist across reboots, widening the stale-PID window).
fn fallback_lock_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".cache")
        })
        .join("standup/standup.lock")
}

fn lock_path() -> PathBuf {
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok();
    lock_path_for(xdg.as_deref())
}

/// Create `dir` if needed, force mode `0700`, and verify it is owned by our
/// UID and not group/other-accessible. Rejects symlink components so an
/// attacker cannot redirect the lock into a directory they control.
fn ensure_secure_dir(dir: &Path) -> anyhow::Result<()> {
    if std::fs::symlink_metadata(dir)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        anyhow::bail!(
            "refusing to use symlinked lock directory: {}",
            dir.display()
        );
    }
    std::fs::create_dir_all(dir)?;
    let mut perms = std::fs::metadata(dir)?.permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(dir, perms)?;
    let meta = std::fs::metadata(dir)?;
    // SAFETY: `getuid` is side-effect free.
    let euid = unsafe { libc::getuid() };
    if meta.uid() != euid {
        anyhow::bail!(
            "insecure lock directory (not owned by you): {}",
            dir.display()
        );
    }
    if meta.mode() & 0o077 != 0 {
        anyhow::bail!("insecure lock directory permissions: {}", dir.display());
    }
    Ok(())
}

/// Open the lock file without following symlinks (`O_NOFOLLOW`) and verify
/// the opened file is owned by our UID (defeats hardlink-swap tricks).
fn open_lock_file(path: &Path) -> anyhow::Result<File> {
    if std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        anyhow::bail!("refusing to open symlinked lock file: {}", path.display());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                anyhow::anyhow!("refusing to open symlinked lock file: {}", path.display())
            } else {
                anyhow::anyhow!("open lock file {}: {e}", path.display())
            }
        })?;
    // SAFETY: `getuid` is side-effect free.
    let euid = unsafe { libc::getuid() };
    if file.metadata()?.uid() != euid {
        anyhow::bail!("insecure lock file (not owned by you): {}", path.display());
    }
    Ok(file)
}

/// Verify that `pid` is a live `standup` process owned by our UID.
///
/// Checks `/proc/<pid>/exe` (falls back to `/proc/<pid>/cmdline`) for a
/// `standup` binary and requires the process UID to match ours. Returns
/// `false` for dead PIDs, PIDs owned by other users, and PID-reuse victims
/// running unrelated programs.
fn pid_belongs_to_standup(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: `getuid` is side-effect free.
    let euid = unsafe { libc::getuid() };
    let owned = std::fs::metadata(format!("/proc/{pid}"))
        .map(|m| m.uid() == euid)
        .unwrap_or(false);
    if !owned {
        return false;
    }
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        if exe.to_string_lossy().contains("standup") {
            return true;
        }
    }
    // `exe` may be unreadable (or the process exec'd via a wrapper):
    // fall back to argv[0] from cmdline (NUL-separated).
    if let Ok(cmd) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        if let Some(argv0) = cmd.split(|&b| b == 0).next() {
            if String::from_utf8_lossy(argv0).contains("standup") {
                return true;
            }
        }
    }
    false
}

/// Acquire the single-instance lock. Fails if another instance holds it.
pub fn acquire() -> anyhow::Result<InstanceGuard> {
    let path = lock_path();
    let is_fallback = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .is_none();
    if let Some(parent) = path.parent() {
        if is_fallback {
            ensure_secure_dir(parent)?;
        } else {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = open_lock_file(&path)?;
    file.try_lock_exclusive().map_err(|_| {
        anyhow::anyhow!(
            "another standup instance is already running (lock: {})",
            path.display()
        )
    })?;
    // We own the lock: record our PID for `--quit`.
    file.set_len(0)?;
    file.rewind()?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()?;
    Ok(InstanceGuard { file })
}

/// Stop the running instance: read its PID from the lock file, verify it is
/// really our `standup` instance, send `SIGTERM`, wait up to ~2 s for it to
/// exit. A stale or foreign PID is never signalled: the lock is removed and
/// "no running instance" is reported instead.
pub fn quit_running_instance() -> anyhow::Result<()> {
    let path = lock_path();
    let mut content = String::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .and_then(|mut f| f.read_to_string(&mut content))
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                anyhow::anyhow!("refusing to read symlinked lock file at {}", path.display())
            } else {
                anyhow::anyhow!(
                    "no running standup instance (no lock file at {})",
                    path.display()
                )
            }
        })?;
    let pid: i32 = content.trim().parse().map_err(|_| {
        anyhow::anyhow!(
            "no running standup instance (unreadable lock file at {})",
            path.display()
        )
    })?;

    let stale = |path: &Path, pid: i32| {
        // Best-effort cleanup so a stale lock can't linger (e.g. across
        // reboots on non-tmpfs fallbacks) and widen the PID-reuse window.
        let _ = std::fs::remove_file(path);
        anyhow::anyhow!("no running standup instance (stale lock for pid {pid})")
    };

    // SAFETY: `kill` with a PID and signal 0 only probes existence.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    if !alive {
        return Err(stale(&path, pid));
    }
    // Never signal a PID that isn't our standup instance (stale lock +
    // PID reuse, or a lock file planted by someone else).
    if !pid_belongs_to_standup(pid) {
        return Err(stale(&path, pid));
    }
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        anyhow::bail!("could not signal instance (pid {pid})");
    }
    for _ in 0..20 {
        if unsafe { libc::kill(pid, 0) } != 0 {
            println!("standup: stopped running instance (pid {pid})");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("instance (pid {pid}) did not exit within 2s after SIGTERM");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_path_ends_with_standup_lock() {
        assert!(lock_path().ends_with("standup/standup.lock"));
    }

    #[test]
    fn xdg_runtime_dir_is_preferred() {
        assert_eq!(
            lock_path_for(Some("/run/user/1000")),
            PathBuf::from("/run/user/1000/standup/standup.lock")
        );
    }

    #[test]
    fn fallback_is_uid_isolated_not_shared_tmp() {
        // Regression test: the fallback must never be the old shared
        // `/tmp/standup/standup.lock` (symlink-truncate + cross-user sharing).
        let old_insecure = std::env::temp_dir().join("standup/standup.lock");
        let fallback = lock_path_for(None);
        assert_ne!(fallback, old_insecure);
        assert!(fallback.ends_with("standup/standup.lock"));
    }

    #[test]
    fn current_process_is_recognized_as_standup() {
        // The test harness binary is named `standup-<hash>`, so the `exe`
        // check matches, and the UID check matches our own process.
        assert!(pid_belongs_to_standup(std::process::id() as i32));
    }

    #[test]
    fn bogus_pids_are_rejected_as_stale() {
        // Dead PIDs, PID 1 (not our standup), and non-PIDs must never be
        // signalled by `--quit`; they are treated as a stale lock.
        assert!(!pid_belongs_to_standup(0));
        assert!(!pid_belongs_to_standup(-1));
        assert!(!pid_belongs_to_standup(-999));
        assert!(!pid_belongs_to_standup(i32::MAX));
        assert!(!pid_belongs_to_standup(1));
    }
}
