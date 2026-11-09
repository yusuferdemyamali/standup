//! Pure `TimerEngine`: monotonic-`Instant` deadlines only, no threads/GTK.

use std::time::{Duration, Instant};

use super::state::{Event, State};

/// Configured phase lengths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Durations {
    pub work: Duration,
    pub snooze: Duration,
    pub brk: Duration,
    pub away_threshold: Duration,
}

/// Side effects the caller (platform/UI layer) must perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ShowReminder,
    HideReminder,
    NotifyBreakDone,
    Nothing,
}

/// Pure state machine over `Instant` deadlines.
#[derive(Debug)]
pub struct TimerEngine {
    durations: Durations,
    state: State,
}

impl TimerEngine {
    pub fn new(durations: Durations, now: Instant) -> Self {
        Self {
            state: State::Work {
                deadline: now + durations.work,
            },
            durations,
        }
    }

    /// (Observability API for UI/platform; covered by unit tests.)
    #[allow(dead_code)]
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Remaining active work time. Snooze/Reminder report the stored
    /// remainder (work counter is never reset by snoozing); Break and
    /// anything already elapsed report zero.
    /// (Observability API for UI/platform; covered by unit tests.)
    #[allow(dead_code)]
    pub fn work_remaining(&self, now: Instant) -> Duration {
        match self.state {
            State::Work { deadline } => deadline.saturating_duration_since(now),
            State::Snooze { work_remaining, .. } | State::Reminder { work_remaining, .. } => {
                work_remaining
            }
            State::Break { .. } => Duration::ZERO,
        }
    }

    /// Explicit event input. Returns the actions the caller must perform.
    /// No-op transitions yield `vec![Action::Nothing]`.
    pub fn handle(&mut self, ev: Event, now: Instant) -> Vec<Action> {
        match ev {
            Event::WorkExpired => self.on_work_expired(now),
            Event::SnoozeAction => self.on_snooze_action(now),
            Event::StartBreakAction => self.on_start_break(now),
            Event::SnoozeExpired => self.on_snooze_expired(now),
            Event::BreakExpired => self.on_break_expired(now),
            Event::AwayEnded { away } => self.on_away_ended(away, now),
            Event::Tick => self.poll(now),
        }
    }

    /// Deadline check for the current state. Emits the same transition as
    /// the matching `Event`; idempotent (second call after expiry emits
    /// `Nothing`).
    pub fn poll(&mut self, now: Instant) -> Vec<Action> {
        match self.state {
            State::Work { deadline } if now >= deadline => self.on_work_expired(now),
            State::Snooze { reminder_at, .. } if now >= reminder_at => self.on_snooze_expired(now),
            State::Break { ends_at } if now >= ends_at => self.on_break_expired(now),
            // Reminder has no deadline; it waits for an explicit action.
            _ => vec![Action::Nothing],
        }
    }

    // -- transitions --------------------------------------------------------

    fn on_work_expired(&mut self, now: Instant) -> Vec<Action> {
        match self.state {
            State::Work { deadline } if now >= deadline => {
                self.state = State::Reminder {
                    since: now,
                    work_remaining: Duration::ZERO,
                };
                vec![Action::ShowReminder]
            }
            _ => vec![Action::Nothing],
        }
    }

    fn on_snooze_action(&mut self, now: Instant) -> Vec<Action> {
        match self.state {
            State::Reminder { work_remaining, .. } => {
                self.state = State::Snooze {
                    reminder_at: now + self.durations.snooze,
                    // Work already expired when the reminder opened, so the
                    // stored remainder is carried through unchanged; the work
                    // counter is never reset by snoozing.
                    work_remaining,
                };
                vec![Action::HideReminder]
            }
            _ => vec![Action::Nothing],
        }
    }

    fn on_start_break(&mut self, now: Instant) -> Vec<Action> {
        match self.state {
            State::Reminder { .. } => {
                self.state = State::Break {
                    ends_at: now + self.durations.brk,
                };
                vec![Action::HideReminder]
            }
            _ => vec![Action::Nothing],
        }
    }

    fn on_snooze_expired(&mut self, now: Instant) -> Vec<Action> {
        match self.state {
            State::Snooze {
                reminder_at,
                work_remaining,
            } if now >= reminder_at => {
                // Re-open directly: no extra work period is inserted.
                self.state = State::Reminder {
                    since: now,
                    work_remaining,
                };
                vec![Action::ShowReminder]
            }
            _ => vec![Action::Nothing],
        }
    }

    fn on_break_expired(&mut self, now: Instant) -> Vec<Action> {
        match self.state {
            State::Break { ends_at } if now >= ends_at => {
                self.state = State::Work {
                    deadline: now + self.durations.work,
                };
                vec![Action::NotifyBreakDone]
            }
            _ => vec![Action::Nothing],
        }
    }

