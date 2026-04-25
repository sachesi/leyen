use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;

use crate::config::{
    find_group, game_parent_group_id, insert_game, load_library, load_settings,
    normalize_game_id_from_executable, remove_game, remove_group, replace_game, replace_group,
    save_library, suggest_prefix_path,
};
use crate::models::{Game, GameGroup, GroupLaunchDefaults, LibraryItem};
use crate::proton::resolve_proton_path;

use super::deps_dialog::show_dependencies_dialog;
use super::{
    LibraryUi, SECONDARY_WINDOW_DEFAULT_HEIGHT, SECONDARY_WINDOW_DEFAULT_WIDTH,
    refresh_library_view,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AddLibraryItemKind {
    Game,
    Group,
}

fn build_proton_choices(
    settings: &crate::models::GlobalSettings,
) -> (Vec<String>, gtk4::StringList) {
    let names: Vec<String> = settings
        .available_proton_versions
        .iter()
        .map(|path| {
            if path == "Default" {
                "Default".to_string()
            } else {
                PathBuf::from(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone())
            }
        })
        .collect();
    let refs: Vec<&str> = names.iter().map(|name| name.as_str()).collect();
    (
        settings.available_proton_versions.clone(),
        gtk4::StringList::new(&refs),
    )
}

pub fn show_add_library_item_dialog(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    kind: AddLibraryItemKind,
) {
    let settings = load_settings();
    let library = load_library();
    let current_group_id = ui.current_group_id.borrow().clone();
    let inside_group = current_group_id.is_some();
    let current_group = current_group_id
        .as_deref()
        .and_then(|group_id| find_group(&library, group_id))
        .cloned();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .default_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .destroy_with_parent(true)
        .build();

    let title = match (kind, inside_group) {
        (AddLibraryItemKind::Game, true) => "Add Game to Group",
        (AddLibraryItemKind::Game, false) => "Add Game",
        (AddLibraryItemKind::Group, _) => "Add Group",
    };

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(title, ""))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let cancel_btn = gtk4::Button::builder().label("Cancel").build();
    let add_btn = gtk4::Button::builder()
        .label("Add")
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel_btn);
    header.pack_end(&add_btn);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    let page = adw::PreferencesPage::builder().build();

    let title_row = adw::EntryRow::builder().title("Title").build();
    let path_row = adw::EntryRow::builder().title("Executable").build();
    let browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Browse for executable")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    path_row.add_suffix(&browse_btn);

    let inherited_prefix_active = kind == AddLibraryItemKind::Game && inside_group;
    let inherited_proton_active = kind == AddLibraryItemKind::Game && inside_group;
    let initial_prefix = if inherited_prefix_active {
        String::new()
    } else if let Some(group) = current_group.as_ref() {
        if !group.defaults.prefix_path.trim().is_empty() {
            group.defaults.prefix_path.clone()
        } else {
            settings.default_prefix_path.clone()
        }
    } else {
        settings.default_prefix_path.clone()
    };
    let prefix_row = adw::EntryRow::builder()
        .title("Prefix")
        .text(&initial_prefix)
        .build();
    let prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Browse for prefix folder")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let inherit_prefix_row = adw::SwitchRow::builder()
        .title("Inherit Prefix")
        .subtitle("Resolve prefix from the group first, then from global settings")
        .active(inherited_prefix_active)
        .visible(kind == AddLibraryItemKind::Game && inside_group)
        .build();

    let game_id_row = adw::EntryRow::builder().title("ID").build();
    game_id_row.set_editable(false);
    let (available_protons, proton_model) = build_proton_choices(&settings);
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&proton_model)
        .build();
    let inherit_proton_row = adw::SwitchRow::builder()
        .title("Inherit Proton")
        .subtitle("Resolve Proton from the group first, then from global settings")
        .active(inherited_proton_active)
        .visible(kind == AddLibraryItemKind::Game && inside_group)
        .build();

    let args_entry = gtk4::Entry::builder()
        .placeholder_text("%command%")
        .hexpand(true)
        .valign(gtk4::Align::Center)
        .build();
    let args_row = adw::ActionRow::builder()
        .title("Launch Arguments")
        .activatable_widget(&args_entry)
        .build();
    args_row.add_suffix(&args_entry);
    let mangohud_row = adw::SwitchRow::builder()
        .title("Force MangoHud")
        .active(settings.global_mangohud)
        .build();
    let gamemode_row = adw::SwitchRow::builder()
        .title("Force GameMode")
        .active(settings.global_gamemode)
        .build();
    let wayland_row = adw::SwitchRow::builder().title("Wayland").build();
    let wow64_row = adw::SwitchRow::builder().title("WoW64").build();
    let ntsync_row = adw::SwitchRow::builder().title("NTSync").build();

    let game_group = adw::PreferencesGroup::builder().title("Item").build();
    game_group.add(&title_row);

    let context_group = adw::PreferencesGroup::builder().title("Grouping").build();
    if let Some(group) = current_group.as_ref() {
        let group_context_row = adw::ActionRow::builder()
            .title("Adding Into Group")
            .subtitle(&group.title)
            .build();
        context_group.add(&group_context_row);
    }

    let game_details_group = adw::PreferencesGroup::builder()
        .title("Game Settings")
        .build();
    game_details_group.add(&path_row);
    game_details_group.add(&inherit_prefix_row);
    game_details_group.add(&prefix_row);
    game_details_group.add(&inherit_proton_row);
    game_details_group.add(&game_id_row);
    game_details_group.add(&proton_row);
    game_details_group.add(&args_row);
    game_details_group.add(&mangohud_row);
    game_details_group.add(&gamemode_row);
    game_details_group.add(&wayland_row);
    game_details_group.add(&wow64_row);
    game_details_group.add(&ntsync_row);

    let group_prefix_row = adw::EntryRow::builder().title("Prefix").build();
    let group_prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Browse for prefix folder")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    group_prefix_row.add_suffix(&group_prefix_browse_btn);

    let group_proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&proton_model)
        .build();

    let group_defaults_group = adw::PreferencesGroup::builder()
        .title("Group Defaults")
        .description("Leave prefix empty or Proton on Default to keep using global defaults.")
        .build();
    group_defaults_group.add(&group_prefix_row);
    group_defaults_group.add(&group_proton_row);

    page.add(&game_group);
    if inside_group && kind == AddLibraryItemKind::Game {
        page.add(&context_group);
    }
    page.add(&game_details_group);
    page.add(&group_defaults_group);
    game_details_group.set_visible(kind == AddLibraryItemKind::Game);
    group_defaults_group.set_visible(kind == AddLibraryItemKind::Group);

    let proton_inherited_available = inside_group;
    prefix_row.set_sensitive(!inherit_prefix_row.is_active());
    prefix_browse_btn.set_sensitive(!inherit_prefix_row.is_active());
    proton_row.set_sensitive(!inherit_proton_row.is_active());
    if proton_inherited_available {
        proton_row.set_selected(0);
    }

    let prefix_row_clone = prefix_row.clone();
    let prefix_browse_btn_clone = prefix_browse_btn.clone();
    let title_row_for_prefix = title_row.clone();
    let default_prefix_for_inherit = settings.default_prefix_path.clone();
    let group_default_prefix = current_group
        .as_ref()
        .map(|group| group.defaults.prefix_path.clone())
        .unwrap_or_default();
    inherit_prefix_row.connect_active_notify(move |row| {
        let inherited = row.is_active();
        prefix_row_clone.set_sensitive(!inherited);
        prefix_browse_btn_clone.set_sensitive(!inherited);
        if inherited {
            prefix_row_clone.set_text("");
        } else if prefix_row_clone.text().is_empty() {
            let fallback = if !group_default_prefix.trim().is_empty() {
                group_default_prefix.clone()
            } else {
                suggest_prefix_path(&default_prefix_for_inherit, &title_row_for_prefix.text())
            };
            prefix_row_clone.set_text(&fallback);
        }
    });

    let proton_row_clone = proton_row.clone();
    inherit_proton_row.connect_active_notify(move |row| {
        let inherited = row.is_active();
        proton_row_clone.set_sensitive(!inherited);
        if inherited {
            proton_row_clone.set_selected(0);
        }
    });

    let previous_auto_prefix = Rc::new(RefCell::new(initial_prefix.clone()));
    let prefix_row_clone = prefix_row.clone();
    let inherit_prefix_row_clone = inherit_prefix_row.clone();
    let previous_auto_prefix_clone = previous_auto_prefix.clone();
    let default_prefix_path = settings.default_prefix_path.clone();
    title_row.connect_changed(move |row| {
        let title = row.text().to_string();
        if !inherit_prefix_row_clone.is_active() {
            let suggested_prefix = suggest_prefix_path(&default_prefix_path, &title);
            let current_prefix = prefix_row_clone.text().to_string();
            let previous_prefix = previous_auto_prefix_clone.borrow().clone();
            if current_prefix.trim().is_empty()
                || current_prefix == previous_prefix
                || current_prefix == default_prefix_path
            {
                prefix_row_clone.set_text(&suggested_prefix);
            }
            *previous_auto_prefix_clone.borrow_mut() = suggested_prefix;
        }
    });

    let game_id_row_clone = game_id_row.clone();
    path_row.connect_changed(move |row| {
        game_id_row_clone.set_text(&normalize_game_id_from_executable(row.text().as_str()));
    });

    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let path_row_clone = path_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Executable")
            .build();
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                path_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let prefix_row_clone = prefix_row.clone();
    let parent_clone = parent.clone();
    prefix_browse_btn.connect_clicked(move |_| {
        let prefix_row_clone = prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Prefix Folder")
            .build();
        file_dialog.select_folder(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let group_prefix_row_clone = group_prefix_row.clone();
    let parent_clone = parent.clone();
    group_prefix_browse_btn.connect_clicked(move |_| {
        let group_prefix_row_clone = group_prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Prefix Folder")
            .build();
        file_dialog.select_folder(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                group_prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&page)
        .build();
    toolbar_view.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar_view));

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| dialog_clone.destroy());

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let dialog_clone = dialog.clone();
    add_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        if title.trim().is_empty() {
            overlay_clone.add_toast(adw::Toast::new("Title is required"));
            return;
        }

        let mut items = load_library();

        if kind == AddLibraryItemKind::Group {
            items.push(LibraryItem::Group(GameGroup {
                id: uuid::Uuid::new_v4().to_string(),
                title,
                defaults: GroupLaunchDefaults {
                    prefix_path: group_prefix_row.text().to_string(),
                    proton: available_protons
                        .get(group_proton_row.selected() as usize)
                        .cloned()
                        .unwrap_or_else(|| "Default".to_string()),
                },
                games: Vec::new(),
            }));
        } else {
            let exe = path_row.text().to_string();
            if exe.trim().is_empty() {
                overlay_clone.add_toast(adw::Toast::new("Executable path is required"));
                return;
            }
            let normalized_game_id = normalize_game_id_from_executable(&exe);
            let game = Game {
                id: uuid::Uuid::new_v4().to_string(),
                title,
                exe_path: exe,
                prefix_path: if inherit_prefix_row.is_active() {
                    String::new()
                } else {
                    prefix_row.text().to_string()
                },
                proton: if inherit_proton_row.is_active() {
                    "Default".to_string()
                } else {
                    available_protons
                        .get(proton_row.selected() as usize)
                        .cloned()
                        .unwrap_or_else(|| "Default".to_string())
                },
                launch_args: args_entry.text().to_string(),
                force_mangohud: mangohud_row.is_active(),
                force_gamemode: gamemode_row.is_active(),
                game_wayland: wayland_row.is_active(),
                game_wow64: wow64_row.is_active(),
                game_ntsync: ntsync_row.is_active(),
                game_id: normalized_game_id,
                playtime_seconds: 0,
                last_played_epoch_seconds: 0,
                last_run_duration_seconds: 0,
                last_run_status: String::new(),
            };
            let _ = insert_game(&mut items, current_group_id.as_deref(), game);
        }

        save_library(&items);
        if kind == AddLibraryItemKind::Game && inside_group {
            ui_clone.stack.set_visible_child_name("group");
            ui_clone.back_btn.set_visible(true);
        }
        refresh_library_view(&ui_clone, &overlay_clone, &parent_clone);
        overlay_clone.add_toast(adw::Toast::new("Item added successfully"));
        dialog_clone.destroy();
    });

    dialog.present();
}

