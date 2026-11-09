//! Session/away tracking: screen lock + suspend/resume via D-Bus
//! (spec §4.1, AC-06).
//!
//! API (owned by this module; designed for UI wiring later):
//!
//! - [`AwayTracker::new()`]: spawns a background monitor thread and returns
//!   immediately, so startup never blocks on D-Bus.
//! - [`AwayTracker::is_away`]: cheap poll for the UI tick loop. While it
//!   returns `true`, the UI must hide any reminder overlay (never draw over
//!   the lock screen) and the timer must not count work time.
//! - [`AwayTracker::away_elapsed_wall`]: suspend-aware monotonic time since
//!   away began (`None` while present; historical name kept for API
//!   stability — BOOTTIME-based, not wall). The platform consumer compares
//!   it against the 5-minute threshold from the spec; this module never
//!   decides breaks.
//! - [`AwayTracker::subscribe_away_changed`]: callbacks fire on the monitor
//!   thread — forward to the GTK thread (e.g. via `glib::idle_add`) before
//!   touching widgets.
//!
//! Sources, watched with `zbus`:
//! - session bus `org.freedesktop.ScreenSaver` / `ActiveChanged(bool)`,
//! - system bus `org.freedesktop.login1` / `Manager` `PrepareForSleep(bool)`
//!   (`true` = about to suspend, `false` = just resumed).
//!
//! Graceful degradation (spec §5): any unavailable bus, proxy, or signal
//! only logs a warning and disables that source. Unsupported signals never
//! fake a break and lock/unlock state is never guessed.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::StreamExt;

type AwayCallback = Box<dyn Fn(bool) + Send + 'static>;

#[derive(Debug, Default)]
struct AwayState {
    locked: bool,
    asleep: bool,
    /// Wall time of the current away period's start (kept for cross-checking
    /// wall jumps; NOT the source of truth for elapsed).
    away_since_wall: Option<std::time::SystemTime>,
    /// CLOCK_BOOTTIME timestamp of the current away period's start.
    /// BOOTTIME is monotonic across wall jumps and keeps ticking during
    /// suspend, so it covers both AC-06 (lock) and AC-07 (suspend) gaps.
    /// `None` when the start stamp could not be read (fallback: wall clock).
    away_since_boot: Option<Duration>,
}

impl AwayState {
    fn away(&self) -> bool {
        self.locked || self.asleep
    }
}

/// Tracks screen-lock / suspend state. `Clone` shares the same state.
#[derive(Clone, Default)]
pub struct AwayTracker {
    state: Arc<Mutex<AwayState>>,
    callbacks: Arc<Mutex<Vec<AwayCallback>>>,
}

impl std::fmt::Debug for AwayTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwayTracker")
            .field("away", &self.is_away())
            .finish_non_exhaustive()
    }
}

impl AwayTracker {
    /// Create the tracker and start the background D-Bus monitor.
    /// Never blocks; never fails — without D-Bus it just stays "present".
    pub fn new() -> Self {
        let tracker = Self {
            state: Arc::new(Mutex::new(AwayState::default())),
            callbacks: Arc::new(Mutex::new(Vec::new())),
        };
        tracker.spawn_monitor();
        tracker
    }

    /// `true` while the screen is locked or the system is (about to be)
    /// suspended.
    pub fn is_away(&self) -> bool {
        self.state.lock().map(|s| s.away()).unwrap_or(false)
    }

    /// Away time since away began, or `None` while present.
    ///
    /// Despite the historical name (kept for API stability), this returns a
    /// suspend-aware monotonic duration based on `CLOCK_BOOTTIME`: immune to
    /// wall-clock jumps and including suspend time (spec: BREAK is real
    /// time). The wall clock is only a cross-check — when wall and BOOTTIME
    /// deltas disagree wildly (wall jump), BOOTTIME is trusted and a warning
    /// is logged. If `CLOCK_BOOTTIME` cannot be read, it falls back to the
    /// wall-clock delta with a warning.
    pub fn away_elapsed_wall(&self) -> Option<Duration> {
        let guard = self.state.lock().ok()?;
        if !guard.away() {
            return None;
        }
        let wall_elapsed = guard.away_since_wall.map(|since| {
            std::time::SystemTime::now()
                .duration_since(since)
                .unwrap_or(Duration::ZERO)
        });
        let boot_elapsed = match (boot_now(), guard.away_since_boot) {
            (Some(now), Some(since)) => now.checked_sub(since).or(Some(Duration::ZERO)),
            _ => None,
        };
        if boot_elapsed.is_none() {
            log::warn!("session: CLOCK_BOOTTIME unavailable; using wall-clock away delta");
        }
        if let (Some(b), Some(w)) = (boot_elapsed, wall_elapsed) {
            if away_deltas_disagree(b, w) {
                log::warn!(
                    "session: wall/boot away deltas disagree (wall={w:?}, boot={b:?}); trusting BOOTTIME"
                );
            }
        }
        select_away_elapsed(boot_elapsed, wall_elapsed)
    }

