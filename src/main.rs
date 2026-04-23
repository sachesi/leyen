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
    app.connect_activate(|app| {
        let css = gtk4::CssProvider::new();
        css.load_from_string(
            r#"
            .game-card {
              border-radius: 12px;
              box-shadow: 0 2px 8px rgba(0,0,0,0.4);
              transition: box-shadow 120ms ease;
              padding: 0;
            }
            .game-card:hover {
              box-shadow: 0 4px 16px rgba(0,0,0,0.6);
            }
            .game-card-title-bar {
              background: linear-gradient(to bottom, transparent, rgba(0,0,0,0.75));
              padding: 24px 8px 8px 8px;
            }
            .game-card-title {
              color: white;
              font-weight: 700;
            }
            .game-card-placeholder {
              background: rgba(255,255,255,0.08);
            }
            .game-card-menu {
              opacity: 0;
            }
            .game-card:hover .game-card-menu {
              opacity: 1;
            }
            "#,
        );
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &css,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        ui::build_ui(app)
    });
    app.run()
}
