//! Desktop notifications (spec §3.2: best-effort, never fatal).
//!
//! - [`notify_break_done`]: the "break over → new work cycle" ping sent when
//!   `BREAK` expires. The fullscreen overlay is NOT re-opened.
//! - [`notify`]: generic summary/body notification for UI wiring.
//!
//! A missing notification daemon (or any other failure) is logged with
//! `log::warn` and returns `Ok`: the timer keeps running, the missing
//! notification is never a critical error.

/// Send a desktop notification. Always returns `Ok`; failures are logged.
pub fn notify(summary: &str, body: &str) -> anyhow::Result<()> {
    match notify_rust::Notification::new()
        .appname("StandUp")
        .summary(summary)
        .body(body)
        .show()
    {
        Ok(_) => Ok(()),
        Err(e) => {
            log::warn!("notification unavailable ({e}); continuing without it");
            Ok(())
        }
    }
}

/// "Mola tamamlandı. Yeni çalışma döngüsü başladı." ping (spec §3.2).
pub fn notify_break_done() -> anyhow::Result<()> {
    notify(
        "Mola tamamlandı",
        "Mola tamamlandı. Yeni çalışma döngüsü başladı.",
    )
}
