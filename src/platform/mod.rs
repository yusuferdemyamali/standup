//! `platform`: Linux-only integrations (D-Bus session/lock, notifications,
//! single-instance lock, autostart). Windows gets a separate adapter later;
//! `domain/` must never import from here.

pub mod autostart;
pub mod notifications;
pub mod session;
pub mod single_instance;
