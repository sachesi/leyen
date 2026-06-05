//! `leyen-gtk` — the GTK4/libadwaita client. A thin frontend: the engine lives
//! in `leyend`, reached over D-Bus via the zbus↔glib bridge (`daemon`). No tokio
//! runtime on the GTK thread; signals drive the UI, method calls drive actions.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

mod daemon;
mod desktop;
mod icons;
mod prefix_tools;
mod ui;

// The GUI's GApplication id (drives single-instance + desktop/icon matching).
// Distinct from the daemon's bus name (`leyen_ipc::BUS_NAME`).
const APP_ID: &str = leyen_model::APP_ID;

fn main() -> glib::ExitCode {
    leyen_model::i18n::init();

    // Start the D-Bus bridge before the UI so early signals queue rather than drop.
    let evt_rx = Rc::new(RefCell::new(Some(daemon::start())));

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(move |app| {
        // gio uniqueness: a second invocation re-activates the primary instance;
        // just present the existing window.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        if let Some(rx) = evt_rx.borrow_mut().take() {
            daemon::run_event_dispatch(rx);
        }
        ui::build_ui(app);
    });

    app.run()
}