    /// Register a callback invoked on every present↔away transition.
    /// Runs on the monitor thread; must be thread-safe and return quickly.
    /// (Push API; the UI currently polls instead.)
    #[allow(dead_code)]
    pub fn subscribe_away_changed(&self, cb: impl Fn(bool) + Send + 'static) {
        if let Ok(mut cbs) = self.callbacks.lock() {
            cbs.push(Box::new(cb));
        }
    }

    fn set_locked(&self, locked: bool) {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.locked == locked {
            return;
        }
        let was_away = guard.away();
        guard.locked = locked;
        Self::after_change(&self.callbacks, guard, was_away, "screen-lock");
    }

    fn set_asleep(&self, asleep: bool) {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.asleep == asleep {
            return;
        }
        let was_away = guard.away();
        guard.asleep = asleep;
        Self::after_change(&self.callbacks, guard, was_away, "suspend");
    }

    /// Update the away stamps on combined transitions and fire callbacks.
    /// Takes the state lock by value so it is released before callbacks run
    /// (callbacks may call back into [`AwayTracker::is_away`]).
    fn after_change(
        callbacks: &Arc<Mutex<Vec<AwayCallback>>>,
        mut guard: MutexGuard<'_, AwayState>,
        was_away: bool,
        source: &str,
    ) {
        let now_away = guard.away();
        if now_away == was_away {
            return;
        }
        if now_away {
            guard.away_since_wall = Some(std::time::SystemTime::now());
            guard.away_since_boot = match boot_now() {
                Some(t) => Some(t),
                None => {
                    log::warn!(
                        "session: CLOCK_BOOTTIME read failed at away-start; wall-clock fallback"
                    );
                    None
                }
            };
        } else {
            guard.away_since_wall = None;
            guard.away_since_boot = None;
        }
        log::info!("session: away={now_away} (source: {source})");
        drop(guard);
        if let Ok(cbs) = callbacks.lock() {
            for cb in cbs.iter() {
                cb(now_away);
            }
        }
    }

    fn spawn_monitor(&self) {
        let this = self.clone();
        std::thread::Builder::new()
            .name("standup-session".into())
            .spawn(move || this.monitor_blocking())
            .map(|_| ())
            .unwrap_or_else(|e| {
                log::warn!(
                    "session: cannot spawn D-Bus monitor thread ({e}); away detection disabled"
                );
            });
    }

    fn monitor_blocking(&self) {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                log::warn!("session: cannot start async runtime ({e}); away detection disabled");
                return;
            }
        };
        rt.block_on(self.monitor_async());
    }

    async fn monitor_async(&self) {
        futures_util::future::join(self.watch_screensaver(), self.watch_login1()).await;
    }

    async fn watch_screensaver(&self) {
        let conn = match zbus::Connection::session().await {
            Ok(c) => c,
            Err(e) => {
                log::warn!("session: session bus unavailable ({e}); screen-lock tracking disabled");
                return;
            }
        };
        let proxy = match zbus::Proxy::new(
            &conn,
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                log::warn!(
                    "session: ScreenSaver proxy unavailable ({e}); screen-lock tracking disabled"
                );
                return;
            }
        };
        let mut stream = match proxy.receive_signal("ActiveChanged").await {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "session: cannot watch ActiveChanged ({e}); screen-lock tracking disabled"
                );
                return;
            }
        };
        log::debug!("session: watching ScreenSaver.ActiveChanged");
        while let Some(msg) = stream.next().await {
            match msg.body().deserialize::<bool>() {
                Ok(active) => self.set_locked(active),
                Err(e) => log::warn!("session: bad ActiveChanged body ({e})"),
            }
        }
        log::warn!("session: ScreenSaver signal stream ended; screen-lock tracking stopped");
    }

    async fn watch_login1(&self) {
        let conn = match zbus::Connection::system().await {
            Ok(c) => c,
            Err(e) => {
                log::warn!("session: system bus unavailable ({e}); suspend tracking disabled");
                return;
            }
        };
        let proxy = match zbus::Proxy::new(
            &conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                log::warn!("session: login1 proxy unavailable ({e}); suspend tracking disabled");
                return;
            }
        };
        let mut stream = match proxy.receive_signal("PrepareForSleep").await {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "session: cannot watch PrepareForSleep ({e}); suspend tracking disabled"
                );
                return;
            }
        };
        log::debug!("session: watching login1 PrepareForSleep");
        while let Some(msg) = stream.next().await {
            match msg.body().deserialize::<bool>() {
                // `true` = about to suspend, `false` = just resumed.
                Ok(start) => self.set_asleep(start),
                Err(e) => log::warn!("session: bad PrepareForSleep body ({e})"),
            }
        }
        log::warn!("session: login1 signal stream ended; suspend tracking stopped");
    }
}

