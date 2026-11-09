//! Fullscreen reminder surfaces: one layer-shell window per monitor.
//!
//! Design notes (main_spec.md §3.1, AC-05):
//! - Every surface shows the same content and state; an action on ANY surface
//!   closes ALL of them. The caller owns that fan-in via [`OverlayAction`].
//! - Exactly ONE surface (the first monitor) requests
//!   [`KeyboardMode::Exclusive`](gtk4_layer_shell::KeyboardMode); every other
//!   surface uses `OnDemand` so it stays mouse-clickable.
//! - This is not a lock screen: `Esc` behaves like Snooze, and Alt-F4 /
//!   close-request is allowed through (it snoozes instead of trapping).
//! - HiDPI safety: no fixed pixel sizes in code; all sizing lives in
//!   `styles.css` in pt/em units.
//! - Reduced motion: the enter animation class is only added when the desktop
//!   still has animations enabled (`gtk-enable-animations` as proxy).

use gtk4::gdk;
use gtk4::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::rc::Rc;

/// User intent coming from any single overlay surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAction {
    StartBreak,
    Snooze,
}

/// Why no overlay could be shown. The caller maps this to the AC-11 path:
/// explain and exit non-zero instead of a half-working fullscreen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerShellError {
    /// No Wayland compositor with the layer-shell protocol.
    Unsupported,
    /// No GDK display at all.
    NoDisplay,
    /// Display exists but reports zero monitors.
    NoMonitors,
}

impl std::fmt::Display for LayerShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayerShellError::Unsupported => write!(f, "layer-shell protocol not supported"),
            LayerShellError::NoDisplay => write!(f, "no display available"),
            LayerShellError::NoMonitors => write!(f, "no monitors reported"),
        }
    }
}

/// Owns every overlay surface. Dropping it closes all windows, so there are
/// no lingering surfaces on the crash/shutdown path (AC-08).
pub struct OverlayGuard {
    windows: Vec<gtk4::ApplicationWindow>,
}

impl Drop for OverlayGuard {
    fn drop(&mut self) {
        for window in &self.windows {
            window.close();
        }
    }
}

/// Show one fullscreen overlay per currently connected monitor.
///
/// Returns a guard holding all surfaces; drop it (or let the caller hide it)
/// to close everything at once. Fails cleanly — without creating anything
/// half-working — when layer-shell is unavailable (AC-11).
pub fn show_on_all_monitors(
    app: &gtk4::Application,
    on_action: Rc<dyn Fn(OverlayAction)>,
) -> Result<OverlayGuard, LayerShellError> {
    if !gtk4_layer_shell::is_supported() {
        return Err(LayerShellError::Unsupported);
    }
    let display = gdk::Display::default().ok_or(LayerShellError::NoDisplay)?;
    let monitors = display.monitors();
    let count = monitors.n_items();
    if count == 0 {
        return Err(LayerShellError::NoMonitors);
    }

    let animate = animations_enabled(&display);
    let mut windows = Vec::new();
    let mut start_buttons = Vec::new();
    for index in 0..count {
        let item = monitors.item(index).ok_or(LayerShellError::NoMonitors)?;
        let monitor = item
            .downcast::<gdk::Monitor>()
            .map_err(|_| LayerShellError::NoMonitors)?;
        let first = index == 0;
        let (window, start_button) = build_surface(app, &monitor, first, animate, &on_action);
        window.present();
        windows.push(window);
        if first {
            start_buttons.push(start_button);
        }
    }
    // Keyboard path: focus the primary action on the exclusive surface so
    // Tab/Enter works immediately (AC-02).
    if let Some(primary) = start_buttons.into_iter().next() {
        primary.grab_focus();
    }
    Ok(OverlayGuard { windows })
}

/// `true` unless the user asked for reduced motion. Uses the GTK
/// `gtk-enable-animations` setting as the proxy (spec §3.1).
fn animations_enabled(display: &gdk::Display) -> bool {
    gtk4::Settings::for_display(display).is_gtk_enable_animations()
}

