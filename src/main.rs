use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

mod cli;
mod config;
mod deps;
mod desktop;
mod icons;
mod instance;
mod launch;
mod logging;
mod models;
mod prefix_tools;
mod runtime;
mod tools;
mod ui;

const APP_ID: &str = "com.github.sachesi.leyen";

#[tokio::main]
async fn main() -> glib::ExitCode {
    if let Err(e) = logging::init() {
        eprintln!("Failed to initialize logging: {e}");
        return glib::ExitCode::FAILURE;
    }

    if let Some(exit_code) = cli::maybe_run_from_args().await {
        return exit_code;
    }

    let _lock = match instance::InstanceLock::acquire() {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!("{err}");
            return glib::ExitCode::FAILURE;
        }
    };

    runtime::check_or_install_umu();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(ui::build_ui);
    app.run()
}