    fn on_away_ended(&mut self, away: Duration, now: Instant) -> Vec<Action> {
        // BREAK counts real time even while locked: never shift it, just
        // finish it if `ends_at` passed while away. No reminder is drawn
        // over the lock screen (platform guarantee).
        if let State::Break { ends_at } = self.state {
            if now >= ends_at {
                self.state = State::Work {
                    deadline: now + self.durations.work,
                };
                return vec![Action::NotifyBreakDone];
            }
            return vec![Action::Nothing];
        }

        if away >= self.durations.away_threshold {
            // Long absence → fresh WORK from `now`.
            let was_reminder = matches!(self.state, State::Reminder { .. });
            self.state = State::Work {
                deadline: now + self.durations.work,
            };
            if was_reminder {
                // Overlay was hidden while locked; make sure it stays hidden.
                return vec![Action::HideReminder];
            }
            return vec![Action::Nothing];
        }

        // Short absence → preserve the remainder by pushing monotonic
        // deadlines forward by the away duration.
        match self.state {
            State::Work { deadline } => {
                let shifted = deadline.checked_add(away).unwrap_or(deadline);
                self.state = State::Work { deadline: shifted };
                vec![Action::Nothing]
            }
            State::Snooze {
                reminder_at,
                work_remaining,
            } => {
                let shifted = reminder_at.checked_add(away).unwrap_or(reminder_at);
                self.state = State::Snooze {
                    reminder_at: shifted,
                    work_remaining,
                };
                // If the snooze already lapsed (lapsed before the lock),
                // open the reminder on return.
                if now >= shifted {
                    self.state = State::Reminder {
                        since: now,
                        work_remaining,
                    };
                    return vec![Action::ShowReminder];
                }
                vec![Action::Nothing]
            }
            State::Reminder { work_remaining, .. } => {
                // Overlay was hidden while locked; re-show the same reminder.
                self.state = State::Reminder {
                    since: now,
                    work_remaining,
                };
                vec![Action::ShowReminder]
            }
            State::Break { .. } => unreachable!("handled above"),
        }
    }
}

