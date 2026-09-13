//! Adding a group, or editing one: its title and the icon, prefix and Proton its
//! games inherit, and, once it exists, the tools for its prefix.

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk4::glib;
use leyen_model::i18n::gettext;
use leyen_model::library::{find_group, replace_group};
use leyen_model::models::{GameGroup, GlobalSettings, GroupLaunchDefaults, LibraryItem};
use leyen_model::runtime::resolve_proton_path;
use libadwaita as adw;

use super::prefix_tools_group::managed_by_preferences;
use super::{
    PrefixSuggestion, PrefixToolsGroup, ProtonChoices, ToolTarget, apply_group_icon,
    choose_file_into, choose_folder_into, image_filter, proton_exists,
};
use crate::daemon::{self, gio_blocking};
use crate::desktop::update_group_desktop_entries_if_present;
use crate::icons::group_icon_file;
use crate::window::LeyenWindow;

mod imp {
    use super::*;

    #[derive(Default, gtk4::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/leyen/ui/group_dialog.ui")]
    pub struct GroupDialog {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub main_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub save_button: TemplateChild<gtk4::Button>,
        #[template_child]
        pub title_row: TemplateChild<adw::EntryRow>,
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
        pub tools_group: TemplateChild<PrefixToolsGroup>,
        pub window: glib::WeakRef<LeyenWindow>,
        /// The group as it was before editing; `None` while adding one.
        pub original: RefCell<Option<GameGroup>>,
        pub settings: RefCell<GlobalSettings>,
        pub protons: RefCell<Option<ProtonChoices>>,
        pub prefix: RefCell<PrefixSuggestion>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GroupDialog {
        const NAME: &'static str = "LeyenGroupDialog";
        type Type = super::GroupDialog;
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

    impl ObjectImpl for GroupDialog {}
    impl WidgetImpl for GroupDialog {}
    impl AdwDialogImpl for GroupDialog {}

    #[gtk4::template_callbacks]
    impl GroupDialog {
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
        fn on_browse_icon(&self, _button: &gtk4::Button) {
            let row = self.icon_row.get();
            glib::spawn_future_local(async move {
                choose_file_into(&row, &gettext("Select Group Icon"), image_filter()).await;
            });
        }

        #[template_callback]
        fn on_browse_prefix(&self, _button: &gtk4::Button) {
            let row = self.prefix_row.get();
            glib::spawn_future_local(async move {
                choose_folder_into(&row, &gettext("Select Prefix Folder")).await;
            });
        }
    }
}

glib::wrapper! {
    pub struct GroupDialog(ObjectSubclass<imp::GroupDialog>)
        @extends adw::Dialog, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::ShortcutManager;
}

impl GroupDialog {
    pub async fn present_add(window: &LeyenWindow) {
        let dialog = Self::build(window, daemon::load_settings().await);
        dialog.set_titles(&gettext("Add Group"), &gettext("Add"));
        dialog.present(Some(window));
    }

    pub async fn present_edit(window: &LeyenWindow, group: GameGroup) {
        let dialog = Self::build(window, daemon::load_settings().await);
        let imp = dialog.imp();
        dialog.set_titles(&gettext("Edit Group"), &gettext("Save"));

        imp.title_row.set_text(&group.title);
        let id = group.id.clone();
        let icon = gio_blocking(move || group_icon_file(&id))
            .await
            .flatten()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        imp.icon_row.set_text(&icon);
        imp.custom_icon_row.set_enable_expansion(!icon.is_empty());
        imp.custom_icon_row.set_expanded(!icon.is_empty());

        imp.prefix.replace(PrefixSuggestion::new(
            &imp.settings.borrow().default_prefix_path,
            &group.defaults.prefix_path,
        ));
        imp.original.replace(Some(group.clone()));
        imp.custom_prefix_row
            .set_enable_expansion(!group.defaults.prefix_path.trim().is_empty());
        let position = imp
            .protons
            .borrow()
            .as_ref()
            .map_or(0, |protons| protons.position(&group.defaults.proton));
        imp.proton_row.set_selected(position);

        imp.tools_group.set_visible(true);
        dialog.update_tools();
        dialog.present(Some(window));
    }

    fn build(window: &LeyenWindow, settings: GlobalSettings) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.window.set(Some(window));

        let protons = ProtonChoices::new(&settings);
        imp.proton_row.set_model(Some(&protons.model));
        imp.protons.replace(Some(protons));
        imp.prefix
            .replace(PrefixSuggestion::new(&settings.default_prefix_path, ""));
        imp.settings.replace(settings);

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

