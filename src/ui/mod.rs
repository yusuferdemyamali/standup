//! `ui`: GTK4 + gtk4-layer-shell overlay (one surface per monitor).
//!
//! CONTRACT: `build_app(cfg: crate::config::AppConfig) -> gtk4::Application`
//! owns the [`crate::domain::TimerEngine`], polls it on a GLib tick, shows one
//! overlay per monitor via `overlay::show_on_all_monitors`, and tears them all
//! down on any single action (AC-05). Esc == Snooze (spec §3.1).
//!
//! Break UX (spec §3.2): after "Harekete Başla" there is NO persistent window;
//! the app runs windowless until the next reminder or a break-done
//! notification.

pub mod overlay;

use crate::{
    config::AppConfig,
    domain::{Action, Event, State, TimerEngine},
};
use gtk4::prelude::*;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

/// Shared state behind the GLib tick and every overlay callback.
struct Shared {
    app: gtk4::Application,
    engine: RefCell<TimerEngine>,
    overlays: RefCell<Option<overlay::OverlayGuard>>,
    away: crate::platform::session::AwayTracker,
    was_away: RefCell<bool>,
    last_away: RefCell<Duration>,
    /// Keeps the headless app alive: dropping this would let GApplication
    /// quit right after startup (gio `hold()` is RAII).
    _hold: gio::ApplicationHoldGuard,
}

