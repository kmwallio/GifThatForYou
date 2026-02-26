use gtk4::prelude::*;
use gtk4::{glib, Application, Box as GtkBox, Button, Label, Orientation, Window};

/// Show a small floating "Recording…" indicator window.
///
/// The window contains a pulsing label and a **Stop** button.  When the button
/// is pressed `on_stop` is called and the window closes.
///
/// Returns the indicator [`Window`] so the caller can close it programmatically
/// (e.g. when a keyboard shortcut triggers the stop action).
pub fn show_indicator<F>(app: &Application, on_stop: F) -> Window
where
    F: Fn() + 'static,
{
    let win = Window::builder()
        .application(app)
        .title("GIF That For You – Recording")
        .decorated(false)
        .resizable(false)
        .build();

    // ── Layout ────────────────────────────────────────────────────────────
    let hbox = GtkBox::new(Orientation::Horizontal, 8);
    hbox.set_margin_top(8);
    hbox.set_margin_bottom(8);
    hbox.set_margin_start(12);
    hbox.set_margin_end(12);

    // Red dot icon.
    let dot = Label::new(Some("⏺"));
    dot.add_css_class("recording-dot");

    let label = Label::new(Some("Recording…"));

    let stop_btn = Button::with_label("■ Stop");
    stop_btn.add_css_class("destructive-action");

    hbox.append(&dot);
    hbox.append(&label);
    hbox.append(&stop_btn);

    win.set_child(Some(&hbox));

    // ── Pulse animation on the dot ────────────────────────────────────────
    apply_indicator_css();
    start_pulse(&dot);

    // ── Stop button action ────────────────────────────────────────────────
    let win_clone = win.clone();
    stop_btn.connect_clicked(move |_| {
        win_clone.close();
        on_stop();
    });

    win.present();
    win
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn apply_indicator_css() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(
        "label.recording-dot { color: #e01b24; font-size: 16px; }\
         label.recording-dot.dim { opacity: 0.3; }",
    );
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().unwrap(),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Blink the red dot by toggling the `dim` CSS class every 600 ms.
fn start_pulse(dot: &Label) {
    let dot = dot.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(600), move || {
        if dot.has_css_class("dim") {
            dot.remove_css_class("dim");
        } else {
            dot.add_css_class("dim");
        }
        // Stop the timeout once the label is no longer part of a window.
        if dot.root().is_some() {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
}
