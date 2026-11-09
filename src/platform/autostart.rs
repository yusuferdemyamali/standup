//! Session autostart install/uninstall (AC-09).
//!
//! Two mutually exclusive methods; exactly one may be active at a time:
//!
//! - `"xdg"`: writes `~/.config/autostart/standup.desktop` (content mirrors
//!   `assets/standup.desktop`), launched by any XDG-autostart-capable
//!   session. No `sudo`, no system-wide changes.
//! - `"hyprland"`: writes `~/.config/hypr/standup-autostart.conf` containing
//!   `exec-once = standup`, and prints the one line the user must add to
//!   `hyprland.conf` themselves. This module NEVER edits `hyprland.conf`.
//!
//! Enabling one method while the other is active is refused with an error
//! telling the user to `--uninstall-autostart` first.

use std::path::PathBuf;

const XDG_FILE_NAME: &str = "standup.desktop";
const HYPR_FILE_NAME: &str = "standup-autostart.conf";
const HYPR_LINE: &str = "exec-once = standup";

/// The installed `.desktop` entry. Single source of truth: ships as
/// `assets/standup.desktop`.
const DESKTOP_ENTRY: &str = include_str!("../../assets/standup.desktop");

fn xdg_autostart_file() -> PathBuf {
    config_home().join("autostart").join(XDG_FILE_NAME)
}

fn hypr_autostart_file() -> PathBuf {
    config_home().join("hypr").join(HYPR_FILE_NAME)
}

fn config_home() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg);
        }
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"))
}

/// Enable autostart via `method`: `"xdg"` or `"hyprland"`.
pub fn install(method: &str) -> anyhow::Result<()> {
    match method {
        "xdg" => install_xdg(),
        "hyprland" => install_hyprland(),
        other => {
            anyhow::bail!("unknown autostart method `{other}` (expected \"xdg\" or \"hyprland\")")
        }
    }
}

/// Disable autostart: removes both method files if present.
pub fn uninstall() -> anyhow::Result<()> {
    let mut removed = false;
    for path in [xdg_autostart_file(), hypr_autostart_file()] {
        match std::fs::remove_file(&path) {
            Ok(()) => {
                println!("standup: removed {}", path.display());
                removed = true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => anyhow::bail!("could not remove {} ({e})", path.display()),
        }
    }
    if !removed {
        println!("standup: autostart already disabled (nothing to remove)");
    }
    Ok(())
}

fn install_xdg() -> anyhow::Result<()> {
    let hypr = hypr_autostart_file();
    if hypr.is_file() {
        anyhow::bail!(
            "Hyprland autostart is already enabled ({}); run `standup --uninstall-autostart` first",
            hypr.display()
        );
    }
    let path = xdg_autostart_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DESKTOP_ENTRY)?;
    println!("standup: enabled XDG autostart: {}", path.display());
    Ok(())
}

fn install_hyprland() -> anyhow::Result<()> {
    let xdg = xdg_autostart_file();
    if xdg.is_file() {
        anyhow::bail!(
            "XDG autostart is already enabled ({}); run `standup --uninstall-autostart` first",
            xdg.display()
        );
    }
    let path = hypr_autostart_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let already: Vec<String> = std::fs::read_to_string(&path)
        .map(|c| c.lines().map(str::to_string).collect())
        .unwrap_or_default();
    if !already.iter().any(|l| l.trim() == HYPR_LINE) {
        let mut content: String = already.join("\n");
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(HYPR_LINE);
        content.push('\n');
        std::fs::write(&path, content)?;
        println!("standup: enabled Hyprland autostart: {}", path.display());
    } else {
        println!(
            "standup: Hyprland autostart already enabled: {}",
            path.display()
        );
    }
    println!("standup: add this line to hyprland.conf (not done automatically):");
    println!("source = ~/.config/hypr/{HYPR_FILE_NAME}");
    Ok(())
}
