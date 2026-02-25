use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::{gdk, glib, Application, DrawingArea, GestureDrag, Window};

use crate::recorder::Region;

/// Show a full-screen transparent overlay that lets the user drag to choose a
/// recording region.
///
/// `on_selected` is called when the user releases the mouse button;  it
/// receives the chosen [`Region`] (always normalised so that width/height are
/// positive).  If the user presses Escape the overlay closes without calling
/// the callback.
pub fn show_region_selector<F>(app: &Application, on_selected: F)
where
    F: Fn(Region) + 'static,
{
    let overlay = Window::builder()
        .application(app)
        .title("Select Region")
        .decorated(false)
        .fullscreened(true)
        .build();

    // Make the window transparent so the desktop is visible below.
    overlay.add_css_class("region-selector");
    apply_transparent_css();

    // ── Drawing area ──────────────────────────────────────────────────────
    let drawing = DrawingArea::new();
    drawing.set_hexpand(true);
    drawing.set_vexpand(true);

    // Shared state: start point (None before first click) and current end.
    let start: std::cell::Cell<Option<(f64, f64)>> = std::cell::Cell::new(None);
    let current: std::cell::Cell<(f64, f64)> = std::cell::Cell::new((0.0, 0.0));

    let start = std::rc::Rc::new(start);
    let current = std::rc::Rc::new(current);

    // Draw the selection rectangle.
    {
        let start = start.clone();
        let current = current.clone();
        drawing.set_draw_func(move |_area, cr, _w, _h| {
            // Semi-transparent dark overlay.
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.4);
            let _ = cr.paint();

            if let Some((sx, sy)) = start.get() {
                let (ex, ey) = current.get();
                let x = sx.min(ex);
                let y = sy.min(ey);
                let w = (ex - sx).abs();
                let h = (ey - sy).abs();

                // Clear the selected region so the user can see through it.
                cr.set_operator(cairo::Operator::Clear);
                cr.rectangle(x, y, w, h);
                let _ = cr.fill();

                // Draw a bright selection border.
                cr.set_operator(cairo::Operator::Over);
                cr.set_source_rgba(0.2, 0.6, 1.0, 1.0);
                cr.set_line_width(2.0);
                cr.rectangle(x, y, w, h);
                let _ = cr.stroke();
            }
        });
    }

    // ── Drag gesture ──────────────────────────────────────────────────────
    let gesture = GestureDrag::new();
    gesture.set_button(gdk::BUTTON_PRIMARY);

    {
        let start = start.clone();
        let current = current.clone();
        let drawing2 = drawing.clone();
        gesture.connect_drag_begin(move |_, x, y| {
            start.set(Some((x, y)));
            current.set((x, y));
            drawing2.queue_draw();
        });
    }

    {
        let current = current.clone();
        let drawing2 = drawing.clone();
        gesture.connect_drag_update(move |gesture, ox, oy| {
            if let Some(start_pt) = gesture.start_point() {
                current.set((start_pt.0 + ox, start_pt.1 + oy));
                drawing2.queue_draw();
            }
        });
    }

    let on_selected = std::rc::Rc::new(on_selected);
    let overlay_clone = overlay.clone();
    {
        let start = start.clone();
        let on_selected = on_selected.clone();
        gesture.connect_drag_end(move |gesture, ox, oy| {
            if let Some((sx, sy)) = start.get() {
                if let Some(start_pt) = gesture.start_point() {
                    let ex = start_pt.0 + ox;
                    let ey = start_pt.1 + oy;

                    // GDK gesture coordinates are in logical (device-independent)
                    // pixels.  The PipeWire screencast stream is delivered in
                    // physical pixels, so scale the crop region up to match.
                    let scale = gesture
                        .widget()
                        .map(|w| w.scale_factor())
                        .unwrap_or(1) as f64;

                    let x = (sx.min(ex) * scale) as i32;
                    let y = (sy.min(ey) * scale) as i32;
                    let w = ((ex - sx).abs() * scale) as i32;
                    let h = ((ey - sy).abs() * scale) as i32;

                    // Require a minimum selection size.
                    if w > 10 && h > 10 {
                        overlay_clone.close();
                        on_selected(Region {
                            x,
                            y,
                            width: w,
                            height: h,
                        });
                    }
                }
            }
        });
    }

    drawing.add_controller(gesture);

    // ── Escape key closes the overlay ────────────────────────────────────
    let key_controller = gtk4::EventControllerKey::new();
    let overlay_clone2 = overlay.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            overlay_clone2.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    overlay.add_controller(key_controller);

    overlay.set_child(Some(&drawing));
    overlay.present();
}

/// Inject CSS that makes the region-selector window fully transparent.
fn apply_transparent_css() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(
        "window.region-selector { background-color: transparent; }",
    );
    gtk4::style_context_add_provider_for_display(
        &gdk::Display::default().unwrap(),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
