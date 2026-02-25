mod indicator;
mod recorder;
mod region_selector;
mod window;

use gtk4::prelude::*;
use gtk4::{glib, Application};

const APP_ID: &str = "io.github.kmwallio.GifThatForYou";

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(|app| {
        window::build_ui(app);
    });

    app.run()
}
