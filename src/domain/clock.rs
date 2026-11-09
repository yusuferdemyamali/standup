//! Monotonic + wall clocks. Engine uses only `Instant` for deadlines.

use std::sync::Mutex;
use std::time::{Instant, SystemTime};

/// Clock abstraction: monotonic for deadlines, wall for away measurement.
/// Public platform contract (used by tests and future platform integrations).
#[allow(dead_code)]
pub trait Clock {
    fn now_monotonic(&self) -> Instant;
    fn now_wall(&self) -> SystemTime;
}

/// Production clock: straight through to std.
#[allow(dead_code)]
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_monotonic(&self) -> Instant {
        Instant::now()
    }

    fn now_wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Controllable clock for tests (kept compiled for future UI/platform tests).
#[allow(dead_code)]
#[derive(Debug)]
pub struct FakeClock {
    mono: Mutex<Instant>,
    wall: Mutex<SystemTime>,
}

#[allow(dead_code)]
impl FakeClock {
    pub fn new(start_mono: Instant) -> Self {
        Self {
            mono: Mutex::new(start_mono),
            wall: Mutex::new(SystemTime::now()),
        }
    }

    /// Advance both monotonic and wall clocks.
    pub fn advance(&self, d: std::time::Duration) {
        let mut mono = self.mono.lock().unwrap();
        *mono = mono.checked_add(d).expect("FakeClock monotonic overflow");
        let mut wall = self.wall.lock().unwrap();
        *wall = wall.checked_add(d).expect("FakeClock wall overflow");
    }

    /// Advance only the wall clock (simulates a wall-clock jump;
    /// monotonic deadlines must be unaffected).
    pub fn advance_wall_only(&self, d: std::time::Duration) {
        let mut wall = self.wall.lock().unwrap();
        *wall = wall.checked_add(d).expect("FakeClock wall overflow");
    }

    /// Advance only the monotonic clock.
    pub fn advance_mono_only(&self, d: std::time::Duration) {
        let mut mono = self.mono.lock().unwrap();
        *mono = mono.checked_add(d).expect("FakeClock monotonic overflow");
    }
}

impl Clock for FakeClock {
    fn now_monotonic(&self) -> Instant {
        *self.mono.lock().unwrap()
    }

    fn now_wall(&self) -> SystemTime {
        *self.wall.lock().unwrap()
    }
}
