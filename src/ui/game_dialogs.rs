use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use std::path::PathBuf;

use crate::config::{load_games, load_settings, save_games};
use crate::models::Game;
use crate::proton::resolve_proton_path;

use super::deps_dialog::show_dependencies_dialog;
use super::populate_game_list;

// --- ADD GAME DIALOG ---

pub fn show_add_game_dialog(
    parent: &adw::ApplicationWindow,
    list_box: &gtk4::Box,
    empty_state: &gtk4::Box,
    overlay: &adw::ToastOverlay,
) {
    let settings = load_settings();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(450)
        .default_height(600)
        .destroy_with_parent(true)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Add Game", ""))
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

    let clamp = adw::Clamp::builder()
        .margin_top(16)
        .margin_bottom(16)
        .build();
    let page = adw::PreferencesPage::builder().build();

    // Input Fields
    let title_row = adw::EntryRow::builder().title("Title").build();
    let path_row = adw::EntryRow::builder().title("Executable").build();

    let browse_btn = gtk4::Button::builder()
        .label("Browse...")
        .valign(gtk4::Align::Center)
        .build();

    path_row.add_suffix(&browse_btn);

    let game_group = adw::PreferencesGroup::builder().title("Game").build();
    game_group.add(&title_row);
    game_group.add(&path_row);

    // File chooser for executable
    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let path_row_clone = path_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Executable")
            .build();
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    path_row_clone.set_text(&path.to_string_lossy());
                }
            }
        });
    });

    let prefix_row = adw::EntryRow::builder()
        .title("Prefix")
        .text(&settings.default_prefix_path)
        .build();

    let prefix_browse_btn = gtk4::Button::builder()
        .label("Browse...")
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let prefix_row_clone = prefix_row.clone();
    let parent_clone2 = parent.clone();
    prefix_browse_btn.connect_clicked(move |_| {
        let prefix_row_clone = prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Prefix Folder")
            .build();
        file_dialog.select_folder(
            Some(&parent_clone2),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        prefix_row_clone.set_text(&path.to_string_lossy());
                    }
                }
            },
        );
    });

    let game_id_row = adw::EntryRow::builder().title("Game ID").build();

    // Build Proton dropdown – display basenames, store full paths via index
    let proton_display_names_add: Vec<String> = settings
        .available_proton_versions
        .iter()
        .map(|p| {
            if p == "Default" {
                "Default".to_string()
            } else {
                PathBuf::from(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.clone())
            }
        })
        .collect();
    let proton_display_refs_add: Vec<&str> = proton_display_names_add
        .iter()
        .map(|s| s.as_str())
        .collect();
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&gtk4::StringList::new(&proton_display_refs_add))
        .build();

    let env_group = adw::PreferencesGroup::builder()
        .title("Environment")
        .build();
    env_group.add(&prefix_row);
    env_group.add(&game_id_row);
    env_group.add(&proton_row);

    let args_row = adw::EntryRow::builder().title("Launch Arguments").build();
    let mangohud_row = adw::SwitchRow::builder()
        .title("Force MangoHud")
        .active(settings.global_mangohud)
        .build();
    let gamemode_row = adw::SwitchRow::builder()
        .title("Force GameMode")
        .active(settings.global_gamemode)
        .build();
    let wayland_row_game = adw::SwitchRow::builder()
        .title("Wayland")
        .active(false)
        .build();
    let wow64_row_game = adw::SwitchRow::builder()
        .title("WoW64")
        .active(false)
        .build();
    let ntsync_row_game = adw::SwitchRow::builder()
        .title("NTSync")
        .active(false)
        .build();
    let advanced_group = adw::PreferencesGroup::builder().title("Overrides").build();
    advanced_group.add(&args_row);
    advanced_group.add(&mangohud_row);
    advanced_group.add(&gamemode_row);
    advanced_group.add(&wayland_row_game);
    advanced_group.add(&wow64_row_game);
    advanced_group.add(&ntsync_row_game);

    page.add(&game_group);
    page.add(&env_group);
    page.add(&advanced_group);

    clamp.set_child(Some(&page));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();
    toolbar_view.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar_view));

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| dialog_clone.destroy());

    // --- SAVE NEW GAME LOGIC ---
    let dialog_clone_2 = dialog.clone();
    let list_box_clone = list_box.clone();
    let empty_state_clone = empty_state.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();

    add_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        let exe = path_row.text().to_string();

        if title.is_empty() || exe.is_empty() {
            overlay_clone.add_toast(adw::Toast::new("Title and executable path are required"));
            return;
        }

        let new_game = Game {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            exe_path: exe,
            prefix_path: prefix_row.text().to_string(),
            proton: if proton_row.selected() < settings.available_proton_versions.len() as u32 {
                settings.available_proton_versions[proton_row.selected() as usize].clone()
            } else {
                "Default".to_string()
            },
            launch_args: args_row.text().to_string(),
            force_mangohud: mangohud_row.is_active(),
            force_gamemode: gamemode_row.is_active(),
            game_wayland: wayland_row_game.is_active(),
            game_wow64: wow64_row_game.is_active(),
            game_ntsync: ntsync_row_game.is_active(),
            game_id: game_id_row.text().to_string(),
        };

        // Load existing games, add new one, save back to disk
        let mut games = load_games();
        games.push(new_game);
        save_games(&games);

        // Refresh UI
        populate_game_list(
            &list_box_clone,
            &empty_state_clone,
            &games,
            &overlay_clone,
            &parent_clone,
        );

        overlay_clone.add_toast(adw::Toast::new("Game added successfully"));
        dialog_clone_2.destroy();
    });

    dialog.present();
}