impl Shared {
    /// Attempt to show the reminder on all monitors (no-op if visible).
    /// Returns the layer-shell error so callers can pick the error policy.
    fn try_show(self: &Rc<Self>) -> Result<(), overlay::LayerShellError> {
        if self.overlays.borrow().is_some() {
            return Ok(());
        }
        let this = self.clone();
        let on_action: Rc<dyn Fn(overlay::OverlayAction)> =
            Rc::new(move |action| this.on_overlay_action(action));
        match overlay::show_on_all_monitors(&self.app, on_action) {
            Ok(guard) => {
                self.overlays.borrow_mut().replace(guard);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Timer/initial show path. AC-11: layer-shell fundamentally
    /// unsupported → explain and exit non-zero, never a half-working
    /// fullscreen. Transient failures (no display / no monitors /
    /// monitor-list race during hotplug) are non-fatal: log, stay hidden,
    /// and let the tick retry on the next poll while state is Reminder.
    fn show(self: &Rc<Self>) {
        if let Err(err) = self.try_show() {
            if err == overlay::LayerShellError::Unsupported {
                log::error!("standup: cannot create layer-shell overlay: {err}");
                eprintln!(
                    "standup: Wayland layer-shell is not available ({err}). \
                     Not showing a partial fullscreen overlay; exiting."
                );
                std::process::exit(2);
            }
            log::error!(
                "standup: overlay show failed transiently ({err}); \
                 will retry while reminder is pending."
            );
        }
    }

    /// Close every surface. Dropping the guard closes the windows (AC-08).
    /// Split into take-then-drop so re-entrant close-requests during teardown
    /// see `None` instead of tripping a RefCell double-borrow.
    fn hide(&self) {
        let taken = self.overlays.borrow_mut().take();
        drop(taken);
    }

    fn apply(self: &Rc<Self>, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::ShowReminder => self.show(),
                Action::HideReminder => self.hide(),
                Action::NotifyBreakDone => {
                    // Fixed spec message lives in the platform lane; tolerate
                    // either `()` or `Result` return shape.
                    let _ = crate::platform::notifications::notify_break_done();
                }
                Action::Nothing => {}
            }
        }
    }

    /// Any single surface action closes ALL surfaces first (AC-05), then
    /// feeds the engine. Late/duplicate callbacks after teardown are ignored.
    fn on_overlay_action(self: &Rc<Self>, action: overlay::OverlayAction) {
        if self.overlays.borrow().is_none() {
            return;
        }
        self.hide();
        let now = Instant::now();
        let event = match action {
            overlay::OverlayAction::StartBreak => Event::StartBreakAction,
            overlay::OverlayAction::Snooze => Event::SnoozeAction,
        };
        let actions = self.engine.borrow_mut().handle(event, now);
        self.apply(actions);
    }

    /// 500 ms poll: monotonic engine deadlines prevent drift (AC-07).
    /// While the session is away (locked/suspended) the overlay is hidden
    /// (never drawn over the lock screen), the engine is not polled (work
    /// time does not count), and on return a single `AwayEnded` with the
    /// wall-measured absence flows in (AC-06: >= 5 min → fresh WORK).
    fn tick(self: &Rc<Self>) {
        let now = Instant::now();
        if self.away.is_away() {
            if let Some(elapsed) = self.away.away_elapsed_wall() {
                *self.last_away.borrow_mut() = elapsed;
            }
            *self.was_away.borrow_mut() = true;
            self.hide();
            return;
        }
        let mut engine = self.engine.borrow_mut();
        let mut actions = Vec::new();
        if *self.was_away.borrow() {
            *self.was_away.borrow_mut() = false;
            let away = *self.last_away.borrow();
            *self.last_away.borrow_mut() = Duration::ZERO;
            actions.extend(engine.handle(Event::AwayEnded { away }, now));
        }
        actions.extend(engine.poll(now));
        let reminded = matches!(*engine.state(), State::Reminder { .. });
        let attempted_show = actions.iter().any(|a| matches!(a, Action::ShowReminder));
        drop(engine);
        self.apply(actions);
        // Retry a reminder that is pending but not visible (e.g. a transient
        // NoMonitors / monitor-list race during hotplug left it hidden).
        // `poll` in Reminder returns Nothing, so without this the reminder
        // would stay hidden forever. Never fires while away (overlay must
        // not cover the lock screen) and never fires twice in one tick.
        if reminded && !attempted_show && self.overlays.borrow().is_none() {
            self.show();
        }
    }

    /// Monitor hotplug must not crash (AC-05): if the reminder is visible,
    /// rebuild the per-monitor surfaces for the new topology. Never fatal:
    /// a transient NoMonitors / item race leaves the reminder hidden and
    /// the tick retry above re-shows it once monitors settle.
    fn refresh_after_monitor_change(self: &Rc<Self>) {
        if self.overlays.borrow().is_some() {
            self.hide();
            if let Err(err) = self.try_show() {
                log::error!(
                    "standup: overlay refresh after monitor change failed ({err}); \
                     will retry while reminder is pending."
                );
            }
        }
    }
}

/// Build the application. Owns the [`TimerEngine`] (work/snooze/break
/// durations from config, 5-minute away threshold), polls it every 500 ms,
/// and shows/hides the fullscreen overlays. No normal window: the app runs
/// headless and windowless until the timer fires (spec §3.2).
pub fn build_app(cfg: AppConfig) -> gtk4::Application {
    let app = gtk4::Application::builder()
        .application_id("com.example.StandUp")
        .build();

    app.connect_startup(move |app| {
        // Background app with no persistent window: hold it explicitly
        // (guard kept in Shared), otherwise GApplication quits after startup.
        let hold = app.hold();
        let Some(display) = gtk4::gdk::Display::default() else {
            eprintln!("standup: no display available; cannot run.");
            std::process::exit(2);
        };

        // Embedded theme: the release binary has no runtime asset dependency.
        let css = gtk4::CssProvider::new();
        css.load_from_string(include_str!("styles.css"));
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let engine = TimerEngine::new(cfg.durations(), Instant::now());
        let away = crate::platform::session::AwayTracker::new();
        let shared = Rc::new(Shared {
            app: app.clone(),
            engine: RefCell::new(engine),
            overlays: RefCell::new(None),
            away,
            was_away: RefCell::new(false),
            last_away: RefCell::new(Duration::ZERO),
            _hold: hold,
        });

        // Monitor hotplug watch (AC-05).
        {
            let monitors = display.monitors();
            let watch = shared.clone();
            monitors.connect_items_changed(move |_, _, _, _| {
                watch.refresh_after_monitor_change();
            });
        }

        // Engine poll.
        {
            let ticking = shared.clone();
            gtk4::glib::timeout_add_local(Duration::from_millis(500), move || {
                ticking.tick();
                gtk4::glib::ControlFlow::Continue
            });
        }

        // Teardown on shutdown: no lingering surfaces (AC-08).
        {
            let closing = shared.clone();
            app.connect_shutdown(move |_| closing.hide());
        }
    });

    // Intentionally no `connect_activate` window: headless until the first
    // reminder, and break time stays windowless too.
    app
}
