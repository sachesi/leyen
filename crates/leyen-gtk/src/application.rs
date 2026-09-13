//! The application: one window, the `app.*` actions and the bridge to the daemon.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::{gio, glib};
use leyen_model::i18n::gettext;
use libadwaita as adw;

use crate::daemon;
use crate::dialogs::PreferencesDialog;
use crate::window::LeyenWindow;

pub const RESOURCE_PATH: &str = "/io/github/sachesi/leyen";

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct LeyenApplication;

    #[glib::object_subclass]
    impl ObjectSubclass for LeyenApplication {
        const NAME: &'static str = "LeyenApplication";
        type Type = super::LeyenApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for LeyenApplication {}

    impl ApplicationImpl for LeyenApplication {
        fn startup(&self) {
            self.parent_startup();
            // The primary instance only: a second launch just activates this one.
            daemon::run_event_dispatch(daemon::start());
            self.obj().setup_actions();
        }

        fn activate(&self) {
            self.obj().main_window().present();
        }
    }

    impl GtkApplicationImpl for LeyenApplication {}
    impl AdwApplicationImpl for LeyenApplication {}
}

glib::wrapper! {
    pub struct LeyenApplication(ObjectSubclass<imp::LeyenApplication>)
        @extends adw::Application, gtk4::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for LeyenApplication {
    fn default() -> Self {
        glib::Object::builder()
            .property("application-id", leyen_model::APP_ID)
            .property("resource-base-path", RESOURCE_PATH)
            .build()
    }
}

impl LeyenApplication {
    /// The library window, created on first use. Hidden rather than closed while a
    /// game runs, so it is found here again.
    pub fn main_window(&self) -> LeyenWindow {
        self.windows()
            .into_iter()
            .find_map(|window| window.downcast::<LeyenWindow>().ok())
            .unwrap_or_else(|| LeyenWindow::new(self))
    }

    fn setup_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| app.quit())
            .build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about())
            .build();
        let preferences = gio::ActionEntry::builder("preferences")
            .activate(|app: &Self, _, _| {
                let window = app.main_window();
                glib::spawn_future_local(async move {
                    PreferencesDialog::present(&window).await;
                });
            })
            .build();
        self.add_action_entries([quit, about, preferences]);

        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("app.preferences", &["<Control>comma"]);
        self.set_accels_for_action("win.toggle-search", &["<Control>f"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Leyen")
            .application_icon(leyen_model::APP_ID)
            .developer_name("sachesi")
            .version(env!("CARGO_PKG_VERSION"))
            .website("https://github.com/sachesi/leyen")
            .issue_url("https://github.com/sachesi/leyen/issues")
            .license_type(gtk4::License::Gpl30)
            .comments(gettext("Run Windows games with Proton"))
            // Translators: put your name here, one per line, optionally with an email address.
            .translator_credits(gettext("translator-credits"))
            .build();
        about.present(Some(&self.main_window()));
    }
}
