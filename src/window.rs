use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation};

use crate::indicator;
use crate::recorder::{Recorder, Region};
use crate::region_selector;

/// Shared application state accessible from multiple closures.
struct AppState {
    recorder: Recorder,
    indicator_window: Option<gtk4::Window>,
}

/// Build and show the main application window.
pub fn build_ui(app: &Application) {
    // ── Shared state ──────────────────────────────────────────────────────
    let state = Rc::new(RefCell::new(AppState {
        recorder: Recorder::new(),
        indicator_window: None,
    }));

    // ── Main window ───────────────────────────────────────────────────────
    let window = ApplicationWindow::builder()
        .application(app)
        .title("GIF That For You")
        .default_width(360)
        .default_height(180)
        .resizable(false)
        .build();

    // ── Layout ────────────────────────────────────────────────────────────
    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(24);
    vbox.set_margin_bottom(24);
    vbox.set_margin_start(24);
    vbox.set_margin_end(24);

    let title_label = Label::new(None);
    title_label.set_markup("<b>GIF That For You</b>");

    let hint_label = Label::new(Some("Record your screen as a GIF"));
    hint_label.add_css_class("dim-label");

    let full_btn = Button::with_label("Record Entire Screen");
    full_btn.add_css_class("suggested-action");

    let region_btn = Button::with_label("Select Region…");

    let status_label = Label::new(Some(""));
    status_label.set_wrap(true);
    status_label.add_css_class("dim-label");

    vbox.append(&title_label);
    vbox.append(&hint_label);
    vbox.append(&full_btn);
    vbox.append(&region_btn);
    vbox.append(&status_label);

    window.set_child(Some(&vbox));

    // ── "Record Entire Screen" button ─────────────────────────────────────
    {
        let state = state.clone();
        let app_clone = app.clone();
        let window_clone = window.clone();
        let status = status_label.clone();
        full_btn.connect_clicked(move |_| {
            start_recording(&app_clone, &window_clone, &state, None, &status);
        });
    }

    // ── "Select Region" button ────────────────────────────────────────────
    {
        let state = state.clone();
        let app_clone = app.clone();
        let window_clone = window.clone();
        let status = status_label.clone();
        region_btn.connect_clicked(move |_| {
            // Hide main window while the user selects the region.
            window_clone.hide();
            let state2 = state.clone();
            let app2 = app_clone.clone();
            let win2 = window_clone.clone();
            let status2 = status.clone();
            region_selector::show_region_selector(&app_clone, move |region| {
                start_recording(&app2, &win2, &state2, Some(region), &status2);
            });
        });
    }

    // ── Keyboard shortcut: Ctrl+Shift+R → stop recording ─────────────────
    {
        let state = state.clone();
        let window_clone = window.clone();
        let status = status_label.clone();
        register_stop_shortcut(app, state, window_clone, status);
    }

    window.present();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Start recording (full screen or a specific region) and show the indicator.
fn start_recording(
    app: &Application,
    window: &ApplicationWindow,
    state: &Rc<RefCell<AppState>>,
    region: Option<Region>,
    status: &Label,
) {
    let recorder = state.borrow().recorder.clone();
    match recorder.start(region) {
        Ok(()) => {
            // Hide the main window while recording.
            window.hide();

            let state_clone = state.clone();
            let window_clone = window.clone();
            let status_clone = status.clone();

            let indicator_win = indicator::show_indicator(app, move || {
                do_stop_recording(&state_clone, &window_clone, &status_clone);
            });

            state.borrow_mut().indicator_window = Some(indicator_win);
        }
        Err(e) => {
            status.set_text(&format!("Error: {e}"));
            window.present();
        }
    }
}

/// Stop the current recording, convert to GIF, and restore the main window.
fn do_stop_recording(
    state: &Rc<RefCell<AppState>>,
    window: &ApplicationWindow,
    status: &Label,
) {
    // Close the indicator window first.
    if let Some(ind) = state.borrow_mut().indicator_window.take() {
        ind.close();
    }

    let recorder = state.borrow().recorder.clone();
    match recorder.stop() {
        Ok(path) => {
            status.set_text(&format!("Saved: {}", path.to_string_lossy()));
        }
        Err(e) => {
            status.set_text(&format!("Error: {e}"));
        }
    }

    window.present();
}

/// Register `Ctrl+Shift+R` as an application-level action to stop recording.
fn register_stop_shortcut(
    app: &Application,
    state: Rc<RefCell<AppState>>,
    window: ApplicationWindow,
    status: Label,
) {
    let stop_action = gtk4::gio::SimpleAction::new("stop-recording", None);
    stop_action.connect_activate(move |_, _| {
        if state.borrow().recorder.is_recording() {
            do_stop_recording(&state, &window, &status);
        }
    });
    app.add_action(&stop_action);
    // Ctrl+Shift+R stops any active recording.
    app.set_accels_for_action("app.stop-recording", &["<Control><Shift>R"]);
}
