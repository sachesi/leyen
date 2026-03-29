use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

mod config;
mod deps;
mod launch;
mod logging;
mod models;
mod proton;
mod ui;
mod umu;

const APP_ID: &str = "com.github.leyen";

fn main() -> glib::ExitCode {
    umu::check_or_install_umu();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(ui::build_ui);
    app.run()
}
