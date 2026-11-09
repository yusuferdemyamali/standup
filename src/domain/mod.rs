//! `domain`: platform-agnostic work/break state machine.
//!
//! CONTRACT (do not rename without updating `platform/`, `ui/` and `main.rs`):
//!
//! ```ignore
//! pub struct Durations { pub work: Duration, pub snooze: Duration,
//!                       pub brk: Duration, pub away_threshold: Duration }
//! pub enum State { Work{ deadline: Instant }, Snooze{ reminder_at: Instant,
//!                  work_remaining: Duration }, Reminder{ since: Instant,
//!                  work_remaining: Duration }, Break{ ends_at: Instant } }
//! pub enum Event { WorkExpired, SnoozeAction, StartBreakAction,
//!                  SnoozeExpired, BreakExpired,
//!                  AwayEnded{ away: Duration }, Tick }
//! pub enum Action { ShowReminder, HideReminder, NotifyBreakDone, Nothing }
//! impl TimerEngine {
//!     pub fn new(durations: Durations, now: Instant) -> Self;
//!     pub fn state(&self) -> &State;
//!     pub fn work_remaining(&self, now: Instant) -> Duration;
//!     pub fn handle(&mut self, ev: Event, now: Instant) -> Vec<Action>;
//!     pub fn poll(&mut self, now: Instant) -> Vec<Action>;
//! }
//! ```
//!
//! Rules (from main_spec.md §4):
//! - WORK counts only active, unlocked session time (platform pauses the
//!   engine while AWAY; engine itself only sees AwayEnded durations).
//! - Snooze never resets the work counter: Reminder stores work_remaining,
//!   Snooze counts 5 min, then Reminder re-opens with the same remainder.
//! - Away >= threshold (5 min) → fresh WORK; shorter away preserves remainder.
//! - All expiry is monotonic-Instant based; wall-clock jumps must not shift
//!   deadlines (AC-07).

pub mod clock;
pub mod state;
pub mod timer;

#[allow(unused_imports)] // `State` is public contract for future UI/platform use.
pub use state::{Event, State};
pub use timer::{Action, Durations, TimerEngine};
