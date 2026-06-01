use crate::t;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use libadwaita as adw;

use adw::prelude::*;
use gtk4::{gio, glib};

use crate::config::{
    find_game_by_leyen_id, find_group, game_parent_group_id, generate_unique_leyen_id, insert_game,
    load_library, load_settings, remove_game, remove_group, replace_game, replace_group,
    suggest_prefix_path, umu_game_id,
};
use crate::desktop::{
    create_game_desktop_entry, desktop_entry_exists, remove_game_desktop_entry,
    update_game_desktop_entry_if_present, update_group_desktop_entries_if_present,
};
use crate::icons::{
    clear_game_icon, clear_group_icon, extract_game_icon, game_icon_file, group_icon_file,
    save_custom_game_icon, save_custom_group_icon,
};
use crate::models::{Game, GameGroup, GroupLaunchDefaults, LibraryItem};
use crate::prefix_tools::{pick_and_run_in_prefix, run_regedit_in_prefix, run_winecfg_in_prefix};
use crate::runtime::proton::resolve_proton_path;
use crate::tools::{gamemode_available, join_err, mangohud_available};

use super::deps_dialog::open_dependencies_page;
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

fn build_env_row(title: &str, initial_value: bool) -> adw::SwitchRow {
    
    adw::SwitchRow::builder()
        .title(title)
        .active(initial_value)
        .build()
}

fn build_icon_file_filter() -> gtk4::FileFilter {
    let filter = gtk4::FileFilter::new();
    filter.set_name(Some(&t!("Supported images")));
    for suffix in ["png", "jpg", "jpeg", "ico"] {
        filter.add_suffix(suffix);
    }
    filter
}

fn build_icon_file_dialog(title: &str) -> gtk4::FileDialog {
    let filter = build_icon_file_filter();
    gtk4::FileDialog::builder()
        .title(title)
        .default_filter(&filter)
        .build()
}

async fn apply_game_icon(
    game_id: String,
    exe_path: String,
    custom_icon_enabled: bool,
    icon_file: String,
) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        if custom_icon_enabled {
            let icon_file = icon_file.trim();
            if icon_file.is_empty() {
                return Err(t!("Custom icon file is required"));
            }
            save_custom_game_icon(&game_id, icon_file)?;
            Ok(None)
        } else {
            match extract_game_icon(&game_id, &exe_path) {
                Ok(()) => Ok(None),
                Err(_) => {
                    clear_game_icon(&game_id);
                    Ok(Some(t!(
                        "No icon could be extracted from the executable; using the default symbol."
                    )))
                }
            }
        }
    })
    .await
    .map_err(join_err)
    .and_then(|r| r)
}

async fn apply_group_icon(
    group_id: String,
    custom_icon_enabled: bool,
    icon_file: String,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        if custom_icon_enabled {
            let icon_file = icon_file.trim();
            if icon_file.is_empty() {
                return Err(t!("Custom icon file is required"));
            }
            save_custom_group_icon(&group_id, icon_file)
        } else {
            clear_group_icon(&group_id);
            Ok(())
        }
    })
    .await
    .map_err(join_err)
    .and_then(|r| r)
}

fn group_custom_prefix_games(group: &GameGroup) -> Vec<String> {
    group
        .games
        .iter()
        .filter(|game| !game.prefix_path.trim().is_empty())
        .map(|game| game.title.clone())
        .collect()
}

fn group_dependency_prefix(group: &GameGroup) -> Option<String> {
    let prefix = group.defaults.prefix_path.trim();
    if prefix.is_empty() {
        None
    } else {
        Some(prefix.to_string())
    }
}

enum GroupToolState {
    Available,
    MixedCustomPrefixes { titles: Vec<String> },
    ManagedByGlobal,
}

enum GameToolState {
    Available,
    ManagedByGroup { group_title: String },
    ManagedByGlobal,
}

fn group_tool_state(group: &GameGroup) -> GroupToolState {
    let custom_prefix_games = group_custom_prefix_games(group);
    if !custom_prefix_games.is_empty() {
        return GroupToolState::MixedCustomPrefixes {
            titles: custom_prefix_games,
        };
    }

    match group_dependency_prefix(group) {
        Some(_) => GroupToolState::Available,
        None => GroupToolState::ManagedByGlobal,
    }
}

fn game_tool_state(game: &Game, group: Option<&GameGroup>) -> GameToolState {
    if !game.prefix_path.trim().is_empty() {
        return GameToolState::Available;
    }

    if let Some(group) = group
        && !group.defaults.prefix_path.trim().is_empty()
    {
        return GameToolState::ManagedByGroup {
            group_title: group.title.clone(),
        };
    }

    GameToolState::ManagedByGlobal
}

fn build_tools_notice_row(title: &str, subtitle: &str, icon_name: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.add_prefix(&gtk4::Image::from_icon_name(icon_name));
    row
}

fn selected_combo_value(row: &adw::ComboRow, values: &[String]) -> String {
    values
        .get(row.selected() as usize)
        .cloned()
        .unwrap_or_else(|| "Default".to_string())
}