pub fn show_edit_group_dialog(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    group: &GameGroup,
) {
    let settings = load_settings();
    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .default_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .destroy_with_parent(true)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Edit Group", ""))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let cancel_btn = gtk4::Button::builder().label("Cancel").build();
    let save_btn = gtk4::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel_btn);
    header.pack_end(&save_btn);

    let title_row = adw::EntryRow::builder()
        .title("Title")
        .text(&group.title)
        .build();
    let prefix_row = adw::EntryRow::builder()
        .title("Prefix")
        .text(&group.defaults.prefix_path)
        .build();
    let prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Browse for prefix folder")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let (available_protons, proton_model) = build_proton_choices(&settings);
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&proton_model)
        .build();
    if let Some(pos) = available_protons
        .iter()
        .position(|value| value == &group.defaults.proton)
    {
        proton_row.set_selected(pos as u32);
    } else {
        proton_row.set_selected(0);
    }

    let page = adw::PreferencesPage::builder().build();
    let group_settings = adw::PreferencesGroup::builder().title("Group").build();
    group_settings.add(&title_row);
    let defaults_group = adw::PreferencesGroup::builder()
        .title("Group Defaults")
        .description("Leave prefix empty or Proton on Default to inherit global settings.")
        .build();
    defaults_group.add(&prefix_row);
    defaults_group.add(&proton_row);
    page.add(&group_settings);
    page.add(&defaults_group);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&page));
    dialog.set_content(Some(&toolbar_view));

    let prefix_row_clone = prefix_row.clone();
    let parent_clone = parent.clone();
    prefix_browse_btn.connect_clicked(move |_| {
        let prefix_row_clone = prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Prefix Folder")
            .build();
        file_dialog.select_folder(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| dialog_clone.destroy());

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let group_id = group.id.clone();
    let dialog_clone = dialog.clone();
    save_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        if title.trim().is_empty() {
            overlay_clone.add_toast(adw::Toast::new("Title is required"));
            return;
        }

        let mut items = load_library();
        if replace_group(
            &mut items,
            &group_id,
            title,
            GroupLaunchDefaults {
                prefix_path: prefix_row.text().to_string(),
                proton: available_protons
                    .get(proton_row.selected() as usize)
                    .cloned()
                    .unwrap_or_else(|| "Default".to_string()),
            },
        ) {
            save_library(&items);
            refresh_library_view(&ui_clone, &overlay_clone, &parent_clone);
            overlay_clone.add_toast(adw::Toast::new("Group updated successfully"));
            dialog_clone.destroy();
        }
    });

    dialog.present();
}

