use std::cell::RefCell;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, DropDown, Label, Orientation, Spinner,
};

use crate::indicator;
use crate::recorder::{Recorder, Region};
use crate::region_selector;

/// Shared application state accessible from multiple closures.
struct AppState {
    recorder: Recorder,
    indicator_window: Option<gtk4::Window>,
    fps: u32,
}

/// Build and show the main application window.
pub fn build_ui(app: &Application) {
    // ── Shared state ──────────────────────────────────────────────────────
    let state = Rc::new(RefCell::new(AppState {
        recorder: Recorder::new(),
        indicator_window: None,
        fps: 15,
    }));

    // ── Main window ───────────────────────────────────────────────────────
    let window = ApplicationWindow::builder()
        .application(app)
        .title("GIF That For You")
        .default_width(360)
        .default_height(240)
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

    let window_btn = Button::with_label("Record Window…");

    let region_btn = Button::with_label("Select Region…");

    // ── FPS dropdown ──────────────────────────────────────────────────────
    let fps_row = GtkBox::new(Orientation::Horizontal, 8);
    let fps_label = Label::new(Some("Frame rate:"));
    let fps_dropdown = DropDown::from_strings(&["5 fps", "10 fps", "15 fps", "20 fps", "24 fps", "30 fps"]);
    fps_dropdown.set_selected(2); // default: 15 fps
    fps_row.append(&fps_label);
    fps_row.append(&fps_dropdown);

    let status_label = Label::new(Some(""));
    status_label.set_wrap(true);
    status_label.add_css_class("dim-label");

    vbox.append(&title_label);
    vbox.append(&hint_label);
    vbox.append(&full_btn);
    vbox.append(&window_btn);
    vbox.append(&region_btn);
    vbox.append(&fps_row);
    vbox.append(&status_label);

    window.set_child(Some(&vbox));

    // ── FPS dropdown → update AppState.fps ────────────────────────────────
    {
        const FPS_VALUES: [u32; 6] = [5, 10, 15, 20, 24, 30];
        let state = state.clone();
        fps_dropdown.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let fps = FPS_VALUES.get(idx).copied().unwrap_or(15);
            state.borrow_mut().fps = fps;
        });
    }

    // ── "Record Entire Screen" button ─────────────────────────────────────
    {
        let state = state.clone();
        let app_clone = app.clone();
        let window_clone = window.clone();
        let status = status_label.clone();
        full_btn.connect_clicked(move |_| {
            status.set_text("Waiting for screen selection…");
            start_recording(&app_clone, &window_clone, &state, 1, None, &status);
        });
    }

    // ── "Record Window" button ────────────────────────────────────────────
    {
        let state = state.clone();
        let app_clone = app.clone();
        let window_clone = window.clone();
        let status = status_label.clone();
        window_btn.connect_clicked(move |_| {
            status.set_text("Waiting for window selection…");
            start_recording(&app_clone, &window_clone, &state, 2, None, &status);
        });
    }

    // ── "Select Region" button ────────────────────────────────────────────
    {
        let state = state.clone();
        let app_clone = app.clone();
        let window_clone = window.clone();
        let status = status_label.clone();
        region_btn.connect_clicked(move |_| {
            // First, let the user draw a crop region on the screen.
            window_clone.hide();
            let state2 = state.clone();
            let app2 = app_clone.clone();
            let win2 = window_clone.clone();
            let status2 = status.clone();
            region_selector::show_region_selector(&app_clone, move |region| {
                status2.set_text("Waiting for screen selection…");
                win2.present();
                start_recording(&app2, &win2, &state2, 1, Some(region), &status2);
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

/// Start recording via the XDG ScreenCast portal.
///
/// The portal opens a system dialog for screen/window selection.  Once the
/// user confirms, the GStreamer pipeline starts and the recording indicator
/// appears.
fn start_recording(
    app: &Application,
    window: &ApplicationWindow,
    state: &Rc<RefCell<AppState>>,
    source_types: u32,
    crop: Option<Region>,
    status: &Label,
) {
    let recorder = state.borrow().recorder.clone();

    let app_clone = app.clone();
    let state_clone = state.clone();
    let window_clone = window.clone();
    let status_clone = status.clone();

    recorder.start_portal(source_types, crop, move |result| {
        match result {
            Ok(()) => {
                // Hide the main window while recording.
                window_clone.hide();

                let state_inner = state_clone.clone();
                let window_inner = window_clone.clone();
                let status_inner = status_clone.clone();

                let indicator_win = indicator::show_indicator(&app_clone, move || {
                    do_stop_recording(&state_inner, &window_inner, &status_inner);
                });

                state_clone.borrow_mut().indicator_window = Some(indicator_win);
            }
            Err(e) => {
                status_clone.set_text(&format!("Error: {e}"));
                window_clone.present();
            }
        }
    });
}

/// Stop the current recording, convert to GIF, and restore the main window.
///
/// The ffmpeg conversion is blocking and can take several seconds, so it runs
/// on a background thread.  A spinner window is shown while work is in
/// progress and closed once the result arrives back on the main thread via a
/// glib channel.
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
    let fps = state.borrow().fps;

    // Show "Animating GIF…" spinner window while ffmpeg runs.
    let processing_win = show_processing_window(window);

    // Run the blocking ffmpeg conversion on a background thread and send the
    // result back through a std mpsc channel.
    let (tx, rx) = std::sync::mpsc::channel::<Result<std::path::PathBuf, String>>();
    std::thread::spawn(move || {
        let _ = tx.send(recorder.stop(fps));
    });

    // Poll the channel from the GLib main loop every 100 ms (no Send
    // constraint needed for timeout_add_local).
    let processing_win_clone = processing_win.clone();
    let status_clone = status.clone();
    let window_clone = window.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match rx.try_recv() {
            Ok(result) => {
                processing_win_clone.close();
                match result {
                    Ok(path) => {
                        status_clone.set_text(&format!("Saved: {}", path.to_string_lossy()))
                    }
                    Err(e) => status_clone.set_text(&format!("Error: {e}")),
                }
                window_clone.present();
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Shouldn't happen, but stop polling and restore the window.
                processing_win_clone.close();
                window_clone.present();
                glib::ControlFlow::Break
            }
        }
    });
}

/// Show a small modal window with a spinner and "Animating GIF…" label.
///
/// The window is transient for `parent` so it stays on top and is associated
/// with the app in the taskbar.  It has no close button — it is closed
/// programmatically once conversion finishes.
fn show_processing_window(parent: &ApplicationWindow) -> gtk4::Window {
    let win = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .deletable(false)
        .title("GIF That For You")
        .default_width(220)
        .default_height(100)
        .build();

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(24);
    vbox.set_margin_bottom(24);
    vbox.set_margin_start(24);
    vbox.set_margin_end(24);
    vbox.set_halign(gtk4::Align::Center);
    vbox.set_valign(gtk4::Align::Center);

    let spinner = Spinner::new();
    spinner.set_halign(gtk4::Align::Center);
    spinner.start();

    let label = Label::new(Some("Animating GIF…"));

    vbox.append(&spinner);
    vbox.append(&label);
    win.set_child(Some(&vbox));
    win.present();
    win
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