// --- EDIT GAME DIALOG ---

pub fn show_edit_game_dialog(
    parent: &adw::ApplicationWindow,
    list_box: &gtk4::Box,
    empty_state: &gtk4::Box,
    overlay: &adw::ToastOverlay,
    game: &Game,
) {
    let settings = load_settings();
    let game_id = game.id.clone();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(450)
        .default_height(600)
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

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);

    let clamp = adw::Clamp::builder()
        .margin_top(16)
        .margin_bottom(16)
        .build();
    let page = adw::PreferencesPage::builder().build();

    // Input Fields - pre-populated with existing game data
    let title_row = adw::EntryRow::builder()
        .title("Title")
        .text(&game.title)
        .build();

    let path_row = adw::EntryRow::builder()
        .title("Executable")
        .text(&game.exe_path)
        .build();

    let browse_btn = gtk4::Button::builder()
        .label("Browse...")
        .valign(gtk4::Align::Center)
        .build();

    path_row.add_suffix(&browse_btn);

    let game_group = adw::PreferencesGroup::builder().title("Game").build();
    game_group.add(&title_row);
    game_group.add(&path_row);

    // File chooser for executable
    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let path_row_clone = path_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Executable")
            .build();
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    path_row_clone.set_text(&path.to_string_lossy());
                }
            }
        });
    });

    let prefix_row = adw::EntryRow::builder()
        .title("Prefix")
        .text(&game.prefix_path)
        .build();

    let prefix_browse_btn = gtk4::Button::builder()
        .label("Browse...")
        .valign(gtk4::Align::Center)
        .build();
    prefix_row.add_suffix(&prefix_browse_btn);

    let prefix_row_clone = prefix_row.clone();
    let parent_clone2 = parent.clone();
    prefix_browse_btn.connect_clicked(move |_| {
        let prefix_row_clone = prefix_row_clone.clone();
        let file_dialog = gtk4::FileDialog::builder()
            .title("Select Prefix Folder")
            .build();
        file_dialog.select_folder(
            Some(&parent_clone2),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        prefix_row_clone.set_text(&path.to_string_lossy());
                    }
                }
            },
        );
    });

    let game_id_row = adw::EntryRow::builder()
        .title("Game ID")
        .text(&game.game_id)
        .build();

    // Build Proton dropdown – display basenames, store full paths via index
    let proton_display_names_edit: Vec<String> = settings
        .available_proton_versions
        .iter()
        .map(|p| {
            if p == "Default" {
                "Default".to_string()
            } else {
                PathBuf::from(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.clone())
            }
        })
        .collect();
    let proton_display_refs_edit: Vec<&str> = proton_display_names_edit
        .iter()
        .map(|s| s.as_str())
        .collect();
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&gtk4::StringList::new(&proton_display_refs_edit))
        .build();

    // Set selected Proton version (match by full path)
    if let Some(pos) = settings
        .available_proton_versions
        .iter()
        .position(|v| v == &game.proton)
    {
        proton_row.set_selected(pos as u32);
    }

    let env_group = adw::PreferencesGroup::builder()
        .title("Environment")
        .build();
    env_group.add(&prefix_row);
    env_group.add(&game_id_row);
    env_group.add(&proton_row);

    let args_row = adw::EntryRow::builder()
        .title("Launch Arguments")
        .text(&game.launch_args)
        .build();

    let mangohud_row = adw::SwitchRow::builder()
        .title("Force MangoHud")
        .active(game.force_mangohud)
        .build();

    let gamemode_row = adw::SwitchRow::builder()
        .title("Force GameMode")
        .active(game.force_gamemode)
        .build();

    let wayland_row_game = adw::SwitchRow::builder()
        .title("Wayland")
        .active(game.game_wayland)
        .build();

    let wow64_row_game = adw::SwitchRow::builder()
        .title("WoW64")
        .active(game.game_wow64)
        .build();

    let ntsync_row_game = adw::SwitchRow::builder()
        .title("NTSync")
        .active(game.game_ntsync)
        .build();

    let advanced_group = adw::PreferencesGroup::builder().title("Overrides").build();
    advanced_group.add(&args_row);
    advanced_group.add(&mangohud_row);
    advanced_group.add(&gamemode_row);
    advanced_group.add(&wayland_row_game);
    advanced_group.add(&wow64_row_game);
    advanced_group.add(&ntsync_row_game);

    let deps_btn = gtk4::Button::builder()
        .label("Manage Dependencies")
        .build();

    let game_prefix = game.prefix_path.clone();
    let game_proton = resolve_proton_path(&game.proton).unwrap_or_default();
    let overlay_clone_deps = overlay.clone();
    let dialog_parent = parent.clone();
    deps_btn.connect_clicked(move |_| {
        show_dependencies_dialog(
            &dialog_parent,
            &game_prefix,
            &game_proton,
            &overlay_clone_deps,
        );
    });

    let tools_group = adw::PreferencesGroup::builder().title("Tools").build();
    tools_group.add(&deps_btn);

    page.add(&game_group);
    page.add(&env_group);
    page.add(&advanced_group);
    page.add(&tools_group);

    clamp.set_child(Some(&page));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();
    toolbar_view.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar_view));

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| dialog_clone.destroy());

    // --- SAVE EDITED GAME LOGIC ---
    let dialog_clone_2 = dialog.clone();
    let list_box_clone = list_box.clone();
    let empty_state_clone = empty_state.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();

    save_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        let exe = path_row.text().to_string();

        if title.is_empty() || exe.is_empty() {
            overlay_clone.add_toast(adw::Toast::new("Title and executable path are required"));
            return;
        }

        let edited_game = Game {
            id: game_id.clone(),
            title,
            exe_path: exe,
            prefix_path: prefix_row.text().to_string(),
            proton: if proton_row.selected() < settings.available_proton_versions.len() as u32 {
                settings.available_proton_versions[proton_row.selected() as usize].clone()
            } else {
                "Default".to_string()
            },
            launch_args: args_row.text().to_string(),
            force_mangohud: mangohud_row.is_active(),
            force_gamemode: gamemode_row.is_active(),
            game_wayland: wayland_row_game.is_active(),
            game_wow64: wow64_row_game.is_active(),
            game_ntsync: ntsync_row_game.is_active(),
            game_id: game_id_row.text().to_string(),
        };

        // Load games, find and replace the edited one
        let mut games = load_games();
        if let Some(pos) = games.iter().position(|g| g.id == game_id) {
            games[pos] = edited_game;
            save_games(&games);

            // Refresh UI
            populate_game_list(
                &list_box_clone,
                &empty_state_clone,
                &games,
                &overlay_clone,
                &parent_clone,
            );

            overlay_clone.add_toast(adw::Toast::new("Game updated successfully"));
            dialog_clone_2.destroy();
        } else {
            overlay_clone.add_toast(adw::Toast::new("Error: Game not found"));
        }
    });

    dialog.present();
}