pub async fn show_add_library_item_dialog(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    kind: AddLibraryItemKind,
) {
    let settings = load_settings().await;
    let library = match crate::config::load_library().await {
        Ok(lib) => lib,
        Err(err) => {
            // We should probably show an error toast here, but for now we unwrap
            // and return early to prevent dialog from showing with empty library
            // that might be saved back.
            log::error!("Failed to load library: {}", err);
            return;
        }
    };
    let current_group_id = ui.current_group_id.borrow().clone();
    let inside_group = current_group_id.is_some();
    let current_group = current_group_id
        .as_deref()
        .and_then(|group_id| find_group(&library, group_id))
        .cloned();

    let dialog = adw::Dialog::builder()
        .content_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .content_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .build();
    let nav = adw::NavigationView::new();

    let title = match (kind, inside_group) {
        (AddLibraryItemKind::Game, true) => t!("Add Game to Group"),
        (AddLibraryItemKind::Game, false) => t!("Add Game"),
        (AddLibraryItemKind::Group, _) => t!("Add Group"),
    };

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(&title, ""))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let cancel_btn = gtk4::Button::builder().label(t!("Cancel")).build();
    let add_btn = gtk4::Button::builder()
        .label(t!("Add"))
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel_btn);
    header.pack_end(&add_btn);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    let page = adw::PreferencesPage::builder().build();

    let title_row = adw::EntryRow::builder().title(t!("Title")).build();
    let path_row = adw::EntryRow::builder().title(t!("Executable")).build();
    let browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for executable"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    path_row.add_suffix(&browse_btn);

    let grouped_game = kind == AddLibraryItemKind::Game && inside_group;
    let initial_prefix = String::new();
    let prefix_row = adw::EntryRow::builder()
        .title(t!("Prefix"))
        .text(&initial_prefix)
        .build();
    let prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for prefix folder"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let prefix_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Prefix"))
        .subtitle(t!("Use a per-game prefix instead of the inherited group and global defaults."))
        .show_enable_switch(true)
        .enable_expansion(false)
        .expanded(false)
        .build();
    prefix_override_row.add_row(&prefix_row);

    let generated_leyen_id = generate_unique_leyen_id(&library);
    let leyen_id_row = adw::EntryRow::builder()
        .title(t!("Leyen ID"))
        .text(&generated_leyen_id)
        .build();
    leyen_id_row.set_editable(false);

    let game_id_row = adw::EntryRow::builder()
        .title(t!("Game ID"))
        .text(umu_game_id(&generated_leyen_id))
        .build();
    game_id_row.set_editable(false);
    let (available_protons, proton_model) = build_proton_choices(&settings);
    let proton_row = adw::ComboRow::builder()
        .title(t!("Proton"))
        .model(&proton_model)
        .build();
    let proton_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Proton"))
        .subtitle(t!("Use a per-game Proton version instead of the inherited default."))
        .show_enable_switch(true)
        .enable_expansion(false)
        .expanded(false)
        .build();
    if grouped_game {
        proton_override_row.add_row(&proton_row);
    }

    let game_icon_row = adw::EntryRow::builder().title(t!("Icon File")).build();
    let game_icon_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for custom icon"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    game_icon_row.add_suffix(&game_icon_browse_btn);
    let game_icon_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Icon"))
        .subtitle(t!("Use a custom icon instead of extracting one from the executable."))
        .show_enable_switch(true)
        .enable_expansion(false)
        .expanded(false)
        .build();
    game_icon_override_row.add_row(&game_icon_row);

    let args_entry = gtk4::Entry::builder()
        .placeholder_text("%command%")
        .hexpand(true)
        .valign(gtk4::Align::Center)
        .build();
    let args_row = adw::ActionRow::builder()
        .title(t!("Launch Arguments"))
        .activatable_widget(&args_entry)
        .build();
    args_row.add_suffix(&args_entry);
    let mangohud_row = build_env_row(&t!("MangoHud"), settings.global_mangohud);
    mangohud_row.set_visible(mangohud_available());
    let gamemode_row = build_env_row(&t!("GameMode"), settings.global_gamemode);
    gamemode_row.set_visible(gamemode_available());
    let wayland_row = build_env_row(&t!("Wayland"), settings.global_wayland);
    let wow64_row = build_env_row(&t!("WoW64"), settings.global_wow64);
    let ntsync_row = build_env_row(&t!("NTSync"), settings.global_ntsync);
    let hdr_row = build_env_row(&t!("HDR"), settings.global_hdr);
    let proton_log_row = build_env_row(&t!("Proton Log"), settings.global_proton_log);

    let game_group = adw::PreferencesGroup::builder().title(t!("Item")).build();
    game_group.add(&title_row);

    let context_group = adw::PreferencesGroup::builder().title(t!("Grouping")).build();
    if let Some(group) = current_group.as_ref() {
        let group_context_row = adw::ActionRow::builder()
            .title(t!("Adding Into Group"))
            .subtitle(&group.title)
            .build();
        context_group.add(&group_context_row);
    }

    let game_details_group = adw::PreferencesGroup::builder()
        .title(t!("Game Settings"))
        .build();
    game_details_group.add(&path_row);
    game_details_group.add(&leyen_id_row);
    game_details_group.add(&game_id_row);
    game_details_group.add(&game_icon_override_row);
    game_details_group.add(&prefix_override_row);
    if grouped_game {
        game_details_group.add(&proton_override_row);
    } else {
        game_details_group.add(&proton_row);
    }
    game_details_group.add(&args_row);

    let group_prefix_row = adw::EntryRow::builder().title(t!("Prefix")).build();
    let group_prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for prefix folder"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    group_prefix_row.add_suffix(&group_prefix_browse_btn);

    let group_proton_row = adw::ComboRow::builder()
        .title(t!("Proton"))
        .model(&proton_model)
        .build();
    let group_icon_row = adw::EntryRow::builder().title(t!("Icon File")).build();
    let group_icon_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for group icon"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    group_icon_row.add_suffix(&group_icon_browse_btn);
    let group_icon_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Icon"))
        .subtitle(t!("Set an optional custom icon for this group."))
        .show_enable_switch(true)
        .enable_expansion(false)
        .expanded(false)
        .build();
    group_icon_override_row.add_row(&group_icon_row);
    let group_prefix_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Prefix"))
        .subtitle(t!("Use a group-specific prefix instead of the global default."))
        .show_enable_switch(true)
        .enable_expansion(false)
        .expanded(false)
        .build();
    group_prefix_override_row.add_row(&group_prefix_row);

    let group_defaults_group = adw::PreferencesGroup::builder()
        .title(t!("Group Defaults"))
        .description(t!("Leave prefix empty or Proton on Default to keep using global defaults."))
        .build();
    group_defaults_group.add(&group_icon_override_row);
    group_defaults_group.add(&group_prefix_override_row);
    group_defaults_group.add(&group_proton_row);

    let env_group = adw::PreferencesGroup::builder()
        .title(t!("Environment"))
        .build();
    env_group.add(&mangohud_row);
    env_group.add(&gamemode_row);
    env_group.add(&wayland_row);
    env_group.add(&wow64_row);
    env_group.add(&ntsync_row);
    env_group.add(&hdr_row);
    env_group.add(&proton_log_row);

    page.add(&game_group);
    if inside_group && kind == AddLibraryItemKind::Game {
        page.add(&context_group);
    }
    page.add(&game_details_group);
    page.add(&env_group);
    page.add(&group_defaults_group);

    game_details_group.set_visible(kind == AddLibraryItemKind::Game);
    env_group.set_visible(kind == AddLibraryItemKind::Game);
    group_defaults_group.set_visible(kind == AddLibraryItemKind::Group);

    if grouped_game {
        proton_row.set_selected(0);
    }

    let prefix_row_clone = prefix_row.clone();
    let prefix_override_row_clone = prefix_override_row.clone();
    let title_row_for_prefix = title_row.clone();
    let default_prefix_for_inherit = settings.default_prefix_path.clone();
    let manual_prefix = Rc::new(RefCell::new(initial_prefix.clone()));
    let manual_prefix_clone = manual_prefix.clone();
    prefix_override_row.connect_enable_expansion_notify(move |row| {
        let custom_enabled = row.enables_expansion();
        if custom_enabled {
            let fallback = {
                let stored = manual_prefix_clone.borrow().clone();
                if !stored.trim().is_empty() {
                    stored
                } else {
                    suggest_prefix_path(&default_prefix_for_inherit, &title_row_for_prefix.text())
                }
            };
            prefix_row_clone.set_text(&fallback);
            prefix_override_row_clone.set_expanded(true);
        } else {
            *manual_prefix_clone.borrow_mut() = prefix_row_clone.text().to_string();
            prefix_row_clone.set_text("");
            prefix_override_row_clone.set_expanded(false);
        }
    });

    let group_prefix_row_clone = group_prefix_row.clone();
    let group_prefix_override_row_clone = group_prefix_override_row.clone();
    let title_row_for_group_prefix = title_row.clone();
    let default_group_prefix = settings.default_prefix_path.clone();
    let group_manual_prefix = Rc::new(RefCell::new(String::new()));
    let group_manual_prefix_clone = group_manual_prefix.clone();
    group_prefix_override_row.connect_enable_expansion_notify(move |row| {
        let custom_enabled = row.enables_expansion();
        if custom_enabled {
            let fallback = {
                let stored = group_manual_prefix_clone.borrow().clone();
                if !stored.trim().is_empty() {
                    stored
                } else {
                    suggest_prefix_path(&default_group_prefix, &title_row_for_group_prefix.text())
                }
            };
            group_prefix_row_clone.set_text(&fallback);
            group_prefix_override_row_clone.set_expanded(true);
        } else {
            *group_manual_prefix_clone.borrow_mut() = group_prefix_row_clone.text().to_string();
            group_prefix_row_clone.set_text("");
            group_prefix_override_row_clone.set_expanded(false);
        }
    });

    let proton_row_clone = proton_row.clone();
    let proton_override_row_clone = proton_override_row.clone();
    let manual_proton_selection = Rc::new(RefCell::new(proton_row.selected()));
    let manual_proton_selection_clone = manual_proton_selection.clone();
    proton_override_row.connect_enable_expansion_notify(move |row| {
        let custom_enabled = row.enables_expansion();
        if custom_enabled {
            proton_row_clone.set_selected(*manual_proton_selection_clone.borrow());
            proton_override_row_clone.set_expanded(true);
        } else {
            *manual_proton_selection_clone.borrow_mut() = proton_row_clone.selected();
            proton_row_clone.set_selected(0);
            proton_override_row_clone.set_expanded(false);
        }
    });

    let previous_auto_prefix = Rc::new(RefCell::new(initial_prefix.clone()));
    let prefix_row_clone = prefix_row.clone();
    let prefix_override_row_clone = prefix_override_row.clone();
    let previous_auto_prefix_clone = previous_auto_prefix.clone();
    let default_prefix_path = settings.default_prefix_path.clone();
    title_row.connect_changed(move |row| {
        let title = row.text().to_string();
        if prefix_override_row_clone.enables_expansion() {
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

    let previous_auto_group_prefix = Rc::new(RefCell::new(String::new()));
    let group_prefix_row_clone = group_prefix_row.clone();
    let group_prefix_override_row_clone = group_prefix_override_row.clone();
    let previous_auto_group_prefix_clone = previous_auto_group_prefix.clone();
    let default_group_prefix_path = settings.default_prefix_path.clone();
    title_row.connect_changed(move |row| {
        let title = row.text().to_string();
        if group_prefix_override_row_clone.enables_expansion() {
            let suggested_prefix = suggest_prefix_path(&default_group_prefix_path, &title);
            let current_prefix = group_prefix_row_clone.text().to_string();
            let previous_prefix = previous_auto_group_prefix_clone.borrow().clone();
            if current_prefix.trim().is_empty()
                || current_prefix == previous_prefix
                || current_prefix == default_group_prefix_path
            {
                group_prefix_row_clone.set_text(&suggested_prefix);
            }
            *previous_auto_group_prefix_clone.borrow_mut() = suggested_prefix;
        }
    });

    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let path_row_clone = path_row_clone.clone();
        let filter = gtk4::FileFilter::new();
        filter.set_name(Some(&t!("Windows programs")));
        for suffix in ["exe", "msi", "bat", "cmd", "com"] {
            filter.add_suffix(suffix);
        }
        let file_dialog = gtk4::FileDialog::builder()
            .title(t!("Select Executable"))
            .default_filter(&filter)
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
            .title(t!("Select Prefix Folder"))
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
            .title(t!("Select Prefix Folder"))
            .build();
        file_dialog.select_folder(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                group_prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let game_icon_row_clone = game_icon_row.clone();
    let parent_clone = parent.clone();
    game_icon_browse_btn.connect_clicked(move |_| {
        let game_icon_row_clone = game_icon_row_clone.clone();
        let file_dialog = build_icon_file_dialog(&t!("Select Icon"));
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                game_icon_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let group_icon_row_clone = group_icon_row.clone();
    let parent_clone = parent.clone();
    group_icon_browse_btn.connect_clicked(move |_| {
        let group_icon_row_clone = group_icon_row_clone.clone();
        let file_dialog = build_icon_file_dialog(&t!("Select Group Icon"));
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                group_icon_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&page)
        .build();
    toolbar_view.set_content(Some(&scroll));

    let overlay = adw::ToastOverlay::new();
    let root_page = adw::NavigationPage::builder()
        .child(&toolbar_view)
        .build();
    nav.add(&root_page);
    overlay.set_child(Some(&nav));
    dialog.set_child(Some(&overlay));

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| { dialog_clone.close(); });

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let dialog_clone = dialog.clone();
    let current_group_for_desktop = current_group.clone();
    let generated_leyen_id = generated_leyen_id.clone();
    add_btn.connect_clicked(move |_| {
        let title_row_val = title_row.clone();
        let ui_clone = ui_clone.clone();
        let overlay_clone = overlay_clone.clone();
        let parent_clone = parent_clone.clone();
        let dialog_clone = dialog_clone.clone();
        let current_group_id = current_group_id.clone();
        let current_group_for_desktop = current_group_for_desktop.clone();
        let generated_leyen_id = generated_leyen_id.clone();
        let available_protons = available_protons.clone();
        let path_row_val = path_row.clone();
        let game_icon_override_row_val = game_icon_override_row.clone();
        let game_icon_row_val = game_icon_row.clone();
        let group_icon_override_row_val = group_icon_override_row.clone();
        let group_icon_row_val = group_icon_row.clone();
        let group_prefix_override_row_val = group_prefix_override_row.clone();
        let group_prefix_row_val = group_prefix_row.clone();
        let group_proton_row_val = group_proton_row.clone();
        let prefix_override_row_val = prefix_override_row.clone();
        let prefix_row_val = prefix_row.clone();
        let proton_row_val = proton_row.clone();
        let proton_override_row_val = proton_override_row.clone();
        let args_entry_val = args_entry.clone();
        let mangohud_row_val = mangohud_row.clone();
        let gamemode_row_val = gamemode_row.clone();
        let wayland_row_val = wayland_row.clone();
        let wow64_row_val = wow64_row.clone();
        let ntsync_row_val = ntsync_row.clone();
        let hdr_row_val = hdr_row.clone();
        let proton_log_row_val = proton_log_row.clone();

        glib::spawn_future_local(async move {
            let title = title_row_val.text().to_string();
            if title.trim().is_empty() {
                overlay_clone.add_toast(adw::Toast::new(&t!("Title is required")));
                return;
            }

            let mut items = match crate::config::load_library().await {
                Ok(items) => items,
                Err(err) => {
                    overlay_clone.add_toast(adw::Toast::new(&err));
                    return;
                }
            };
            let mut icon_notice = None;

            if kind == AddLibraryItemKind::Group {
                let proton = available_protons
                    .get(group_proton_row_val.selected() as usize)
                    .cloned()
                    .unwrap_or_else(|| "Default".to_string());

                if proton != "Default" {
                    let p = proton.clone();
                    if !tokio::task::spawn_blocking(move || std::path::Path::new(&p).exists())
                        .await
                        .unwrap_or(true)
                    {
                        overlay_clone.add_toast(adw::Toast::new(&t!("Selected Proton path does not exist")));
                        return;
                    }
                }

                let group_id = uuid::Uuid::new_v4().to_string();
                if let Err(err) = apply_group_icon(
                    group_id.clone(),
                    group_icon_override_row_val.enables_expansion(),
                    group_icon_row_val.text().to_string(),
                )
                .await
                {
                    overlay_clone.add_toast(adw::Toast::new(&err));
                    return;
                }

                items.push(LibraryItem::Group(GameGroup {
                    id: group_id,
                    title,
                    defaults: GroupLaunchDefaults {
                        prefix_path: if group_prefix_override_row_val.enables_expansion() {
                            group_prefix_row_val.text().to_string()
                        } else {
                            String::new()
                        },
                        proton,
                    },
                    games: Vec::new(),
                }));
            } else {
                let proton = if grouped_game && !proton_override_row_val.enables_expansion() {
                    "Default".to_string()
                } else {
                    available_protons
                        .get(proton_row_val.selected() as usize)
                        .cloned()
                        .unwrap_or_else(|| "Default".to_string())
                };

                if proton != "Default" {
                    let p = proton.clone();
                    if !tokio::task::spawn_blocking(move || std::path::Path::new(&p).exists())
                        .await
                        .unwrap_or(true)
                    {
                        overlay_clone.add_toast(adw::Toast::new(&t!("Selected Proton path does not exist")));
                        return;
                    }
                }
                let exe = path_row_val.text().to_string();
                if exe.trim().is_empty() {
                    overlay_clone.add_toast(adw::Toast::new(&t!("Executable path is required")));
                    return;
                }

                let game_id = uuid::Uuid::new_v4().to_string();
                let custom_icon = game_icon_override_row_val.enables_expansion();
                icon_notice = match apply_game_icon(
                    game_id.clone(),
                    exe.clone(),
                    custom_icon,
                    game_icon_row_val.text().to_string(),
                )
                .await
                {
                    Ok(warning) => warning,
                    Err(err) => {
                        overlay_clone.add_toast(adw::Toast::new(&err));
                        return;
                    }
                };

                let leyen_id = if find_game_by_leyen_id(&items, &generated_leyen_id).is_some() {
                    generate_unique_leyen_id(&items)
                } else {
                    generated_leyen_id.clone()
                };
                let umu_game_id = umu_game_id(&leyen_id);
                let game = Game {
                    id: game_id.clone(),
                    title,
                    exe_path: exe,
                    prefix_path: if !prefix_override_row_val.enables_expansion() {
                        String::new()
                    } else {
                        prefix_row_val.text().to_string()
                    },
                    proton,
                    launch_args: args_entry_val.text().to_string(),
                    mangohud: mangohud_row_val.is_active(),
                    gamemode: gamemode_row_val.is_active(),
                    wayland: wayland_row_val.is_active(),
                    wow64: wow64_row_val.is_active(),
                    ntsync: ntsync_row_val.is_active(),
                    hdr: hdr_row_val.is_active(),
                    proton_log: proton_log_row_val.is_active(),
                    leyen_id,
                    game_id: umu_game_id,
                    custom_icon,
                    playtime_seconds: 0,
                    last_played_epoch_seconds: 0,
                    last_run_duration_seconds: 0,
                    last_run_status: String::new(),
                };
                let desktop_game = game.clone();
                if !insert_game(&mut items, current_group_id.as_deref(), game) {
                    let gid = game_id.clone();
                    tokio::task::spawn_blocking(move || clear_game_icon(&gid))
                        .await
                        .ok();
                    overlay_clone
                        .add_toast(adw::Toast::new(&t!("Failed to add game to the selected group")));
                    return;
                }

                if let Err(err) = create_game_desktop_entry(
                    desktop_game.clone(),
                    current_group_for_desktop.clone(),
                )
                .await
                {
                    icon_notice = Some(match icon_notice {
                        Some(existing) => format!("{existing} Failed to create menu entry: {err}"),
                        None => format!("Failed to create menu entry: {err}"),
                    });
                }
            }

            crate::config::save_library(items).await;
            if kind == AddLibraryItemKind::Game && inside_group {
                ui_clone.stack.set_visible_child_name("group");
                ui_clone.back_btn.set_visible(true);
            }
            refresh_library_view(&ui_clone, &overlay_clone, &parent_clone).await;
            let success_message = if let Some(icon_notice) = icon_notice {
                t!("Item added successfully. {}").replacen("{}", &icon_notice, 1)
            } else {
                t!("Item added successfully")
            };
            overlay_clone.add_toast(adw::Toast::new(&success_message));
            dialog_clone.close();
        });
    });

    dialog.present(Some(parent));
}

pub async fn show_edit_group_dialog(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    group: &GameGroup,
) {
    let settings = load_settings().await;
    let dialog = adw::Dialog::builder()
        .content_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .content_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .build();
    let nav = adw::NavigationView::new();

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(&t!("Edit Group"), ""))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let cancel_btn = gtk4::Button::builder().label(t!("Cancel")).build();
    let save_btn = gtk4::Button::builder()
        .label(t!("Save"))
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel_btn);
    header.pack_end(&save_btn);

    let title_row = adw::EntryRow::builder()
        .title(t!("Title"))
        .text(&group.title)
        .build();
    let custom_prefix_active = !group.defaults.prefix_path.trim().is_empty();
    let prefix_row = adw::EntryRow::builder().title(t!("Prefix")).build();
    prefix_row.set_text(if custom_prefix_active {
        &group.defaults.prefix_path
    } else {
        ""
    });
    let prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for prefix folder"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let (available_protons, proton_model) = build_proton_choices(&settings);
    let proton_row = adw::ComboRow::builder()
        .title(t!("Proton"))
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

    let existing_group_icon = group_icon_file(&group.id)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let custom_group_icon_active = !existing_group_icon.is_empty();
    let group_icon_row = adw::EntryRow::builder()
        .title(t!("Icon File"))
        .text(&existing_group_icon)
        .build();
    let group_icon_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for group icon"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    group_icon_row.add_suffix(&group_icon_browse_btn);
    let group_icon_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Icon"))
        .subtitle(t!("Set an optional custom icon for this group."))
        .show_enable_switch(true)
        .enable_expansion(custom_group_icon_active)
        .expanded(custom_group_icon_active)
        .build();
    group_icon_override_row.add_row(&group_icon_row);
    let prefix_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Prefix"))
        .subtitle(t!("Use a group-specific prefix instead of the global default."))
        .show_enable_switch(true)
        .enable_expansion(custom_prefix_active)
        .expanded(custom_prefix_active)
        .build();
    prefix_override_row.add_row(&prefix_row);

    let page = adw::PreferencesPage::builder().build();
    let group_settings = adw::PreferencesGroup::builder().title(t!("Group")).build();
    group_settings.add(&title_row);
    let defaults_group = adw::PreferencesGroup::builder()
        .title(t!("Group Defaults"))
        .description(t!("Leave prefix empty or Proton on Default to inherit global settings."))
        .build();
    defaults_group.add(&group_icon_override_row);
    defaults_group.add(&prefix_override_row);
    defaults_group.add(&proton_row);

    let tools_group = adw::PreferencesGroup::builder().title(t!("Tools")).build();
    let tools_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(180)
        .build();

    let available_tools = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    let winecfg_btn = gtk4::Button::builder()
        .label(t!("Wine Configuration"))
        .build();
    winecfg_btn.set_margin_bottom(6);
    let regedit_btn = gtk4::Button::builder()
        .label(t!("Registry Editor"))
        .build();
    regedit_btn.set_margin_top(6);
    regedit_btn.set_margin_bottom(6);
    let deps_btn = gtk4::Button::builder().label(t!("Manage Dependencies")).build();
    deps_btn.set_margin_top(6);
    deps_btn.set_margin_bottom(6);
    let run_btn = gtk4::Button::builder().label(t!("Run in prefix")).build();
    run_btn.set_margin_top(6);
    available_tools.append(&winecfg_btn);
    available_tools.append(&regedit_btn);
    available_tools.append(&deps_btn);
    available_tools.append(&run_btn);
    tools_stack.add_named(&available_tools, Some("available"));

    let mixed_warning_row =
        build_tools_notice_row(&t!("Group tools unavailable"), "", "dialog-warning-symbolic");
    tools_stack.add_named(&mixed_warning_row, Some("mixed"));

    let global_notice_row = build_tools_notice_row(
        &t!("Managed by global preferences"),
        &t!("Use Global Settings to manage dependencies or run a program in the default prefix."),
        "dialog-information-symbolic",
    );
    tools_stack.add_named(&global_notice_row, Some("global"));
    tools_group.add(&tools_stack);

    let nav_for_deps = nav.clone();
    let overlay_clone_deps = overlay.clone();
    let overlay_clone_winecfg = overlay.clone();
    let overlay_clone_regedit = overlay.clone();
    let prefix_row_for_winecfg = prefix_row.clone();
    let prefix_row_for_regedit = prefix_row.clone();
    let prefix_row_for_deps = prefix_row.clone();
    let proton_row_for_winecfg = proton_row.clone();
    let proton_row_for_regedit = proton_row.clone();
    let proton_row_for_deps = proton_row.clone();
    let available_protons_for_winecfg = available_protons.clone();
    let available_protons_for_regedit = available_protons.clone();
    let available_protons_for_deps = available_protons.clone();
    let settings_default_proton_for_winecfg = settings.default_proton.clone();
    let settings_default_proton_for_regedit = settings.default_proton.clone();
    let settings_default_proton_for_deps = settings.default_proton.clone();

    winecfg_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_winecfg.text().to_string();
        if prefix.trim().is_empty() {
            overlay_clone_winecfg
                .add_toast(adw::Toast::new(&t!("Custom group prefix path is required")));
            return;
        }
        let proton_choice =
            selected_combo_value(&proton_row_for_winecfg, &available_protons_for_winecfg);
        let resolved = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_winecfg.clone()
        } else {
            proton_choice
        };
        let proton = resolve_proton_path(&resolved).unwrap_or_default();
        let o = overlay_clone_winecfg.clone();
        glib::spawn_future_local(async move {
            run_winecfg_in_prefix(&o, &prefix, &proton).await;
        });
    });

    regedit_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_regedit.text().to_string();
        if prefix.trim().is_empty() {
            overlay_clone_regedit
                .add_toast(adw::Toast::new(&t!("Custom group prefix path is required")));
            return;
        }
        let proton_choice =
            selected_combo_value(&proton_row_for_regedit, &available_protons_for_regedit);
        let resolved = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_regedit.clone()
        } else {
            proton_choice
        };
        let proton = resolve_proton_path(&resolved).unwrap_or_default();
        let o = overlay_clone_regedit.clone();
        glib::spawn_future_local(async move {
            run_regedit_in_prefix(&o, &prefix, &proton).await;
        });
    });

    deps_btn.connect_clicked(move |_| {
        let deps_prefix = prefix_row_for_deps.text().to_string();
        if deps_prefix.trim().is_empty() {
            overlay_clone_deps.add_toast(adw::Toast::new(&t!("Custom group prefix path is required")));
            return;
        }
        let proton_choice = selected_combo_value(&proton_row_for_deps, &available_protons_for_deps);
        let resolved_choice = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_deps.clone()
        } else {
            proton_choice
        };
        let deps_proton = resolve_proton_path(&resolved_choice).unwrap_or_default();
        let nav = nav_for_deps.clone();
        let o = overlay_clone_deps.clone();
        glib::spawn_future_local(async move {
            open_dependencies_page(&nav, deps_prefix.as_str(), &deps_proton, &o).await;
        });
    });

    let dialog_parent = parent.clone();
    let overlay_clone_run = overlay.clone();
    let prefix_row_for_run = prefix_row.clone();
    let proton_row_for_run = proton_row.clone();
    let available_protons_for_run = available_protons.clone();
    let settings_default_proton_for_run = settings.default_proton.clone();
    run_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_run.text().to_string();
        if prefix.trim().is_empty() {
            overlay_clone_run.add_toast(adw::Toast::new(&t!("Custom group prefix path is required")));
            return;
        }
        let proton_choice = selected_combo_value(&proton_row_for_run, &available_protons_for_run);
        let resolved_choice = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_run.clone()
        } else {
            proton_choice
        };
        let proton = resolve_proton_path(&resolved_choice).unwrap_or_default();
        let p = dialog_parent.clone();
        let o = overlay_clone_run.clone();
        glib::spawn_future_local(async move {
            pick_and_run_in_prefix(&p, &o, &prefix, &proton).await;
        });
    });

    let custom_prefix_games = match group_tool_state(group) {
        GroupToolState::MixedCustomPrefixes { titles } => titles,
        _ => Vec::new(),
    };
    let tools_stack_clone = tools_stack.clone();
    if !custom_prefix_games.is_empty() {
        mixed_warning_row.set_subtitle(
            &t!("These games use their own prefixes: {}.")
                .replacen("{}", &custom_prefix_games.join(", "), 1),
        );
        tools_stack_clone.set_visible_child_name("mixed");
    } else if prefix_override_row.enables_expansion() {
        tools_stack_clone.set_visible_child_name("available");
    } else {
        tools_stack_clone.set_visible_child_name("global");
    }
    let tools_stack_clone = tools_stack.clone();
    let mixed_warning_row_clone = mixed_warning_row.clone();
    let custom_prefix_games_clone = custom_prefix_games.clone();
    prefix_override_row.connect_enable_expansion_notify(move |row| {
        if !custom_prefix_games_clone.is_empty() {
            mixed_warning_row_clone.set_subtitle(
                &t!("These games use their own prefixes: {}.")
                    .replacen("{}", &custom_prefix_games_clone.join(", "), 1),
            );
            tools_stack_clone.set_visible_child_name("mixed");
        } else if row.enables_expansion() {
            tools_stack_clone.set_visible_child_name("available");
        } else {
            tools_stack_clone.set_visible_child_name("global");
        }
    });
    page.add(&group_settings);
    page.add(&defaults_group);
    page.add(&tools_group);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&page));

    let overlay = adw::ToastOverlay::new();
    let root_page = adw::NavigationPage::builder()
        .child(&toolbar_view)
        .build();
    nav.add(&root_page);
    overlay.set_child(Some(&nav));
    dialog.set_child(Some(&overlay));

    let prefix_row_clone = prefix_row.clone();
    let prefix_override_row_clone = prefix_override_row.clone();
    let title_row_for_prefix = title_row.clone();
    let default_prefix_for_inherit = settings.default_prefix_path.clone();
    let manual_prefix = Rc::new(RefCell::new(group.defaults.prefix_path.clone()));
    let manual_prefix_clone = manual_prefix.clone();
    prefix_override_row.connect_enable_expansion_notify(move |row| {
        let custom_enabled = row.enables_expansion();
        if custom_enabled {
            let fallback = {
                let stored = manual_prefix_clone.borrow().clone();
                if !stored.trim().is_empty() {
                    stored
                } else {
                    suggest_prefix_path(&default_prefix_for_inherit, &title_row_for_prefix.text())
                }
            };
            prefix_row_clone.set_text(&fallback);
            prefix_override_row_clone.set_expanded(true);
        } else {
            *manual_prefix_clone.borrow_mut() = prefix_row_clone.text().to_string();
            prefix_row_clone.set_text("");
            prefix_override_row_clone.set_expanded(false);
        }
    });

    let previous_auto_prefix = Rc::new(RefCell::new(group.defaults.prefix_path.clone()));
    let prefix_row_clone = prefix_row.clone();
    let prefix_override_row_clone = prefix_override_row.clone();
    let previous_auto_prefix_clone = previous_auto_prefix.clone();
    let default_prefix_path = settings.default_prefix_path.clone();
    title_row.connect_changed(move |row| {
        let title = row.text().to_string();
        if prefix_override_row_clone.enables_expansion() {
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

    let prefix_row_clone = prefix_row.clone();
    let parent_clone = parent.clone();
    prefix_browse_btn.connect_clicked(move |_| {
        let prefix_row_clone = prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title(t!("Select Prefix Folder"))
            .build();
        file_dialog.select_folder(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let group_icon_row_clone = group_icon_row.clone();
    let parent_clone = parent.clone();
    group_icon_browse_btn.connect_clicked(move |_| {
        let group_icon_row_clone = group_icon_row_clone.clone();
        let file_dialog = build_icon_file_dialog(&t!("Select Group Icon"));
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                group_icon_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| { dialog_clone.close(); });

    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let group_id = group.id.clone();
    let dialog_clone = dialog.clone();
    save_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        if title.trim().is_empty() {
            overlay_clone.add_toast(adw::Toast::new(&t!("Title is required")));
            return;
        }

        let ui_clone = ui_clone.clone();
        let overlay_clone = overlay_clone.clone();
        let parent_clone = parent_clone.clone();
        let group_id = group_id.clone();
        let dialog_clone = dialog_clone.clone();
        let prefix_override_row_val = prefix_override_row.clone();
        let prefix_row_val = prefix_row.clone();
        let proton_row_val = proton_row.clone();
        let available_protons = available_protons.clone();
        let group_icon_override_row_val = group_icon_override_row.clone();
        let group_icon_row_val = group_icon_row.clone();
        let title_row_val = title_row.clone();

        glib::spawn_future_local(async move {
            let title = title_row_val.text().to_string();
            let proton = available_protons
                .get(proton_row_val.selected() as usize)
                .cloned()
                .unwrap_or_else(|| "Default".to_string());

            if proton != "Default" {
                let p = proton.clone();
                if !tokio::task::spawn_blocking(move || std::path::Path::new(&p).exists())
                    .await
                    .unwrap_or(true)
                {
                    overlay_clone.add_toast(adw::Toast::new(&t!("Selected Proton path does not exist")));
                    return;
                }
            }

            let mut items = match crate::config::load_library().await {
                Ok(items) => items,
                Err(err) => {
                    overlay_clone.add_toast(adw::Toast::new(&err));
                    return;
                }
            };
            if let Err(err) = apply_group_icon(
                group_id.clone(),
                group_icon_override_row_val.enables_expansion(),
                group_icon_row_val.text().to_string(),
            )
            .await
            {
                overlay_clone.add_toast(adw::Toast::new(&err));
                return;
            }

            if replace_group(
                &mut items,
                &group_id,
                title,
                GroupLaunchDefaults {
                    prefix_path: if prefix_override_row_val.enables_expansion() {
                        prefix_row_val.text().to_string()
                    } else {
                        String::new()
                    },
                    proton,
                },
            ) {
                crate::config::save_library(items.clone()).await;
                let updated_group = find_group(&items, &group_id).cloned();
                let desktop_notice = if let Some(group) = updated_group {
                    update_group_desktop_entries_if_present(group)
                        .await
                        .err()
                        .map(|err| format!("Failed to update menu entries: {err}"))
                } else {
                    None
                };
                refresh_library_view(&ui_clone, &overlay_clone, &parent_clone).await;
                let success_message = if let Some(desktop_notice) = desktop_notice {
                    t!("Group updated successfully. {}").replacen("{}", &desktop_notice, 1)
                } else {
                    t!("Group updated successfully")
                };
                overlay_clone.add_toast(adw::Toast::new(&success_message));
                dialog_clone.close();
            }
        });
    });

    dialog.present(Some(parent));
}

pub async fn show_edit_game_dialog(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    game: &Game,
) {
    let settings = load_settings().await;
    let library = match load_library().await {
        Ok(lib) => lib,
        Err(err) => {
            log::error!("Failed to load library for edit: {}", err);
            overlay.add_toast(adw::Toast::new(&err));
            return;
        }
    };
    let current_parent_group_id = game_parent_group_id(&library, &game.id);
    let current_parent_group = current_parent_group_id
        .as_deref()
        .and_then(|group_id| find_group(&library, group_id))
        .cloned();
    let dialog = adw::Dialog::builder()
        .content_width(SECONDARY_WINDOW_DEFAULT_WIDTH)
        .content_height(SECONDARY_WINDOW_DEFAULT_HEIGHT)
        .build();
    let nav = adw::NavigationView::new();

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(&t!("Edit Game"), ""))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    let cancel_btn = gtk4::Button::builder().label(t!("Cancel")).build();
    let save_btn = gtk4::Button::builder()
        .label(t!("Save"))
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel_btn);
    header.pack_end(&save_btn);

    let title_row = adw::EntryRow::builder()
        .title(t!("Title"))
        .text(&game.title)
        .build();
    let path_row = adw::EntryRow::builder()
        .title(t!("Executable"))
        .text(&game.exe_path)
        .build();
    let browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for executable"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    path_row.add_suffix(&browse_btn);

    let prefix_row = adw::EntryRow::builder().title(t!("Prefix")).build();
    let prefix_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for prefix folder"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let grouped_game = current_parent_group.is_some();
    let custom_prefix_active = !game.prefix_path.trim().is_empty();
    prefix_row.set_text(if !custom_prefix_active {
        ""
    } else {
        &game.prefix_path
    });
    let prefix_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Prefix"))
        .subtitle(t!("Use a per-game prefix instead of the inherited group and global defaults."))
        .show_enable_switch(true)
        .enable_expansion(custom_prefix_active)
        .expanded(custom_prefix_active)
        .build();
    prefix_override_row.add_row(&prefix_row);

    let leyen_id_row = adw::EntryRow::builder()
        .title(t!("Leyen ID"))
        .text(&game.leyen_id)
        .build();
    leyen_id_row.set_editable(false);

    let game_id_row = adw::EntryRow::builder()
        .title(t!("Game ID"))
        .text(umu_game_id(&game.leyen_id))
        .build();
    game_id_row.set_editable(false);

    let (available_protons, proton_model) = build_proton_choices(&settings);
    let proton_row = adw::ComboRow::builder()
        .title(t!("Proton"))
        .model(&proton_model)
        .build();
    let custom_proton_active =
        grouped_game && !game.proton.trim().is_empty() && game.proton != "Default";
    let selected_proton = if grouped_game {
        if custom_proton_active {
            &game.proton
        } else {
            "Default"
        }
    } else if game.proton.trim().is_empty() {
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
    let proton_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Proton"))
        .subtitle(t!("Use a per-game Proton version instead of the inherited default."))
        .show_enable_switch(true)
        .enable_expansion(custom_proton_active)
        .expanded(custom_proton_active)
        .build();
    if grouped_game {
        proton_override_row.add_row(&proton_row);
    }

    let existing_custom_game_icon = if game.custom_icon {
        game_icon_file(&game.id)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let game_icon_row = adw::EntryRow::builder()
        .title(t!("Icon File"))
        .text(&existing_custom_game_icon)
        .build();
    let game_icon_browse_btn = gtk4::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text(t!("Browse for custom icon"))
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    game_icon_row.add_suffix(&game_icon_browse_btn);
    let game_icon_override_row = adw::ExpanderRow::builder()
        .title(t!("Custom Icon"))
        .subtitle(t!("Use a custom icon instead of extracting one from the executable."))
        .show_enable_switch(true)
        .enable_expansion(game.custom_icon)
        .expanded(game.custom_icon)
        .build();
    game_icon_override_row.add_row(&game_icon_row);

    let args_entry = gtk4::Entry::builder()
        .text(&game.launch_args)
        .placeholder_text("%command%")
        .hexpand(true)
        .valign(gtk4::Align::Center)
        .build();
    let args_row = adw::ActionRow::builder()
        .title(t!("Launch Arguments"))
        .activatable_widget(&args_entry)
        .build();
    args_row.add_suffix(&args_entry);
    let mangohud_row = build_env_row(&t!("MangoHud"), game.mangohud);
    mangohud_row.set_visible(mangohud_available());
    let gamemode_row = build_env_row(&t!("GameMode"), game.gamemode);
    gamemode_row.set_visible(gamemode_available());
    let wayland_row = build_env_row(&t!("Wayland"), game.wayland);
    let wow64_row = build_env_row(&t!("WoW64"), game.wow64);
    let ntsync_row = build_env_row(&t!("NTSync"), game.ntsync);
    let hdr_row = build_env_row(&t!("HDR"), game.hdr);
    let proton_log_row = build_env_row(&t!("Proton Log"), game.proton_log);

    let page = adw::PreferencesPage::builder().build();
    let game_group = adw::PreferencesGroup::builder().title(t!("Game")).build();
    game_group.add(&title_row);
    game_group.add(&path_row);
    if let Some(group) = current_parent_group.as_ref() {
        let context_group = adw::PreferencesGroup::builder().title(t!("Grouping")).build();
        let group_row = adw::ActionRow::builder()
            .title(t!("Group"))
            .subtitle(&group.title)
            .build();
        context_group.add(&group_row);
        page.add(&context_group);
    }
    let settings_group = adw::PreferencesGroup::builder().title(t!("Settings")).build();
    settings_group.add(&leyen_id_row);
    settings_group.add(&game_id_row);
    settings_group.add(&game_icon_override_row);
    settings_group.add(&prefix_override_row);
    if grouped_game {
        settings_group.add(&proton_override_row);
    } else {
        settings_group.add(&proton_row);
    }
    settings_group.add(&args_row);
    let env_group = adw::PreferencesGroup::builder()
        .title(t!("Environment"))
        .build();
    env_group.add(&mangohud_row);
    env_group.add(&gamemode_row);
    env_group.add(&wayland_row);
    env_group.add(&wow64_row);
    env_group.add(&ntsync_row);
    env_group.add(&hdr_row);
    env_group.add(&proton_log_row);

    let tools = adw::PreferencesGroup::builder().title(t!("Tools")).build();
    let tools_stack = gtk4::Stack::builder()
        .transition_type(gtk4::StackTransitionType::Crossfade)
        .transition_duration(180)
        .build();

    let lid = game.leyen_id.clone();
    let exists = tokio::task::spawn_blocking(move || desktop_entry_exists(&lid))
        .await
        .unwrap_or_else(|e| {
            log::warn!("desktop_entry_exists task failed: {}", join_err(e));
            false
        });
    let menu_btn = gtk4::Button::builder()
        .label(if exists {
            t!("Remove from menu")
        } else {
            t!("Add to menu")
        })
        .build();
    menu_btn.set_margin_top(6);
    let available_tools = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    let winecfg_btn = gtk4::Button::builder()
        .label(t!("Wine Configuration"))
        .build();
    winecfg_btn.set_margin_bottom(6);
    let regedit_btn = gtk4::Button::builder()
        .label(t!("Registry Editor"))
        .build();
    regedit_btn.set_margin_top(6);
    regedit_btn.set_margin_bottom(6);
    let deps_btn = gtk4::Button::builder().label(t!("Manage Dependencies")).build();
    deps_btn.set_margin_top(6);
    deps_btn.set_margin_bottom(6);
    let run_btn = gtk4::Button::builder().label(t!("Run in prefix")).build();
    run_btn.set_margin_top(6);
    available_tools.append(&winecfg_btn);
    available_tools.append(&regedit_btn);
    available_tools.append(&deps_btn);
    available_tools.append(&run_btn);
    tools_stack.add_named(&available_tools, Some("available"));

    let group_notice_row =
        build_tools_notice_row(&t!("Managed by group prefix"), "", "dialog-information-symbolic");
    tools_stack.add_named(&group_notice_row, Some("group"));

    let global_notice_row = build_tools_notice_row(
        &t!("Managed by global preferences"),
        &t!("Use Global Settings to manage dependencies or run a program in the default prefix."),
        "dialog-information-symbolic",
    );
    tools_stack.add_named(&global_notice_row, Some("global"));
    tools.add(&tools_stack);

    let nav_for_deps = nav.clone();
    let overlay_clone_deps = overlay.clone();
    let overlay_clone_winecfg = overlay.clone();
    let overlay_clone_regedit = overlay.clone();
    let prefix_row_for_winecfg = prefix_row.clone();
    let prefix_row_for_regedit = prefix_row.clone();
    let prefix_row_for_deps = prefix_row.clone();
    let proton_row_for_winecfg = proton_row.clone();
    let proton_row_for_regedit = proton_row.clone();
    let proton_row_for_deps = proton_row.clone();
    let proton_override_row_for_winecfg = proton_override_row.clone();
    let proton_override_row_for_regedit = proton_override_row.clone();
    let proton_override_row_for_deps = proton_override_row.clone();
    let available_protons_for_winecfg = available_protons.clone();
    let available_protons_for_regedit = available_protons.clone();
    let available_protons_for_deps = available_protons.clone();
    let current_parent_group_for_winecfg = current_parent_group.clone();
    let current_parent_group_for_regedit = current_parent_group.clone();
    let current_parent_group_for_deps = current_parent_group.clone();
    let settings_default_proton_for_winecfg = settings.default_proton.clone();
    let settings_default_proton_for_regedit = settings.default_proton.clone();
    let settings_default_proton_for_deps = settings.default_proton.clone();

    winecfg_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_winecfg.text().to_string();
        if prefix.trim().is_empty() {
            overlay_clone_winecfg
                .add_toast(adw::Toast::new(&t!("Custom game prefix path is required")));
            return;
        }
        let proton_choice = if grouped_game && !proton_override_row_for_winecfg.enables_expansion()
        {
            current_parent_group_for_winecfg
                .as_ref()
                .map(|group| group.defaults.proton.trim())
                .filter(|value| !value.is_empty() && *value != "Default")
                .map(|v| v.to_string())
                .unwrap_or_else(|| {
                    selected_combo_value(
                        &proton_row_for_winecfg,
                        &available_protons_for_winecfg,
                    )
                })
        } else {
            selected_combo_value(&proton_row_for_winecfg, &available_protons_for_winecfg)
        };
        let resolved = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_winecfg.clone()
        } else {
            proton_choice
        };
        let proton = resolve_proton_path(&resolved).unwrap_or_default();
        let o = overlay_clone_winecfg.clone();
        glib::spawn_future_local(async move {
            run_winecfg_in_prefix(&o, &prefix, &proton).await;
        });
    });

    regedit_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_regedit.text().to_string();
        if prefix.trim().is_empty() {
            overlay_clone_regedit
                .add_toast(adw::Toast::new(&t!("Custom game prefix path is required")));
            return;
        }
        let proton_choice = if grouped_game && !proton_override_row_for_regedit.enables_expansion()
        {
            current_parent_group_for_regedit
                .as_ref()
                .map(|group| group.defaults.proton.trim())
                .filter(|value| !value.is_empty() && *value != "Default")
                .map(|v| v.to_string())
                .unwrap_or_else(|| {
                    selected_combo_value(
                        &proton_row_for_regedit,
                        &available_protons_for_regedit,
                    )
                })
        } else {
            selected_combo_value(&proton_row_for_regedit, &available_protons_for_regedit)
        };
        let resolved = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_regedit.clone()
        } else {
            proton_choice
        };
        let proton = resolve_proton_path(&resolved).unwrap_or_default();
        let o = overlay_clone_regedit.clone();
        glib::spawn_future_local(async move {
            run_regedit_in_prefix(&o, &prefix, &proton).await;
        });
    });

    deps_btn.connect_clicked(move |_| {
        let deps_prefix = prefix_row_for_deps.text().to_string();
        if deps_prefix.trim().is_empty() {
            overlay_clone_deps.add_toast(adw::Toast::new(&t!("Custom game prefix path is required")));
            return;
        }
        let proton_choice = if grouped_game && !proton_override_row_for_deps.enables_expansion() {
            current_parent_group_for_deps
                .as_ref()
                .map(|group| group.defaults.proton.trim())
                .filter(|value| !value.is_empty() && *value != "Default")
                .unwrap_or(settings_default_proton_for_deps.as_str())
                .to_string()
        } else {
            selected_combo_value(&proton_row_for_deps, &available_protons_for_deps)
        };
        let resolved_choice = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_deps.clone()
        } else {
            proton_choice
        };
        let deps_proton = resolve_proton_path(&resolved_choice).unwrap_or_default();
        let nav = nav_for_deps.clone();
        let o = overlay_clone_deps.clone();
        glib::spawn_future_local(async move {
            open_dependencies_page(&nav, deps_prefix.as_str(), &deps_proton, &o).await;
        });
    });

    let dialog_parent = parent.clone();
    let overlay_clone_run = overlay.clone();
    let prefix_row_for_run = prefix_row.clone();
    let proton_row_for_run = proton_row.clone();
    let proton_override_row_for_run = proton_override_row.clone();
    let available_protons_for_run = available_protons.clone();
    let current_parent_group_for_run = current_parent_group.clone();
    let settings_default_proton_for_run = settings.default_proton.clone();
    run_btn.connect_clicked(move |_| {
        let prefix = prefix_row_for_run.text().to_string();
        if prefix.trim().is_empty() {
            overlay_clone_run.add_toast(adw::Toast::new(&t!("Custom game prefix path is required")));
            return;
        }
        let proton_choice = if grouped_game && !proton_override_row_for_run.enables_expansion() {
            current_parent_group_for_run
                .as_ref()
                .map(|group| group.defaults.proton.trim())
                .filter(|value| !value.is_empty() && *value != "Default")
                .unwrap_or(settings_default_proton_for_run.as_str())
                .to_string()
        } else {
            selected_combo_value(&proton_row_for_run, &available_protons_for_run)
        };
        let resolved_choice = if proton_choice.trim().is_empty() || proton_choice == "Default" {
            settings_default_proton_for_run.clone()
        } else {
            proton_choice
        };
        let proton = resolve_proton_path(&resolved_choice).unwrap_or_default();
        let p = dialog_parent.clone();
        let o = overlay_clone_run.clone();
        glib::spawn_future_local(async move {
            pick_and_run_in_prefix(&p, &o, &prefix, &proton).await;
        });
    });

    match game_tool_state(game, current_parent_group.as_ref()) {
        GameToolState::Available => {
            tools_stack.set_visible_child_name("available");
        }
        GameToolState::ManagedByGroup { group_title } => {
            group_notice_row.set_subtitle(
                &t!("Use {} settings to manage dependencies or run a program in that prefix.")
                    .replacen("{}", &group_title, 1),
            );
            tools_stack.set_visible_child_name("group");
        }
        GameToolState::ManagedByGlobal => {
            tools_stack.set_visible_child_name("global");
        }
    }
    let tools_stack_clone = tools_stack.clone();
    let group_notice_row_clone = group_notice_row.clone();
    let current_parent_group_for_tools = current_parent_group.clone();
    prefix_override_row.connect_enable_expansion_notify(move |row| {
        if row.enables_expansion() {
            tools_stack_clone.set_visible_child_name("available");
        } else if let Some(group) = current_parent_group_for_tools.as_ref()
            && !group.defaults.prefix_path.trim().is_empty()
        {
            group_notice_row_clone.set_subtitle(
                &t!("Use {} settings to manage dependencies or run a program in that prefix.")
                    .replacen("{}", &group.title, 1),
            );
            tools_stack_clone.set_visible_child_name("group");
        } else {
            tools_stack_clone.set_visible_child_name("global");
        }
    });
    let overlay_clone_menu = overlay.clone();
    let game_for_menu = game.clone();
    let group_for_menu = current_parent_group.clone();
    menu_btn.connect_clicked(move |button| {
        let button = button.clone();
        let overlay = overlay_clone_menu.clone();
        let game = game_for_menu.clone();
        let group = group_for_menu.clone();
        glib::spawn_future_local(async move {
            let leyen_id = game.leyen_id.clone();
            let exists = tokio::task::spawn_blocking(move || desktop_entry_exists(&leyen_id))
                .await
                .unwrap_or_else(|e| {
                    log::warn!("desktop_entry_exists task failed: {}", join_err(e));
                    false
                });
            if exists {
                match remove_game_desktop_entry(game.leyen_id.clone()).await {
                    Ok(_) => {
                        button.set_label(&t!("Add to menu"));
                        overlay.add_toast(adw::Toast::new(&t!("Removed from menu")));
                    }
                    Err(err) => overlay.add_toast(adw::Toast::new(&t!("Failed to remove menu entry: {}").replacen("{}", &err.to_string(), 1))),
                }
            } else {
                match create_game_desktop_entry(game, group).await {
                    Ok(_) => {
                        button.set_label(&t!("Remove from menu"));
                        overlay.add_toast(adw::Toast::new(&t!("Added to menu")));
                    }
                    Err(err) => overlay.add_toast(adw::Toast::new(&t!("Failed to create menu entry: {}").replacen("{}", &err.to_string(), 1))),
                }
            }
        });
    });
    tools.add(&menu_btn);

    page.add(&game_group);
    page.add(&settings_group);
    page.add(&env_group);
    page.add(&tools);

    let prefix_row_clone = prefix_row.clone();
    let prefix_override_row_clone = prefix_override_row.clone();
    let title_row_for_prefix = title_row.clone();
    let default_prefix_for_inherit = settings.default_prefix_path.clone();
    let manual_prefix = Rc::new(RefCell::new(game.prefix_path.clone()));
    let manual_prefix_clone = manual_prefix.clone();
    prefix_override_row.connect_enable_expansion_notify(move |row| {
        let custom_enabled = row.enables_expansion();
        if custom_enabled {
            let fallback = {
                let stored = manual_prefix_clone.borrow().clone();
                if !stored.trim().is_empty() {
                    stored
                } else {
                    suggest_prefix_path(&default_prefix_for_inherit, &title_row_for_prefix.text())
                }
            };
            prefix_row_clone.set_text(&fallback);
            prefix_override_row_clone.set_expanded(true);
        } else {
            *manual_prefix_clone.borrow_mut() = prefix_row_clone.text().to_string();
            prefix_row_clone.set_text("");
            prefix_override_row_clone.set_expanded(false);
        }
    });

    let proton_row_clone = proton_row.clone();
    let proton_override_row_clone = proton_override_row.clone();
    let manual_proton_selection = Rc::new(RefCell::new(proton_row.selected()));
    let manual_proton_selection_clone = manual_proton_selection.clone();
    proton_override_row.connect_enable_expansion_notify(move |row| {
        let custom_enabled = row.enables_expansion();
        if custom_enabled {
            proton_row_clone.set_selected(*manual_proton_selection_clone.borrow());
            proton_override_row_clone.set_expanded(true);
        } else {
            *manual_proton_selection_clone.borrow_mut() = proton_row_clone.selected();
            proton_row_clone.set_selected(0);
            proton_override_row_clone.set_expanded(false);
        }
    });

    let previous_auto_prefix = Rc::new(RefCell::new(game.prefix_path.clone()));
    let prefix_row_clone = prefix_row.clone();
    let prefix_override_row_clone = prefix_override_row.clone();
    let previous_auto_prefix_clone = previous_auto_prefix.clone();
    let default_prefix_path = settings.default_prefix_path.clone();
    title_row.connect_changed(move |row| {
        let title = row.text().to_string();
        if prefix_override_row_clone.enables_expansion() {
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

    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let path_row_clone = path_row_clone.clone();
        let filter = gtk4::FileFilter::new();
        filter.set_name(Some(&t!("Windows programs")));
        for suffix in ["exe", "msi", "bat", "cmd", "com"] {
            filter.add_suffix(suffix);
        }
        let file_dialog = gtk4::FileDialog::builder()
            .title(t!("Select Executable"))
            .default_filter(&filter)
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
            .title(t!("Select Prefix Folder"))
            .build();
        file_dialog.select_folder(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                prefix_row_clone.set_text(&path.to_string_lossy());
            }
        });
    });

    let game_icon_row_clone = game_icon_row.clone();
    let parent_clone = parent.clone();
    game_icon_browse_btn.connect_clicked(move |_| {
        let game_icon_row_clone = game_icon_row_clone.clone();
        let file_dialog = build_icon_file_dialog(&t!("Select Icon"));
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                game_icon_row_clone.set_text(&path.to_string_lossy());
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

    let overlay = adw::ToastOverlay::new();
    let root_page = adw::NavigationPage::builder()
        .child(&toolbar_view)
        .build();
    nav.add(&root_page);
    overlay.set_child(Some(&nav));
    dialog.set_child(Some(&overlay));

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| { dialog_clone.close(); });

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
            overlay_clone.add_toast(adw::Toast::new(&t!("Title and executable path are required")));
            return;
        }

        let ui_clone = ui_clone.clone();
        let overlay_clone = overlay_clone.clone();
        let parent_clone = parent_clone.clone();
        let game_id = game_id.clone();
        let original_game = original_game.clone();
        let dialog_clone = dialog_clone.clone();
        let prefix_override_row_val = prefix_override_row.clone();
        let prefix_row_val = prefix_row.clone();
        let proton_row_val = proton_row.clone();
        let proton_override_row_val = proton_override_row.clone();
        let available_protons = available_protons.clone();
        let args_entry_val = args_entry.clone();
        let mangohud_row_val = mangohud_row.clone();
        let gamemode_row_val = gamemode_row.clone();
        let wayland_row_val = wayland_row.clone();
        let wow64_row_val = wow64_row.clone();
        let ntsync_row_val = ntsync_row.clone();
        let hdr_row_val = hdr_row.clone();
        let proton_log_row_val = proton_log_row.clone();
        let game_icon_row_val = game_icon_row.clone();
        let game_icon_override_row_val = game_icon_override_row.clone();
        let current_parent_group = current_parent_group.clone();
        let title_row_val = title_row.clone();
        let path_row_val = path_row.clone();

        glib::spawn_future_local(async move {
            let title = title_row_val.text().to_string();
            let exe = path_row_val.text().to_string();

            let proton = if grouped_game && !proton_override_row_val.enables_expansion() {
                "Default".to_string()
            } else {
                available_protons
                    .get(proton_row_val.selected() as usize)
                    .cloned()
                    .unwrap_or_else(|| "Default".to_string())
            };

            if proton != "Default" {
                let p = proton.clone();
                if !tokio::task::spawn_blocking(move || std::path::Path::new(&p).exists())
                    .await
                    .unwrap_or(true)
                {
                    overlay_clone.add_toast(adw::Toast::new(&t!("Selected Proton path does not exist")));
                    return;
                }
            }

            let mut items = match crate::config::load_library().await {
                Ok(items) => items,
                Err(err) => {
                    overlay_clone.add_toast(adw::Toast::new(&err));
                    return;
                }
            };
            let custom_icon = game_icon_override_row_val.enables_expansion();
            let icon_notice = match apply_game_icon(
                game_id.clone(),
                exe.clone(),
                custom_icon,
                game_icon_row_val.text().to_string(),
            )
            .await
            {
                Ok(warning) => warning,
                Err(err) => {
                    overlay_clone.add_toast(adw::Toast::new(&err));
                    return;
                }
            };

            let edited_game = Game {
                id: game_id.clone(),
                title,
                exe_path: exe,
                prefix_path: if !prefix_override_row_val.enables_expansion() {
                    String::new()
                } else {
                    prefix_row_val.text().to_string()
                },
                proton,
                launch_args: args_entry_val.text().to_string(),
                mangohud: mangohud_row_val.is_active(),
                gamemode: gamemode_row_val.is_active(),
                wayland: wayland_row_val.is_active(),
                wow64: wow64_row_val.is_active(),
                ntsync: ntsync_row_val.is_active(),
                hdr: hdr_row_val.is_active(),
                proton_log: proton_log_row_val.is_active(),
                leyen_id: original_game.leyen_id.clone(),
                game_id: umu_game_id(&original_game.leyen_id),
                custom_icon,
                playtime_seconds: original_game.playtime_seconds,
                last_played_epoch_seconds: original_game.last_played_epoch_seconds,
                last_run_duration_seconds: original_game.last_run_duration_seconds,
                last_run_status: original_game.last_run_status.clone(),
            };

            if replace_game(&mut items, &edited_game) {
                crate::config::save_library(items).await;
                let desktop_notice = update_game_desktop_entry_if_present(
                    edited_game.clone(),
                    current_parent_group.clone(),
                )
                .await
                .err()
                .map(|err| format!("Failed to update menu entry: {err}"));
                refresh_library_view(&ui_clone, &overlay_clone, &parent_clone).await;
                let mut notices = Vec::new();
                if let Some(icon_notice) = icon_notice {
                    notices.push(icon_notice);
                }
                if let Some(desktop_notice) = desktop_notice {
                    notices.push(desktop_notice);
                }
                let success_message = if notices.is_empty() {
                    t!("Game updated successfully")
                } else {
                    t!("Game updated successfully. {}").replacen("{}", &notices.join(" "), 1)
                };
                overlay_clone.add_toast(adw::Toast::new(&success_message));
                dialog_clone.close();
            } else {
                overlay_clone.add_toast(adw::Toast::new(&t!("Error: Game not found")));
            }
        });
    });

    dialog.present(Some(parent));
}

