use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

mod cli;
mod config;
mod deps;
mod desktop;
mod i18n;
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

    // TEMP DEBUG: log every panic (message + location + backtrace) instead of
    // letting it unwind silently across the GLib FFI boundary, which can wedge
    // the main loop. Set RUST_BACKTRACE=1 for frames.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!(target: "dbg", "[DBG PANIC] at {location}: {info}\n{backtrace}");
        eprintln!("[DBG PANIC] at {location}: {info}\n{backtrace}");
    }));

    i18n::init();

    if let Some(exit_code) = cli::maybe_run_from_args().await {
        return exit_code;
    }

    let _lock = match instance::InstanceLock::acquire() {
        Ok(lock) => lock,
        Err(instance::InstanceError::AlreadyRunning) => {
            let _ = instance::signal_show_window();
            return glib::ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("{err}");
            return glib::ExitCode::FAILURE;
        }
    };

    let settings = config::load_settings().await;
    logging::apply_log_settings(&settings);
    logging::maybe_enable_debug_logging(); // TEMP DEBUG

    runtime::check_or_install_umu().await;
    runtime::check_or_install_winetricks().await;
    launch::reconcile_stale_sessions_on_startup().await;
    launch::start_running_sessions_monitor();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(ui::build_ui);
    let exit_code = app.run();

    logging::shutdown();

    exit_code
}