pub fn show_edit_game_dialog(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    game: &Game,
) {
    let settings = load_settings();
    let library = load_library();
    let current_parent_group_id = game_parent_group_id(&library, &game.id).flatten();
    let current_parent_group = current_parent_group_id
        .as_deref()
        .and_then(|group_id| find_group(&library, group_id))
        .cloned();
    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .default_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .destroy_with_parent(true)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Edit Game", ""))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let cancel_btn = gtk4::Button::builder().label("Cancel").build();
    let save_btn = gtk4::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel_btn);
    header.pack_end(&save_btn);

    let title_row = adw::EntryRow::builder()
        .title("Title")
        .text(&game.title)
        .build();
    let path_row = adw::EntryRow::builder()
        .title("Executable")
        .text(&game.exe_path)
        .build();
    let browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Browse for executable")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    path_row.add_suffix(&browse_btn);

    let prefix_row = adw::EntryRow::builder().title("Prefix").build();
    let prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Browse for prefix folder")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let inherit_prefix_active =
        current_parent_group.is_some() && game.prefix_path.trim().is_empty();
    prefix_row.set_text(if inherit_prefix_active {
        ""
    } else {
        &game.prefix_path
    });
    let inherit_prefix_row = adw::SwitchRow::builder()
        .title("Inherit Prefix")
        .subtitle("Resolve prefix from the group first, then from global settings")
        .active(inherit_prefix_active)
        .visible(current_parent_group.is_some())
        .build();

    let game_id_row = adw::EntryRow::builder()
        .title("ID")
        .text(&normalize_game_id_from_executable(&game.exe_path))
        .build();
    game_id_row.set_editable(false);

    let (available_protons, proton_model) = build_proton_choices(&settings);
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&proton_model)
        .build();
    let inherit_proton_active = current_parent_group.is_some()
        && (game.proton.trim().is_empty() || game.proton == "Default");
    let selected_proton = if inherit_proton_active {
        "Default"
    } else {
        &game.proton
    };
    if let Some(pos) = available_protons
        .iter()
        .position(|value| value == selected_proton)
    {
        proton_row.set_selected(pos as u32);
    } else {
        proton_row.set_selected(0);
    }
    let inherit_proton_row = adw::SwitchRow::builder()
        .title("Inherit Proton")
        .subtitle("Resolve Proton from the group first, then from global settings")
        .active(inherit_proton_active)
        .visible(current_parent_group.is_some())
        .build();

    let args_entry = gtk4::Entry::builder()
        .text(&game.launch_args)
        .placeholder_text("%command%")
        .hexpand(true)
        .valign(gtk4::Align::Center)
        .build();
    let args_row = adw::ActionRow::builder()
        .title("Launch Arguments")
        .activatable_widget(&args_entry)
        .build();
    args_row.add_suffix(&args_entry);
    let mangohud_row = adw::SwitchRow::builder()
        .title("Force MangoHud")
        .active(game.force_mangohud)
        .build();
    let gamemode_row = adw::SwitchRow::builder()
        .title("Force GameMode")
        .active(game.force_gamemode)
        .build();
    let wayland_row = adw::SwitchRow::builder()
        .title("Wayland")
        .active(game.game_wayland)
        .build();
    let wow64_row = adw::SwitchRow::builder()
        .title("WoW64")
        .active(game.game_wow64)
        .build();
    let ntsync_row = adw::SwitchRow::builder()
        .title("NTSync")
        .active(game.game_ntsync)
        .build();

    let page = adw::PreferencesPage::builder().build();
    let game_group = adw::PreferencesGroup::builder().title("Game").build();
    game_group.add(&title_row);
    game_group.add(&path_row);
    if let Some(group) = current_parent_group.as_ref() {
        let context_group = adw::PreferencesGroup::builder().title("Grouping").build();
        let group_row = adw::ActionRow::builder()
            .title("Group")
            .subtitle(&group.title)
            .build();
        context_group.add(&group_row);
        page.add(&context_group);
    }
    let env_group = adw::PreferencesGroup::builder()
        .title("Environment")
        .build();
    env_group.add(&inherit_prefix_row);
    env_group.add(&prefix_row);
    env_group.add(&inherit_proton_row);
    env_group.add(&game_id_row);
    env_group.add(&proton_row);
    let overrides = adw::PreferencesGroup::builder().title("Overrides").build();
    overrides.add(&args_row);
    overrides.add(&mangohud_row);
    overrides.add(&gamemode_row);
    overrides.add(&wayland_row);
    overrides.add(&wow64_row);
    overrides.add(&ntsync_row);

    let tools = adw::PreferencesGroup::builder().title("Tools").build();
    let deps_btn = gtk4::Button::builder().label("Manage Dependencies").build();
    let deps_prefix = if !game.prefix_path.trim().is_empty() {
        game.prefix_path.clone()
    } else if let Some(group) = current_parent_group.as_ref() {
        if !group.defaults.prefix_path.trim().is_empty() {
            group.defaults.prefix_path.clone()
        } else {
            settings.default_prefix_path.clone()
        }
    } else {
        settings.default_prefix_path.clone()
    };
    let deps_proton_choice = if game.proton.trim().is_empty() || game.proton == "Default" {
        current_parent_group
            .as_ref()
            .map(|group| group.defaults.proton.trim())
            .filter(|value| !value.is_empty() && *value != "Default")
            .unwrap_or(settings.default_proton.as_str())
            .to_string()
    } else {
        game.proton.clone()
    };
    let deps_proton = resolve_proton_path(&deps_proton_choice).unwrap_or_default();
    let overlay_clone_deps = overlay.clone();
    let dialog_parent = parent.clone();
    deps_btn.connect_clicked(move |_| {
        show_dependencies_dialog(
            &dialog_parent,
            &deps_prefix,
            &deps_proton,
            &overlay_clone_deps,
        );
    });
    tools.add(&deps_btn);

    page.add(&game_group);
    page.add(&env_group);
    page.add(&overrides);
    page.add(&tools);

    prefix_row.set_sensitive(!inherit_prefix_row.is_active());
    prefix_browse_btn.set_sensitive(!inherit_prefix_row.is_active());
    proton_row.set_sensitive(!inherit_proton_row.is_active());

    let prefix_row_clone = prefix_row.clone();
    let prefix_browse_btn_clone = prefix_browse_btn.clone();
    let title_row_for_prefix = title_row.clone();
    let default_prefix_for_inherit = settings.default_prefix_path.clone();
    let group_default_prefix = current_parent_group
        .as_ref()
        .map(|group| group.defaults.prefix_path.clone())
        .unwrap_or_default();
    inherit_prefix_row.connect_active_notify(move |row| {
        let inherited = row.is_active();
        prefix_row_clone.set_sensitive(!inherited);
        prefix_browse_btn_clone.set_sensitive(!inherited);
        if inherited {
            prefix_row_clone.set_text("");
        } else if prefix_row_clone.text().is_empty() {
            let fallback = if !group_default_prefix.trim().is_empty() {
                group_default_prefix.clone()
            } else {
                suggest_prefix_path(&default_prefix_for_inherit, &title_row_for_prefix.text())
            };
            prefix_row_clone.set_text(&fallback);
        }
    });

    let proton_row_clone = proton_row.clone();
    inherit_proton_row.connect_active_notify(move |row| {
        let inherited = row.is_active();
        proton_row_clone.set_sensitive(!inherited);
        if inherited {
            proton_row_clone.set_selected(0);
        }
    });

    let previous_auto_prefix = Rc::new(RefCell::new(game.prefix_path.clone()));
    let prefix_row_clone = prefix_row.clone();
    let inherit_prefix_row_clone = inherit_prefix_row.clone();
    let previous_auto_prefix_clone = previous_auto_prefix.clone();
    let default_prefix_path = settings.default_prefix_path.clone();
    title_row.connect_changed(move |row| {
        let title = row.text().to_string();
        if !inherit_prefix_row_clone.is_active() {
            let suggested_prefix = suggest_prefix_path(&default_prefix_path, &title);
            let current_prefix = prefix_row_clone.text().to_string();
            let previous_prefix = previous_auto_prefix_clone.borrow().clone();
            if current_prefix.trim().is_empty()
                || current_prefix == previous_prefix
                || current_prefix == default_prefix_path
            {
                prefix_row_clone.set_text(&suggested_prefix);
            }
            *previous_auto_prefix_clone.borrow_mut() = suggested_prefix;
        }
    });

    let game_id_row_clone = game_id_row.clone();
    path_row.connect_changed(move |row| {
        game_id_row_clone.set_text(&normalize_game_id_from_executable(row.text().as_str()));
    });

    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let path_row_clone = path_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Executable")
            .build();
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                path_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let prefix_row_clone = prefix_row.clone();
    let parent_clone = parent.clone();
    prefix_browse_btn.connect_clicked(move |_| {
        let prefix_row_clone = prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Prefix Folder")
            .build();
        file_dialog.select_folder(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&page)
        .build();
    toolbar_view.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar_view));

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| dialog_clone.destroy());

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let game_id = game.id.clone();
    let original_game = game.clone();
    let dialog_clone = dialog.clone();
    save_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        let exe = path_row.text().to_string();
        if title.trim().is_empty() || exe.trim().is_empty() {
            overlay_clone.add_toast(adw::Toast::new("Title and executable path are required"));
            return;
        }
        let normalized_game_id = normalize_game_id_from_executable(&exe);

        let edited_game = Game {
            id: game_id.clone(),
            title,
            exe_path: exe,
            prefix_path: if inherit_prefix_row.is_active() {
                String::new()
            } else {
                prefix_row.text().to_string()
            },
            proton: if inherit_proton_row.is_active() {
                "Default".to_string()
            } else {
                available_protons
                    .get(proton_row.selected() as usize)
                    .cloned()
                    .unwrap_or_else(|| "Default".to_string())
            },
            launch_args: args_entry.text().to_string(),
            force_mangohud: mangohud_row.is_active(),
            force_gamemode: gamemode_row.is_active(),
            game_wayland: wayland_row.is_active(),
            game_wow64: wow64_row.is_active(),
            game_ntsync: ntsync_row.is_active(),
            game_id: normalized_game_id,
            playtime_seconds: original_game.playtime_seconds,
            last_played_epoch_seconds: original_game.last_played_epoch_seconds,
            last_run_duration_seconds: original_game.last_run_duration_seconds,
            last_run_status: original_game.last_run_status.clone(),
        };

        let mut items = load_library();
        if replace_game(&mut items, &edited_game) {
            save_library(&items);
            refresh_library_view(&ui_clone, &overlay_clone, &parent_clone);
            overlay_clone.add_toast(adw::Toast::new("Game updated successfully"));
            dialog_clone.destroy();
        } else {
            overlay_clone.add_toast(adw::Toast::new("Error: Game not found"));
        }
    });

    dialog.present();
}