/// Tolerance for wall-vs-BOOTTIME disagreement. Deltas differing by more
/// than this are treated as a wall-clock jump (forward or backward).
const AWAY_DELTA_DISAGREE_TOLERANCE: Duration = Duration::from_secs(30);

/// Current `CLOCK_BOOTTIME` time-since-boot. Unlike `Instant`
/// (`CLOCK_MONOTONIC`, frozen during suspend) and unlike `SystemTime`
/// (subject to NTP/admin jumps), BOOTTIME is monotonic across wall jumps
/// AND keeps ticking during suspend — the right basis for "real 5 minutes"
/// away (AC-06/AC-07). Returns `None` if the clock cannot be read.
fn boot_now() -> Option<Duration> {
    let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `ts` is a valid writable pointer for `clock_gettime`.
    let ret = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, ts.as_mut_ptr()) };
    if ret != 0 {
        return None;
    }
    // SAFETY: `clock_gettime` succeeded, so `ts` is initialised.
    let ts = unsafe { ts.assume_init() };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return None;
    }
    let nsec = (ts.tv_nsec as u64).min(999_999_999);
    Some(Duration::new(ts.tv_sec as u64, nsec as u32))
}

/// Pick the away elapsed to report. Prefers the BOOTTIME (suspend-aware,
/// jump-immune) delta; falls back to the wall delta only when BOOTTIME is
/// unavailable. Pure function so wall-jump immunity is unit-testable
/// (a real forward/backward wall jump cannot be simulated with `SystemTime`
/// in a unit test, but the selection logic over precomputed deltas can).
fn select_away_elapsed(
    boot_elapsed: Option<Duration>,
    wall_elapsed: Option<Duration>,
) -> Option<Duration> {
    boot_elapsed.or(wall_elapsed)
}

/// `true` when wall and BOOTTIME away deltas disagree by more than the
/// tolerance — i.e. the wall clock jumped while away.
fn away_deltas_disagree(boot_elapsed: Duration, wall_elapsed: Duration) -> bool {
    boot_elapsed.abs_diff(wall_elapsed) > AWAY_DELTA_DISAGREE_TOLERANCE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_present_and_tracks_transitions() {
        let t = AwayTracker::new();
        assert!(!t.is_away());
        assert_eq!(t.away_elapsed_wall(), None);

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        t.subscribe_away_changed(move |away| seen2.lock().unwrap().push(away));

        t.set_locked(true);
        assert!(t.is_away());
        assert!(t.away_elapsed_wall().is_some());

        // Second source joins: still one transition total.
        t.set_asleep(true);
        assert!(t.is_away());

        // One source leaves: still away, no new callback.
        t.set_locked(false);
        assert!(t.is_away());

        // All clear: back to present.
        t.set_asleep(false);
        assert!(!t.is_away());
        assert_eq!(t.away_elapsed_wall(), None);

        assert_eq!(*seen.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn select_prefers_boot_over_forward_wall_jump() {
        // Short lock (10 s by BOOTTIME) with a +1 h forward wall jump:
        // must NOT look like a long break.
        let picked = select_away_elapsed(
            Some(Duration::from_secs(10)),
            Some(Duration::from_secs(3610)),
        );
        assert_eq!(picked, Some(Duration::from_secs(10)));
        assert!(away_deltas_disagree(
            Duration::from_secs(10),
            Duration::from_secs(3610)
        ));
    }

    #[test]
    fn select_prefers_boot_over_backward_wall_jump() {
        // Long absence (10 min by BOOTTIME) hidden by a -9 min wall jump:
        // must NOT look like a short gap.
        let picked = select_away_elapsed(
            Some(Duration::from_secs(600)),
            Some(Duration::from_secs(60)),
        );
        assert_eq!(picked, Some(Duration::from_secs(600)));
        assert!(away_deltas_disagree(
            Duration::from_secs(600),
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn select_agrees_within_tolerance_and_falls_back() {
        // Normal case: deltas close (scheduling jitter only) — no jump.
        assert!(!away_deltas_disagree(
            Duration::from_secs(100),
            Duration::from_secs(105)
        ));
        assert_eq!(
            select_away_elapsed(
                Some(Duration::from_secs(100)),
                Some(Duration::from_secs(105))
            ),
            Some(Duration::from_secs(100))
        );
        // BOOTTIME unavailable: fall back to wall instead of returning None.
        assert_eq!(
            select_away_elapsed(None, Some(Duration::from_secs(42))),
            Some(Duration::from_secs(42))
        );
        assert_eq!(select_away_elapsed(None, None), None);
    }

    #[test]
    fn boot_clock_is_readable() {
        assert!(boot_now().is_some());
    }

    #[test]
    fn away_elapsed_is_boot_based_and_small_right_after_lock() {
        let t = AwayTracker::new();
        t.set_locked(true);
        // BOOTTIME-based: must be tiny immediately after away-start,
        // regardless of any wall movement.
        let elapsed = t.away_elapsed_wall().expect("away just began");
        assert!(elapsed < Duration::from_secs(30));
        t.set_locked(false);
    }
}
