//! `leyen-gtk` — the GTK4/libadwaita client. A thin frontend: the engine lives
//! in `leyend`, reached over D-Bus via the zbus↔glib bridge (`daemon`). No tokio
//! runtime on the GTK thread; signals drive the UI, method calls drive actions.

use gtk4::prelude::*;
use gtk4::{gio, glib};

mod application;
mod daemon;
mod desktop;
mod dialogs;
mod format;
mod game_row;
mod group_row;
mod icons;
mod library_icon;
mod log_window;
mod migrate;
mod playback;
mod prefix_tools;
mod running_games;
mod window;

fn main() -> glib::ExitCode {
    leyen_model::i18n::init();
    // Before the main loop, so the library never looks for an icon mid-rename.
    migrate::migrate_legacy_app_id();
    gio::resources_register_include!("leyen.gresource").expect("the resources are compiled in");
    glib::set_application_name("Leyen");
    application::LeyenApplication::default().run()
}