pub fn show_delete_confirmation(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    item_id: &str,
) {
    let items = load_library();
    let label = items
        .iter()
        .find_map(|item| match item {
            LibraryItem::Game(game) if game.id == item_id => Some(format!("game '{}'", game.title)),
            LibraryItem::Group(group) if group.id == item_id => {
                Some(format!("group '{}'", group.title))
            }
            _ => None,
        })
        .or_else(|| {
            items.iter().find_map(|item| match item {
                LibraryItem::Group(group) => group
                    .games
                    .iter()
                    .find(|game| game.id == item_id)
                    .map(|game| format!("game '{}'", game.title)),
                LibraryItem::Game(_) => None,
            })
        })
        .unwrap_or_else(|| "item".to_string());

    let dialog = gtk4::AlertDialog::builder()
        .message("Delete Item?")
        .detail(&format!(
            "Are you sure you want to delete {}?\n\nThis action cannot be undone.",
            label
        ))
        .buttons(vec!["Cancel".to_string(), "Delete".to_string()])
        .cancel_button(0)
        .default_button(0)
        .build();

    let item_id = item_id.to_string();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Ok(1) = result {
            let mut items = load_library();
            let deleted = remove_game(&mut items, &item_id)
                .map(|game| game.title)
                .or_else(|| remove_group(&mut items, &item_id));

            if let Some(title) = deleted {
                save_library(&items);
                refresh_library_view(&ui_clone, &overlay_clone, &parent_clone);
                overlay_clone.add_toast(adw::Toast::new(&format!(
                    "'{}' deleted successfully",
                    title
                )));
            }
        }
    });
}