// -- unit tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::{Clock, FakeClock};
    use std::time::Duration;

    fn short() -> Durations {
        Durations {
            work: Duration::from_secs(25),
            snooze: Duration::from_secs(5),
            brk: Duration::from_secs(5),
            away_threshold: Duration::from_secs(5),
        }
    }

    fn engine_at(clock: &FakeClock) -> TimerEngine {
        TimerEngine::new(short(), clock.now_monotonic())
    }

    #[test]
    fn work_to_reminder_after_work_duration() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        assert!(matches!(eng.state(), State::Work { .. }));

        // Just before expiry: nothing.
        clock.advance(Duration::from_secs(24));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::Nothing]);
        assert!(matches!(eng.state(), State::Work { .. }));

        // At expiry: reminder opens exactly once.
        clock.advance(Duration::from_secs(1));
        assert_eq!(
            eng.handle(Event::WorkExpired, clock.now_monotonic()),
            vec![Action::ShowReminder]
        );
        assert!(matches!(eng.state(), State::Reminder { .. }));
    }

    #[test]
    fn snooze_returns_to_reminder_with_no_work_reset() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);

        // Snooze hides the overlay.
        assert_eq!(
            eng.handle(Event::SnoozeAction, clock.now_monotonic()),
            vec![Action::HideReminder]
        );
        let (reminder_at, stored) = match *eng.state() {
            State::Snooze {
                reminder_at,
                work_remaining,
            } => (reminder_at, work_remaining),
            s => panic!("expected Snooze, got {s:?}"),
        };
        assert_eq!(stored, Duration::ZERO);

        // 1s early: still snoozing.
        clock.advance(Duration::from_secs(4));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::Nothing]);
        assert_eq!(eng.work_remaining(clock.now_monotonic()), stored);

        // Exactly at 5 min: reminder re-opens, same remainder.
        clock.advance(Duration::from_secs(1));
        assert!(clock.now_monotonic() >= reminder_at);
        assert_eq!(
            eng.handle(Event::SnoozeExpired, clock.now_monotonic()),
            vec![Action::ShowReminder]
        );
        match *eng.state() {
            State::Reminder { work_remaining, .. } => assert_eq!(work_remaining, stored),
            s => panic!("expected Reminder, got {s:?}"),
        }
    }

    #[test]
    fn repeated_snooze_works_three_times() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);

        for _ in 0..3 {
            assert_eq!(
                eng.handle(Event::SnoozeAction, clock.now_monotonic()),
                vec![Action::HideReminder]
            );
            assert!(matches!(eng.state(), State::Snooze { .. }));
            clock.advance(Duration::from_secs(5));
            assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);
            assert!(matches!(eng.state(), State::Reminder { .. }));
        }
    }

    #[test]
    fn break_to_work_with_notification() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);

        assert_eq!(
            eng.handle(Event::StartBreakAction, clock.now_monotonic()),
            vec![Action::HideReminder]
        );
        assert!(matches!(eng.state(), State::Break { .. }));

        clock.advance(Duration::from_secs(4));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::Nothing]);

        clock.advance(Duration::from_secs(1));
        assert_eq!(
            eng.handle(Event::BreakExpired, clock.now_monotonic()),
            vec![Action::NotifyBreakDone]
        );
        match *eng.state() {
            State::Work { deadline } => {
                assert_eq!(deadline, clock.now_monotonic() + short().work);
            }
            s => panic!("expected fresh Work, got {s:?}"),
        }
    }

    #[test]
    fn short_away_preserves_work_remainder() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        // 10s of work done, 15s left.
        clock.advance(Duration::from_secs(10));
        let before = eng.work_remaining(clock.now_monotonic());
        assert_eq!(before, Duration::from_secs(15));

        // 2s away (< 5s threshold): deadline pushed forward.
        // Mono keeps running while locked, so the return `now` includes
        // the away span; the shift compensates and the remainder is kept.
        clock.advance(Duration::from_secs(2));
        let now = clock.now_monotonic();
        assert_eq!(
            eng.handle(
                Event::AwayEnded {
                    away: Duration::from_secs(2)
                },
                now
            ),
            vec![Action::Nothing]
        );
        assert!(matches!(eng.state(), State::Work { .. }));
        assert_eq!(
            eng.work_remaining(clock.now_monotonic()),
            Duration::from_secs(15)
        );

        // Work still needs the full remainder from here.
        clock.advance(Duration::from_secs(14));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::Nothing]);
        clock.advance(Duration::from_secs(1));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);
    }

    #[test]
    fn long_away_resets_to_fresh_work() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(20)); // almost done
        let now = clock.now_monotonic();
        assert_eq!(
            eng.handle(
                Event::AwayEnded {
                    away: Duration::from_secs(5)
                },
                now
            ),
            vec![Action::Nothing]
        );
        match *eng.state() {
            State::Work { deadline } => {
                assert_eq!(deadline, now + short().work);
            }
            s => panic!("expected fresh Work, got {s:?}"),
        }
        assert_eq!(eng.work_remaining(now), short().work);
    }

    #[test]
    fn reminder_open_then_short_away_reshows() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);

        // Overlay hidden while locked; short return re-shows it.
        let now = clock.now_monotonic();
        assert_eq!(
            eng.handle(
                Event::AwayEnded {
                    away: Duration::from_secs(2)
                },
                now
            ),
            vec![Action::ShowReminder]
        );
        assert!(matches!(eng.state(), State::Reminder { .. }));
    }

    #[test]
    fn reminder_open_then_long_away_goes_fresh_work() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);

        let now = clock.now_monotonic();
        assert_eq!(
            eng.handle(
                Event::AwayEnded {
                    away: Duration::from_secs(10)
                },
                now
            ),
            vec![Action::HideReminder]
        );
        assert!(matches!(eng.state(), State::Work { .. }));
    }

    #[test]
    fn break_counts_real_time_while_away() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);
        assert_eq!(
            eng.handle(Event::StartBreakAction, clock.now_monotonic()),
            vec![Action::HideReminder]
        );
        // Break ends at now+5s; 5s away means ends_at has passed on return.
        clock.advance(Duration::from_secs(5));
        let now = clock.now_monotonic();
        assert_eq!(
            eng.handle(
                Event::AwayEnded {
                    away: Duration::from_secs(5)
                },
                now
            ),
            vec![Action::NotifyBreakDone]
        );
        assert!(matches!(eng.state(), State::Work { .. }));
    }

    #[test]
    fn wall_jump_does_not_shift_deadlines() {
        // Deadlines are Instant-based: moving only the wall clock must not
        // trigger or delay any transition.
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance_wall_only(Duration::from_secs(3600));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::Nothing]);
        assert!(matches!(eng.state(), State::Work { .. }));

        clock.advance_mono_only(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);
    }

    #[test]
    fn no_double_emission_on_repeated_poll() {
        let clock = FakeClock::new(Instant::now());
        let mut eng = engine_at(&clock);
        clock.advance(Duration::from_secs(25));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);
        // Second poll: already in Reminder, nothing new.
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::Nothing]);
        assert_eq!(
            eng.handle(Event::WorkExpired, clock.now_monotonic()),
            vec![Action::Nothing]
        );

        // Same for snooze -> reminder.
        assert_eq!(
            eng.handle(Event::SnoozeAction, clock.now_monotonic()),
            vec![Action::HideReminder]
        );
        clock.advance(Duration::from_secs(5));
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::ShowReminder]);
        assert_eq!(eng.poll(clock.now_monotonic()), vec![Action::Nothing]);
    }
}