// --- DELETE CONFIRMATION DIALOG ---

pub fn show_delete_confirmation(
    parent: &adw::ApplicationWindow,
    list_box: &gtk4::Box,
    empty_state: &gtk4::Box,
    overlay: &adw::ToastOverlay,
    game_id: &str,
) {
    let games = load_games();
    let game = games.iter().find(|g| g.id == game_id);

    let game_title = game.map(|g| g.title.as_str()).unwrap_or("Unknown Game");

    let dialog = gtk4::AlertDialog::builder()
        .message("Delete Game?")
        .detail(&format!(
            "Are you sure you want to delete '{}'?\n\nThis action cannot be undone.",
            game_title
        ))
        .buttons(vec!["Cancel".to_string(), "Delete".to_string()])
        .cancel_button(0)
        .default_button(0)
        .build();

    let game_id = game_id.to_string();
    let list_box_clone = list_box.clone();
    let empty_state_clone = empty_state.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();

    dialog.choose(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Ok(response) = result {
            if response == 1 {
                // "Delete" button is at index 1
                let mut games = load_games();
                if let Some(pos) = games.iter().position(|g| g.id == game_id) {
                    let deleted_title = games[pos].title.clone();
                    games.remove(pos);
                    save_games(&games);

                    // Refresh UI
                    populate_game_list(
                        &list_box_clone,
                        &empty_state_clone,
                        &games,
                        &overlay_clone,
                        &parent_clone,
                    );

                    overlay_clone.add_toast(adw::Toast::new(&format!(
                        "'{}' deleted successfully",
                        deleted_title
                    )));
                }
            }
        }
    });
}
