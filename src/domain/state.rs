//! Domain states and events (pure data, no logic).

use std::time::{Duration, Instant};

/// Engine state. `AWAY` is *not* a state: the platform pauses the engine
/// while locked/suspended and reports `Event::AwayEnded` on return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Work {
        deadline: Instant,
    },
    Snooze {
        reminder_at: Instant,
        work_remaining: Duration,
    },
    Reminder {
        since: Instant,
        work_remaining: Duration,
    },
    Break {
        ends_at: Instant,
    },
}

/// Inputs the engine reacts to. Time-driven variants are exercised via
/// `poll()` in production and directly in tests.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    WorkExpired,
    SnoozeAction,
    StartBreakAction,
    SnoozeExpired,
    BreakExpired,
    AwayEnded { away: Duration },
    Tick,
}
