//! StandUp — Linux MVP entry point.
//!
//!굵 Architecture: `domain/` (platform-agnostic timer state machine) is driven
//! by `platform/` (session/lock/suspend signals, notifications, single-instance)
//! and rendered by `ui/` (GTK4 + gtk4-layer-shell overlays, one surface per monitor).

mod config;
mod domain;
mod platform;
mod ui;

use std::time::Duration;

use anyhow::Context;
use gio::prelude::ApplicationExtManual;
use log::{error, info};

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--quit") {
        return platform::single_instance::quit_running_instance();
    }
    if args.iter().any(|a| a == "--install-autostart") {
        let method = args
            .windows(2)
            .find(|w| w[0] == "--install-autostart")
            .and_then(|w| w.get(1))
            .map(|s| s.as_str())
            .unwrap_or("xdg");
        return platform::autostart::install(method);
    }
    if args.iter().any(|a| a == "--uninstall-autostart") {
        return platform::autostart::uninstall();
    }

    // Shortened durations for dev/test: --work-secs / --snooze-secs / --break-secs.
    // Never used in normal launches; only for manual verification without
    // waiting full 25-minute cycles.
    let mut overrides = config::DurationOverrides::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--work-secs" => {
                overrides.work = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .map(Duration::from_secs);
                i += 1;
            }
            "--snooze-secs" => {
                overrides.snooze = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .map(Duration::from_secs);
                i += 1;
            }
            "--break-secs" => {
                overrides.brk = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .map(Duration::from_secs);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    let _guard = match platform::single_instance::acquire() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("standup: another instance is already running ({e}). Exiting.");
            std::process::exit(0);
        }
    };

    let cfg = config::load_with_overrides(overrides).context("load config")?;
    info!(
        "standup starting: work={}s snooze={}s break={}s",
        cfg.work_secs, cfg.snooze_secs, cfg.break_secs
    );

    let app = ui::build_app(cfg);
    // No normal window: the app runs headless until the timer fires.
    // gtk Application without any window shows nothing; overlays are
    // layer-shell surfaces created on demand by the timer.
    let argv: Vec<String> = std::env::args().take(1).collect();
    let exit_code = app.run_with_args(&argv);
    let code: i32 = exit_code.into();
    if code != 0 {
        error!("gtk main loop exited with failure");
    }
    Ok(())
}