fn build_surface(
    app: &gtk4::Application,
    monitor: &gdk::Monitor,
    primary_monitor: bool,
    animate: bool,
    on_action: &Rc<dyn Fn(OverlayAction)>,
) -> (gtk4::ApplicationWindow, gtk4::Button) {
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .decorated(false)
        .build();
    window.add_css_class("su-root");

    // Must run before the window is realized/mapped.
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("standup-reminder"));
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.set_monitor(Some(monitor));
    window.set_keyboard_mode(if primary_monitor {
        KeyboardMode::Exclusive
    } else {
        // Stays mouse-clickable; takes keyboard only when clicked.
        KeyboardMode::OnDemand
    });

    let (content, start_button) = build_content(on_action, animate);
    window.set_child(Some(&content));

    // Esc == Snooze (spec §3.1).
    {
        let action = on_action.clone();
        let keys = gtk4::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                action(OverlayAction::Snooze);
                gtk4::glib::Propagation::Stop
            } else {
                gtk4::glib::Propagation::Proceed
            }
        });
        window.add_controller(keys);
    }
    // Never trap session controls: closing a surface snoozes and the caller
    // tears down every surface (AC-02, AC-05).
    {
        let action = on_action.clone();
        window.connect_close_request(move |_| {
            action(OverlayAction::Snooze);
            gtk4::glib::Propagation::Proceed
        });
    }

    (window, start_button)
}

/// Builds the centered reminder content. Returns the root widget plus the
/// primary button (for initial keyboard focus).
fn build_content(
    on_action: &Rc<dyn Fn(OverlayAction)>,
    animate: bool,
) -> (gtk4::Box, gtk4::Button) {
    let center = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .spacing(12)
        .build();
    center.add_css_class("su-center");
    if animate {
        center.add_css_class("su-enter");
    }

    // Movement glyph: pure-CSS ring + a widely available arrow glyph.
    // No binary assets, no network.
    let ring = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .build();
    ring.add_css_class("su-glyph-ring");
    let arrow = gtk4::Label::new(Some("\u{2197}"));
    arrow.add_css_class("su-glyph-arrow");
    arrow.set_halign(gtk4::Align::Center);
    arrow.set_valign(gtk4::Align::Center);
    arrow.set_hexpand(true);
    arrow.set_vexpand(true);
    ring.append(&arrow);
    let glyph = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .halign(gtk4::Align::Center)
        .build();
    glyph.add_css_class("su-glyph");
    glyph.append(&ring);

    let title = gtk4::Label::new(Some("Biraz hareket zaman\u{131}."));
    title.add_css_class("su-title");
    title.set_wrap(true);
    title.set_justify(gtk4::Justification::Center);
    title.set_halign(gtk4::Align::Center);

    let helper = gtk4::Label::new(Some(
        "25 dakikad\u{131}r bilgisayar ba\u{15F}\u{131}ndas\u{131}n. \
         Kalk, biraz y\u{FC}r\u{FC} ve kendine 5 dakika ay\u{131}r.",
    ));
    helper.add_css_class("su-helper");
    helper.set_wrap(true);
    helper.set_max_width_chars(48);
    helper.set_justify(gtk4::Justification::Center);
    helper.set_halign(gtk4::Align::Center);

    let actions = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .halign(gtk4::Align::Center)
        .spacing(16)
        .build();
    actions.add_css_class("su-actions");

    let start = gtk4::Button::with_label("Harekete Ba\u{15F}la");
    start.add_css_class("su-btn");
    start.add_css_class("su-primary");
    {
        let action = on_action.clone();
        start.connect_clicked(move |_| action(OverlayAction::StartBreak));
    }
    let snooze = gtk4::Button::with_label("5 Dakika Ertele");
    snooze.add_css_class("su-btn");
    snooze.add_css_class("su-secondary");
    {
        let action = on_action.clone();
        snooze.connect_clicked(move |_| action(OverlayAction::Snooze));
    }
    actions.append(&start);
    actions.append(&snooze);

    center.append(&glyph);
    center.append(&title);
    center.append(&helper);
    center.append(&actions);

    (center, start)
}