pub async fn show_delete_confirmation(
    parent: &adw::ApplicationWindow,
    ui: &LibraryUi,
    overlay: &adw::ToastOverlay,
    item_id: &str,
) {
    let items = match crate::config::load_library().await {
        Ok(items) => items,
        Err(err) => {
            overlay.add_toast(adw::Toast::new(&err));
            return;
        }
    };
    let label = items
        .iter()
        .find_map(|item| match item {
            LibraryItem::Game(game) if game.id == item_id => Some(t!("game '{}'").replacen("{}", &game.title, 1)),
            LibraryItem::Group(group) if group.id == item_id => {
                Some(t!("group '{}'").replacen("{}", &group.title, 1))
            }
            _ => None,
        })
        .or_else(|| {
            items.iter().find_map(|item| match item {
                LibraryItem::Group(group) => group
                    .games
                    .iter()
                    .find(|game| game.id == item_id)
                    .map(|game| t!("game '{}'").replacen("{}", &game.title, 1)),
                LibraryItem::Game(_) => None,
            })
        })
        .unwrap_or_else(|| t!("item"));

    let dialog = gtk4::AlertDialog::builder()
        .message(t!("Delete Item?"))
        .detail(
            t!("Are you sure you want to delete {}?\n\nThis action cannot be undone.")
                .replacen("{}", &label, 1),
        )
        .buttons(vec![t!("Cancel"), t!("Delete")])
        .cancel_button(0)
        .default_button(0)
        .build();

    let item_id = item_id.to_string();
    let ui_clone = ui.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Ok(1) = result {
            let ui_clone = ui_clone.clone();
            let overlay_clone = overlay_clone.clone();
            let parent_clone = parent_clone.clone();
            let item_id = item_id.clone();

            glib::spawn_future_local(async move {
                let mut items = match crate::config::load_library().await {
                Ok(items) => items,
                Err(err) => {
                    overlay_clone.add_toast(adw::Toast::new(&err));
                    return;
                }
            };

                let mut delete_notice = None;
                let deleted = if let Some(game) = remove_game(&mut items, &item_id) {
                    let gid = game.id.clone();
                    tokio::task::spawn_blocking(move || clear_game_icon(&gid))
                        .await
                        .ok();
                    if let Err(err) = remove_game_desktop_entry(game.leyen_id.clone()).await {
                        delete_notice = Some(t!("Failed to remove menu entry: {}").replacen("{}", &err.to_string(), 1));
                    }
                    Some(game.title)
                } else if let Some(group) = remove_group(&mut items, &item_id) {
                    let gid = group.id.clone();
                    tokio::task::spawn_blocking(move || clear_group_icon(&gid))
                        .await
                        .ok();
                    for game in &group.games {
                        let gid = game.id.clone();
                        tokio::task::spawn_blocking(move || clear_game_icon(&gid))
                            .await
                            .ok();
                        if let Err(err) = remove_game_desktop_entry(game.leyen_id.clone()).await {
                            delete_notice = Some(t!("Failed to remove a menu entry: {}").replacen("{}", &err.to_string(), 1));
                        }
                    }
                    Some(group.title)
                } else {
                    None
                };

                if let Some(title) = deleted {
                    crate::config::save_library(items).await;
                    refresh_library_view(&ui_clone, &overlay_clone, &parent_clone).await;
                    let message = if let Some(delete_notice) = delete_notice {
                        t!("'{}' deleted successfully. {}").replacen("{}", &title, 1).replacen("{}", &delete_notice, 1)
                    } else {
                        t!("'{}' deleted successfully").replacen("{}", &title, 1)
                    };
                    overlay_clone.add_toast(adw::Toast::new(&message));
                }
            });
        }
    });
}
