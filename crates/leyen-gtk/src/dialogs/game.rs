//! Adding a game, or editing one: its executable, prefix, Proton and environment,
//! and, once it exists, the tools for its prefix and its menu entry.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_model::i18n::gettext;
use leyen_model::library::{
    find_game_by_leyen_id, generate_unique_leyen_id, insert_game, replace_game, umu_game_id,
};
use leyen_model::models::{Game, GameGroup, GlobalSettings};
use leyen_model::runtime::resolve_proton_path;
use leyen_model::tools::{gamemode_available, mangohud_available};
use libadwaita as adw;

use super::prefix_tools_group::managed_by_preferences;
use super::{
    PrefixSuggestion, PrefixToolsGroup, ProtonChoices, ToolTarget, apply_game_icon,
    choose_file_into, choose_folder_into, image_filter, proton_exists, windows_programs_filter,
};
use crate::daemon::{self, gio_blocking};
use crate::desktop::{
    create_game_desktop_entry, desktop_entry_exists, remove_game_desktop_entry,
    update_game_desktop_entry_if_present,
};
use crate::icons::{clear_game_icon, game_icon_file};
use crate::window::LeyenWindow;

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/game_dialog.ui")]
    pub struct GameDialog {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub main_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub save_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub title_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub exe_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub group_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub leyen_id_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub game_id_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub custom_icon_row: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub icon_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub custom_prefix_row: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub prefix_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub proton_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub custom_proton_row: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub grouped_proton_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub args_entry: TemplateChild<gtk4::Entry>,
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
        pub tools_group: TemplateChild<PrefixToolsGroup>,
        #[template_child]
        pub menu_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub menu_entry_row: TemplateChild<adw::SwitchRow>,
        pub window: glib::WeakRef<LeyenWindow>,
        /// The game as it was before editing; `None` while adding one.
        pub original: RefCell<Option<Game>>,
        /// The group the game is in or is being added to.
        pub group: RefCell<Option<GameGroup>>,
        pub settings: RefCell<GlobalSettings>,
        pub protons: RefCell<Option<ProtonChoices>>,
        pub leyen_id: RefCell<String>,
        pub prefix: RefCell<PrefixSuggestion>,
        /// The Proton picked before "Custom Proton" was switched off.
        pub stored_proton: Cell<u32>,
        /// Set while the menu entry switch is moved by the dialog, not the user.
        pub syncing_menu_entry: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GameDialog {
        const NAME: &'static str = "LeyenGameDialog";
        type Type = super::GameDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            PrefixToolsGroup::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for GameDialog {}
    impl WidgetImpl for GameDialog {}
    impl AdwDialogImpl for GameDialog {}

    #[gtk4::template_callbacks]
    impl GameDialog {
        #[template_callback]
        fn on_cancel(&self, _button: &gtk4::Button) {
            self.obj().close();
        }

        #[template_callback]
        fn on_save(&self, _button: &gtk4::Button) {
            let obj = self.obj().clone();
            glib::spawn_future_local(async move { obj.save().await });
        }

        #[template_callback]
        fn on_title_changed(&self, row: &adw::EntryRow) {
            if self.custom_prefix_row.enables_expansion() {
                self.prefix
                    .borrow_mut()
                    .title_changed(&self.prefix_row, &row.text());
            }
        }

        #[template_callback]
        fn on_custom_prefix_toggled(&self, _pspec: &glib::ParamSpec, row: &adw::ExpanderRow) {
            let enabled = row.enables_expansion();
            self.prefix
                .borrow_mut()
                .toggled(enabled, &self.prefix_row, &self.title_row.text());
            row.set_expanded(enabled);
            self.obj().update_tools();
        }

        #[template_callback]
        fn on_custom_proton_toggled(&self, _pspec: &glib::ParamSpec, row: &adw::ExpanderRow) {
            if row.enables_expansion() {
                self.grouped_proton_row
                    .set_selected(self.stored_proton.get());
            } else {
                self.stored_proton.set(self.grouped_proton_row.selected());
                self.grouped_proton_row.set_selected(0);
            }
            row.set_expanded(row.enables_expansion());
        }

        #[template_callback]
        fn on_browse_exe(&self, _button: &gtk4::Button) {
            let row = self.exe_row.get();
            glib::spawn_future_local(async move {
                choose_file_into(
                    &row,
                    &gettext("Select Executable"),
                    windows_programs_filter(),
                )
                .await;
            });
        }

        #[template_callback]
        fn on_browse_icon(&self, _button: &gtk4::Button) {
            let row = self.icon_row.get();
            glib::spawn_future_local(async move {
                choose_file_into(&row, &gettext("Select Icon"), image_filter()).await;
            });
        }

        #[template_callback]
        fn on_browse_prefix(&self, _button: &gtk4::Button) {
            let row = self.prefix_row.get();
            glib::spawn_future_local(async move {
                choose_folder_into(&row, &gettext("Select Prefix Folder")).await;
            });
        }

        #[template_callback]
        fn on_menu_entry_toggled(&self, _pspec: &glib::ParamSpec, row: &adw::SwitchRow) {
            if self.syncing_menu_entry.get() {
                return;
            }
            let obj = self.obj().clone();
            let active = row.is_active();
            glib::spawn_future_local(async move { obj.set_menu_entry(active).await });
        }
    }
}

glib::wrapper! {
    pub struct GameDialog(ObjectSubclass<imp::GameDialog>)
        @extends adw::Dialog, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::ShortcutManager;
}

impl GameDialog {
    /// Adds a game to the library, or to the group `group_id` when given.
    pub async fn present_add(window: &LeyenWindow, group_id: Option<String>) {
        let settings = daemon::load_settings().await;
        let library = match daemon::load_library().await {
            Ok(library) => library,
            Err(err) => {
                window.toast(&err);
                return;
            }
        };
        let group = group_id
            .as_deref()
            .and_then(|id| leyen_model::library::find_group(&library, id))
            .cloned();

        let dialog = Self::build(window, settings, group);
        let imp = dialog.imp();
        let title = if imp.group.borrow().is_some() {
            gettext("Add Game to Group")
        } else {
            gettext("Add Game")
        };
        dialog.set_titles(&title, &gettext("Add"));

        let leyen_id = generate_unique_leyen_id(&library);
        dialog.show_ids(&leyen_id);
        let settings = imp.settings.borrow().clone();
        dialog.set_environment(
            settings.global_mangohud,
            settings.global_gamemode,
            settings.global_wayland,
            settings.global_wow64,
            settings.global_ntsync,
            settings.global_hdr,
            settings.global_proton_log,
        );
        dialog.select_proton("Default");
        dialog.present(Some(window));
    }

    pub async fn present_edit(window: &LeyenWindow, game: Game, group: Option<GameGroup>) {
        let settings = daemon::load_settings().await;
        let dialog = Self::build(window, settings, group);
        let imp = dialog.imp();
        dialog.set_titles(&gettext("Edit Game"), &gettext("Save"));

        imp.title_row.set_text(&game.title);
        imp.exe_row.set_text(&game.exe_path);
        imp.args_entry.set_text(&game.launch_args);
        dialog.show_ids(&game.leyen_id);
        dialog.set_environment(
            game.mangohud,
            game.gamemode,
            game.wayland,
            game.wow64,
            game.ntsync,
            game.hdr,
            game.proton_log,
        );

        if game.custom_icon {
            let id = game.id.clone();
            let icon = gio_blocking(move || game_icon_file(&id))
                .await
                .flatten()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            imp.icon_row.set_text(&icon);
        }
        imp.custom_icon_row.set_enable_expansion(game.custom_icon);
        imp.custom_icon_row.set_expanded(game.custom_icon);

        imp.prefix.replace(PrefixSuggestion::new(
            &imp.settings.borrow().default_prefix_path,
            &game.prefix_path,
        ));
        imp.custom_prefix_row
            .set_enable_expansion(!game.prefix_path.trim().is_empty());
        dialog.select_proton(&game.proton);

        imp.original.replace(Some(game.clone()));
        imp.tools_group.set_visible(true);
        dialog.update_tools();

        imp.menu_group.set_visible(true);
        let leyen_id = game.leyen_id.clone();
        let exists = gio_blocking(move || desktop_entry_exists(&leyen_id))
            .await
            .unwrap_or(false);
        imp.syncing_menu_entry.set(true);
        imp.menu_entry_row.set_active(exists);
        imp.syncing_menu_entry.set(false);

        dialog.present(Some(window));
    }

    fn build(window: &LeyenWindow, settings: GlobalSettings, group: Option<GameGroup>) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.window.set(Some(window));

        let protons = ProtonChoices::new(&settings);
        imp.proton_row.set_model(Some(&protons.model));
        imp.grouped_proton_row.set_model(Some(&protons.model));
        imp.protons.replace(Some(protons));

        imp.mangohud_row.set_visible(mangohud_available());
        imp.gamemode_row.set_visible(gamemode_available());

        if let Some(group) = &group {
            imp.group_row.set_subtitle(&group.title);
            imp.group_row.set_visible(true);
            // A grouped game inherits the group's Proton unless told otherwise.
            imp.proton_row.set_visible(false);
            imp.custom_proton_row.set_visible(true);
        }
        imp.prefix
            .replace(PrefixSuggestion::new(&settings.default_prefix_path, ""));
        imp.settings.replace(settings);
        imp.group.replace(group);

        let weak = dialog.downgrade();
        imp.tools_group.set_target(move || {
            weak.upgrade()
                .map(|dialog| dialog.tool_target())
                .unwrap_or_else(|| Err(String::new()))
        });
        dialog
    }

    fn set_titles(&self, title: &str, save_label: &str) {
        let imp = self.imp();
        self.set_title(title);
        imp.main_page.set_title(title);
        imp.save_button.set_label(save_label);
    }

    fn show_ids(&self, leyen_id: &str) {
        let imp = self.imp();
        imp.leyen_id.replace(leyen_id.to_string());
        imp.leyen_id_row.set_subtitle(leyen_id);
        imp.game_id_row.set_subtitle(&umu_game_id(leyen_id));
    }

    #[allow(clippy::too_many_arguments)]
    fn set_environment(
        &self,
        mangohud: bool,
        gamemode: bool,
        wayland: bool,
        wow64: bool,
        ntsync: bool,
        hdr: bool,
        proton_log: bool,
    ) {
        let imp = self.imp();
        imp.mangohud_row.set_active(mangohud);
        imp.gamemode_row.set_active(gamemode);
        imp.wayland_row.set_active(wayland);
        imp.wow64_row.set_active(wow64);
        imp.ntsync_row.set_active(ntsync);
        imp.hdr_row.set_active(hdr);
        imp.proton_log_row.set_active(proton_log);
    }

    /// Selects the game's Proton. A grouped game with its own switches "Custom
    /// Proton" on; one that inherits keeps "Default" behind it.
    fn select_proton(&self, proton: &str) {
        let imp = self.imp();
        let position = imp
            .protons
            .borrow()
            .as_ref()
            .map_or(0, |protons| protons.position(proton));
        if imp.group.borrow().is_some() {
            let custom = !proton.trim().is_empty() && proton != "Default";
            imp.stored_proton.set(position);
            imp.grouped_proton_row
                .set_selected(if custom { position } else { 0 });
            imp.custom_proton_row.set_enable_expansion(custom);
            imp.custom_proton_row.set_expanded(custom);
        } else {
            imp.proton_row.set_selected(position);
        }
    }

    /// The Proton the game will be saved with.
    fn chosen_proton(&self) -> String {
        let imp = self.imp();
        let protons = imp.protons.borrow();
        let Some(protons) = protons.as_ref() else {
            return "Default".to_string();
        };
        if imp.group.borrow().is_some() {
            if imp.custom_proton_row.enables_expansion() {
                protons.value(imp.grouped_proton_row.selected())
            } else {
                "Default".to_string()
            }
        } else {
            protons.value(imp.proton_row.selected())
        }
    }

    /// Where the prefix tools act: the game's own prefix, with its Proton, the
    /// group's, or the default one, in that order.
    fn tool_target(&self) -> Result<ToolTarget, String> {
        let imp = self.imp();
        let prefix = imp.prefix_row.text().to_string();
        if prefix.trim().is_empty() {
            return Err(gettext("Custom game prefix path is required"));
        }
        let chosen = self.chosen_proton();
        let group_proton = imp
            .group
            .borrow()
            .as_ref()
            .map(|group| group.defaults.proton.trim().to_string())
            .filter(|proton| !proton.is_empty() && proton != "Default");
        let proton = if chosen != "Default" {
            chosen
        } else if let Some(group_proton) = group_proton {
            group_proton
        } else {
            imp.settings.borrow().default_proton.clone()
        };
        Ok(ToolTarget {
            prefix,
            proton: resolve_proton_path(&proton).unwrap_or_default(),
        })
    }

    /// The tools act on the game's own prefix; without one they point to where its
    /// prefix is managed.
    fn update_tools(&self) {
        let imp = self.imp();
        if imp.custom_prefix_row.enables_expansion() {
            imp.tools_group.show_tools();
            return;
        }
        let group = imp.group.borrow();
        match group.as_ref() {
            Some(group) if !group.defaults.prefix_path.trim().is_empty() => {
                imp.tools_group.show_notice(
                    &gettext("Managed by group prefix"),
                    &gettext(
                        "Use {} settings to manage dependencies or run a program in that prefix.",
                    )
                    .replacen("{}", &group.title, 1),
                    "dialog-information-symbolic",
                );
            }
            _ => {
                let (title, subtitle) = managed_by_preferences();
                imp.tools_group
                    .show_notice(&title, &subtitle, "dialog-information-symbolic");
            }
        }
    }

    fn toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    async fn set_menu_entry(&self, active: bool) {
        let imp = self.imp();
        let Some(game) = imp.original.borrow().clone() else {
            return;
        };
        let group = imp.group.borrow().clone();
        let result = if active {
            create_game_desktop_entry(game, group)
                .await
                .map(|_| gettext("Added to menu"))
                .map_err(|err| gettext("Failed to create menu entry: {}").replacen("{}", &err, 1))
        } else {
            remove_game_desktop_entry(game.leyen_id.clone())
                .await
                .map(|_| gettext("Removed from menu"))
                .map_err(|err| gettext("Failed to remove menu entry: {}").replacen("{}", &err, 1))
        };
        match result {
            Ok(message) => self.toast(&message),
            Err(message) => {
                self.toast(&message);
                imp.syncing_menu_entry.set(true);
                imp.menu_entry_row.set_active(!active);
                imp.syncing_menu_entry.set(false);
            }
        }
    }

    async fn save(&self) {
        let imp = self.imp();
        let title = imp.title_row.text().trim().to_string();
        let exe = imp.exe_row.text().to_string();
        if title.is_empty() {
            self.toast(&gettext("Title is required"));
            return;
        }
        if exe.trim().is_empty() {
            self.toast(&gettext("Executable path is required"));
            return;
        }
        let proton = self.chosen_proton();
        if !proton_exists(&proton).await {
            self.toast(&gettext("Selected Proton path does not exist"));
            return;
        }

        // Once, however often Save is pressed while this runs.
        imp.save_button.set_sensitive(false);
        let saved = self.write(title, exe, proton).await;
        imp.save_button.set_sensitive(true);
        if let Some(message) = saved {
            if let Some(window) = imp.window.upgrade() {
                window.refresh_future().await;
                window.toast(&message);
            }
            self.close();
        }
    }

    /// Writes the game into the library. Returns the message for the window, or
    /// `None` when it failed and the reason is shown in the dialog.
    async fn write(&self, title: String, exe: String, proton: String) -> Option<String> {
        let imp = self.imp();
        let mut items = match daemon::load_library().await {
            Ok(items) => items,
            Err(err) => {
                self.toast(&err);
                return None;
            }
        };
        let original = imp.original.borrow().clone();
        let group = imp.group.borrow().clone();
        let game_id = original
            .as_ref()
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), |game| game.id.clone());

        let custom_icon = imp.custom_icon_row.enables_expansion();
        let icon_notice = match apply_game_icon(
            game_id.clone(),
            exe.clone(),
            custom_icon.then(|| imp.icon_row.text().to_string()),
        )
        .await
        {
            Ok(notice) => notice,
            Err(err) => {
                self.toast(&err);
                return None;
            }
        };

        let leyen_id = match &original {
            Some(game) => game.leyen_id.clone(),
            // Another client may have taken the id since the dialog opened.
            None if find_game_by_leyen_id(&items, &imp.leyen_id.borrow()).is_some() => {
                generate_unique_leyen_id(&items)
            }
            None => imp.leyen_id.borrow().clone(),
        };
        let game = Game {
            id: game_id.clone(),
            title,
            exe_path: exe,
            prefix_path: if imp.custom_prefix_row.enables_expansion() {
                imp.prefix_row.text().to_string()
            } else {
                String::new()
            },
            proton,
            launch_args: imp.args_entry.text().to_string(),
            mangohud: imp.mangohud_row.is_active(),
            gamemode: imp.gamemode_row.is_active(),
            wayland: imp.wayland_row.is_active(),
            wow64: imp.wow64_row.is_active(),
            ntsync: imp.ntsync_row.is_active(),
            hdr: imp.hdr_row.is_active(),
            proton_log: imp.proton_log_row.is_active(),
            game_id: umu_game_id(&leyen_id),
            leyen_id,
            custom_icon,
            playtime_seconds: original.as_ref().map_or(0, |game| game.playtime_seconds),
            last_played_epoch_seconds: original
                .as_ref()
                .map_or(0, |game| game.last_played_epoch_seconds),
            last_run_duration_seconds: original
                .as_ref()
                .map_or(0, |game| game.last_run_duration_seconds),
            last_run_status: original
                .as_ref()
                .map(|game| game.last_run_status.clone())
                .unwrap_or_default(),
        };

        let mut notices: Vec<String> = icon_notice.into_iter().collect();
        if original.is_some() {
            if !replace_game(&mut items, &game) {
                self.toast(&gettext("Error: Game not found"));
                return None;
            }
        } else {
            let group_id = group.as_ref().map(|group| group.id.as_str());
            if !insert_game(&mut items, group_id, game.clone()) {
                let _ = gio_blocking(move || clear_game_icon(&game_id)).await;
                self.toast(&gettext("Failed to add game to the selected group"));
                return None;
            }
        }

        if let Err(reason) = daemon::save_library(items).await {
            self.toast(&reason);
            if let Some(window) = imp.window.upgrade() {
                window.refresh();
            }
            return None;
        }

        // A new game gets a menu entry; an edited one updates the entry it has.
        let menu_entry = if original.is_some() {
            update_game_desktop_entry_if_present(game, group)
                .await
                .err()
                .map(|err| gettext("Failed to update menu entry: {}").replacen("{}", &err, 1))
        } else {
            create_game_desktop_entry(game, group)
                .await
                .err()
                .map(|err| gettext("Failed to create menu entry: {}").replacen("{}", &err, 1))
        };
        notices.extend(menu_entry);

        let (done, done_with_notice) = if original.is_some() {
            (
                gettext("Game updated successfully"),
                gettext("Game updated successfully. {}"),
            )
        } else {
            (
                gettext("Item added successfully"),
                gettext("Item added successfully. {}"),
            )
        };
        Some(if notices.is_empty() {
            done
        } else {
            done_with_notice.replacen("{}", &notices.join(" "), 1)
        })
    }
}