    fn chosen_proton(&self) -> String {
        let imp = self.imp();
        imp.protons.borrow().as_ref().map_or_else(
            || "Default".to_string(),
            |protons| protons.value(imp.proton_row.selected()),
        )
    }

    fn tool_target(&self) -> Result<ToolTarget, String> {
        let imp = self.imp();
        let prefix = imp.prefix_row.text().to_string();
        if prefix.trim().is_empty() {
            return Err(gettext("Custom group prefix path is required"));
        }
        let chosen = self.chosen_proton();
        let proton = if chosen == "Default" {
            imp.settings.borrow().default_proton.clone()
        } else {
            chosen
        };
        Ok(ToolTarget {
            prefix,
            proton: resolve_proton_path(&proton).unwrap_or_default(),
        })
    }

    /// The tools act on the group's prefix, unless some of its games keep their own
    /// or it has none.
    fn update_tools(&self) {
        let imp = self.imp();
        let own_prefixes: Vec<String> = imp
            .original
            .borrow()
            .iter()
            .flat_map(|group| &group.games)
            .filter(|game| !game.prefix_path.trim().is_empty())
            .map(|game| game.title.clone())
            .collect();
        if !own_prefixes.is_empty() {
            imp.tools_group.show_notice(
                &gettext("Group tools unavailable"),
                &gettext("These games use their own prefixes: {}.").replacen(
                    "{}",
                    &own_prefixes.join(", "),
                    1,
                ),
                "dialog-warning-symbolic",
            );
        } else if imp.custom_prefix_row.enables_expansion() {
            imp.tools_group.show_tools();
        } else {
            let (title, subtitle) = managed_by_preferences();
            imp.tools_group
                .show_notice(&title, &subtitle, "dialog-information-symbolic");
        }
    }

    fn toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    async fn save(&self) {
        let imp = self.imp();
        let title = imp.title_row.text().trim().to_string();
        if title.is_empty() {
            self.toast(&gettext("Title is required"));
            return;
        }
        let proton = self.chosen_proton();
        if !proton_exists(&proton).await {
            self.toast(&gettext("Selected Proton path does not exist"));
            return;
        }

        imp.save_button.set_sensitive(false);
        let saved = self.write(title, proton).await;
        imp.save_button.set_sensitive(true);
        if let Some(message) = saved {
            if let Some(window) = imp.window.upgrade() {
                window.refresh_future().await;
                window.toast(&message);
            }
            self.close();
        }
    }

    async fn write(&self, title: String, proton: String) -> Option<String> {
        let imp = self.imp();
        let mut items = match daemon::load_library().await {
            Ok(items) => items,
            Err(err) => {
                self.toast(&err);
                return None;
            }
        };
        let original = imp.original.borrow().clone();
        let group_id = original.as_ref().map_or_else(
            || uuid::Uuid::new_v4().to_string(),
            |group| group.id.clone(),
        );

        let custom_icon = imp
            .custom_icon_row
            .enables_expansion()
            .then(|| imp.icon_row.text().to_string());
        if let Err(err) = apply_group_icon(group_id.clone(), custom_icon).await {
            self.toast(&err);
            return None;
        }

        let defaults = GroupLaunchDefaults {
            prefix_path: if imp.custom_prefix_row.enables_expansion() {
                imp.prefix_row.text().to_string()
            } else {
                String::new()
            },
            proton,
        };
        if original.is_some() {
            if !replace_group(&mut items, &group_id, title, defaults) {
                self.toast(&gettext("Error: Group not found"));
                return None;
            }
        } else {
            items.push(LibraryItem::Group(GameGroup {
                id: group_id.clone(),
                title,
                defaults,
                games: Vec::new(),
            }));
        }

        let updated = find_group(&items, &group_id).cloned();
        if let Err(reason) = daemon::save_library(items).await {
            self.toast(&reason);
            if let Some(window) = imp.window.upgrade() {
                window.refresh();
            }
            return None;
        }
        if original.is_none() {
            return Some(gettext("Item added successfully"));
        }

        // Menu entries carry the group's title.
        let notice = match updated {
            Some(group) => update_group_desktop_entries_if_present(group)
                .await
                .err()
                .map(|err| gettext("Failed to update menu entries: {}").replacen("{}", &err, 1)),
            None => None,
        };
        Some(match notice {
            Some(notice) => gettext("Group updated successfully. {}").replacen("{}", &notice, 1),
            None => gettext("Group updated successfully"),
        })
    }
}
