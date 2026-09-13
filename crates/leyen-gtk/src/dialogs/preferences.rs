//! The preferences: the default prefix and Proton, the environment every game
//! starts with, logging and maintenance. Saved when the dialog closes.

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_model::i18n::gettext;
use leyen_model::models::{GLOBAL_SETTINGS_VERSION, GlobalSettings};
use leyen_model::runtime::{get_umu_runtime_dir, resolve_proton_path};
use leyen_model::tools::{gamemode_available, mangohud_available};
use libadwaita as adw;

use super::{PrefixToolsGroup, ProtonChoices, ToolTarget, choose_folder_into};
use crate::daemon::{self, gio_blocking};

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/preferences_dialog.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub prefix_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub proton_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub tools_group: TemplateChild<PrefixToolsGroup>,
        #[template_child]
        pub mangohud_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub gamemode_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub wayland_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub wow64_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub ntsync_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub hdr_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub proton_log_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub log_errors_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub log_warnings_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub log_operations_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub shared_container_row: TemplateChild<adw::SwitchRow>,
        pub protons: RefCell<Option<ProtonChoices>>,
        pub settings: RefCell<GlobalSettings>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "LeyenPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            PrefixToolsGroup::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PreferencesDialog {}
    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}

    #[gtk4::template_callbacks]
    impl PreferencesDialog {
        #[template_callback]
        fn on_browse_prefix(&self, _button: &gtk4::Button) {
            let row = self.prefix_row.get();
            glib::spawn_future_local(async move {
                choose_folder_into(&row, &gettext("Select Prefix Folder")).await;
            });
        }

        #[template_callback]
        fn on_reset_runtime(&self, _button: &gtk4::Button) {
            let obj = self.obj().clone();
            glib::spawn_future_local(async move { obj.reset_runtime().await });
        }

        #[template_callback]
        fn on_closed(&self, _dialog: &adw::Dialog) {
            let settings = self.obj().collect_settings();
            glib::spawn_future_local(async move {
                // Settings are client-owned; the daemon re-reads them (incl. log
                // levels) when told to.
                daemon::save_settings(settings).await;
            });
        }
    }
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::ShortcutManager;
}

impl PreferencesDialog {
    pub async fn present(parent: &impl IsA<gtk4::Widget>) {
        let settings = daemon::load_settings().await;
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();

        imp.prefix_row.set_text(&settings.default_prefix_path);
        let protons = ProtonChoices::new(&settings);
        imp.proton_row.set_model(Some(&protons.model));
        imp.proton_row
            .set_selected(protons.position(&settings.default_proton));
        imp.protons.replace(Some(protons));

        imp.mangohud_row.set_visible(mangohud_available());
        imp.mangohud_row.set_active(settings.global_mangohud);
        imp.gamemode_row.set_visible(gamemode_available());
        imp.gamemode_row.set_active(settings.global_gamemode);
        imp.wayland_row.set_active(settings.global_wayland);
        imp.wow64_row.set_active(settings.global_wow64);
        imp.ntsync_row.set_active(settings.global_ntsync);
        imp.hdr_row.set_active(settings.global_hdr);
        imp.proton_log_row.set_active(settings.global_proton_log);
        imp.log_errors_row.set_active(settings.log_errors);
        imp.log_warnings_row.set_active(settings.log_warnings);
        imp.log_operations_row.set_active(settings.log_operations);
        imp.shared_container_row
            .set_active(settings.use_shared_container);
        imp.settings.replace(settings);

        let weak = dialog.downgrade();
        imp.tools_group.set_target(move || {
            let dialog = weak.upgrade().ok_or_else(String::new)?;
            let imp = dialog.imp();
            Ok(ToolTarget {
                prefix: imp.prefix_row.text().to_string(),
                proton: resolve_proton_path(&dialog.chosen_proton()).unwrap_or_default(),
            })
        });
        imp.tools_group.show_tools();

        adw::prelude::AdwDialogExt::present(&dialog, Some(parent));
    }

    fn chosen_proton(&self) -> String {
        let imp = self.imp();
        imp.protons.borrow().as_ref().map_or_else(
            || "Default".to_string(),
            |protons| protons.value(imp.proton_row.selected()),
        )
    }

    fn collect_settings(&self) -> GlobalSettings {
        let imp = self.imp();
        GlobalSettings {
            version: GLOBAL_SETTINGS_VERSION,
            default_prefix_path: imp.prefix_row.text().to_string(),
            default_proton: self.chosen_proton(),
            global_mangohud: mangohud_available() && imp.mangohud_row.is_active(),
            global_gamemode: gamemode_available() && imp.gamemode_row.is_active(),
            global_wayland: imp.wayland_row.is_active(),
            global_wow64: imp.wow64_row.is_active(),
            global_ntsync: imp.ntsync_row.is_active(),
            global_hdr: imp.hdr_row.is_active(),
            global_proton_log: imp.proton_log_row.is_active(),
            available_proton_versions: imp.settings.borrow().available_proton_versions.clone(),
            log_errors: imp.log_errors_row.is_active(),
            log_warnings: imp.log_warnings_row.is_active(),
            log_operations: imp.log_operations_row.is_active(),
            use_shared_container: imp.shared_container_row.is_active(),
        }
    }

    async fn reset_runtime(&self) {
        if !daemon::running_games_snapshot().await.is_empty() {
            self.add_toast(adw::Toast::new(&gettext(
                "Cannot reset runtime while games are running. Close all games first.",
            )));
            return;
        }

        let confirm = adw::AlertDialog::new(
            Some(&gettext("Reset umu Runtime?")),
            // One literal: xgettext reads Rust as C, where a line continuation keeps
            // the next line's indentation.
            Some(&gettext(
                "This deletes the Steam Linux Runtime (steamrt3) directory. umu-launcher will re-download a clean copy the next time a dependency is installed.\n\nUse this to fix \"pressure-vessel-wrap\" errors during dependency installations.",
            )),
        );
        confirm.add_responses(&[("cancel", &gettext("Cancel")), ("reset", &gettext("Reset"))]);
        confirm.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
        confirm.set_default_response(Some("cancel"));
        confirm.set_close_response("cancel");
        if confirm.choose_future(Some(self)).await != "reset" {
            return;
        }

        let runtime_dir = get_umu_runtime_dir();
        let result = gio_blocking(move || std::fs::remove_dir_all(&runtime_dir))
            .await
            .unwrap_or_else(|| Err(std::io::Error::other("background task failed")));
        let message = match result {
            Ok(()) => gettext(
                "umu runtime reset. Re-run any dependency install to download a fresh copy.",
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                gettext("umu runtime directory not found — nothing to reset.")
            }
            Err(err) => {
                gettext("Failed to reset umu runtime: {}").replacen("{}", &err.to_string(), 1)
            }
        };
        self.add_toast(adw::Toast::new(&message));
    }
}
