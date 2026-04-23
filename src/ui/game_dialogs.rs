use libadwaita as adw;

use adw::prelude::*;
use gtk4::gio;
use std::path::PathBuf;

use crate::config::{load_games, load_settings, save_games};
use crate::models::{Game, ViewMode};
use crate::proton::resolve_proton_path;

use super::deps_dialog::show_dependencies_dialog;
use super::populate_game_views;

fn refresh_ui(
    flow_box: &gtk4::FlowBox,
    list_box: &gtk4::ListBox,
    stack: &gtk4::Stack,
    search_entry: &gtk4::SearchEntry,
    overlay: &adw::ToastOverlay,
    parent: &adw::ApplicationWindow,
    view_mode: &ViewMode,
) {
    let games = load_games();
    populate_game_views(
        flow_box,
        list_box,
        stack,
        &games,
        &search_entry.text(),
        overlay,
        parent,
        view_mode,
    );
}

fn build_cover_row(
    parent: &adw::ApplicationWindow,
    initial_path: Option<&str>,
) -> (adw::ActionRow, gtk4::Picture, gtk4::Label) {
    let row = adw::ActionRow::builder().title("Cover Art").build();
    let picture = gtk4::Picture::new();
    picture.set_size_request(160, 90);
    picture.set_content_fit(gtk4::ContentFit::Cover);

    let path_label = gtk4::Label::builder()
        .label(initial_path.unwrap_or("No cover selected"))
        .xalign(0.0)
        .ellipsize(gtk4::pango::EllipsizeMode::Middle)
        .css_classes(["dim-label"])
        .build();

    if let Some(path) = initial_path {
        picture.set_filename(Some(path));
    } else {
        picture.set_icon_name(Some("image-x-generic-symbolic"));
    }

    let browse_btn = gtk4::Button::builder().label("Browse…").build();
    row.add_prefix(&picture);
    row.add_suffix(&browse_btn);
    row.set_subtitle(path_label.label().as_str());

    let parent_clone = parent.clone();
    let picture_clone = picture.clone();
    let path_label_clone = path_label.clone();
    browse_btn.connect_clicked(move |_| {
        let image_filter = gtk4::FileFilter::new();
        image_filter.set_name(Some("Image files"));
        image_filter.add_mime_type("image/png");
        image_filter.add_mime_type("image/jpeg");
        image_filter.add_mime_type("image/webp");

        let filters: gio::ListStore = gio::ListStore::new::<gtk4::FileFilter>();
        filters.append(&image_filter);

        let dialog = gtk4::FileDialog::builder()
            .title("Select Cover Art")
            .build();
        dialog.set_filters(Some(&filters));
        dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    let path_str = path.to_string_lossy().to_string();
                    picture_clone.set_filename(Some(&path_str));
                    path_label_clone.set_label(&path_str);
                }
            }
        });
    });

    (row, picture, path_label)
}

pub fn show_add_game_dialog(
    parent: &adw::ApplicationWindow,
    flow_box: &gtk4::FlowBox,
    list_box: &gtk4::ListBox,
    stack: &gtk4::Stack,
    search_entry: &gtk4::SearchEntry,
    overlay: &adw::ToastOverlay,
    view_mode: &ViewMode,
) {
    let settings = load_settings();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(450)
        .default_height(640)
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

    let title_row = adw::EntryRow::builder().title("Title").build();
    let path_row = adw::EntryRow::builder().title("Executable").build();
    let browse_btn = gtk4::Button::builder().label("Browse...").build();
    path_row.add_suffix(&browse_btn);

    let game_group = adw::PreferencesGroup::builder().title("Game").build();
    game_group.add(&title_row);
    game_group.add(&path_row);

    let (cover_row, _cover_picture, cover_path_label) = build_cover_row(parent, None);
    game_group.add(&cover_row);

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

    let prefix_browse_btn = gtk4::Button::builder().label("Browse...").build();
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
    let wayland_row_game = adw::SwitchRow::builder().title("Wayland").build();
    let wow64_row_game = adw::SwitchRow::builder().title("WoW64").build();
    let ntsync_row_game = adw::SwitchRow::builder().title("NTSync").build();
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

    let dialog_clone_2 = dialog.clone();
    let flow_box_clone = flow_box.clone();
    let list_box_clone = list_box.clone();
    let stack_clone = stack.clone();
    let search_clone = search_entry.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let view_mode = view_mode.clone();

    add_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        let exe = path_row.text().to_string();

        if title.is_empty() || exe.is_empty() {
            overlay_clone.add_toast(adw::Toast::new("Title and executable path are required"));
            return;
        }

        let cover_value = cover_path_label.label().to_string();
        let cover_path = if cover_value == "No cover selected" {
            None
        } else {
            Some(cover_value)
        };

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
            cover_path,
        };

        let mut games = load_games();
        games.push(new_game);
        save_games(&games);
        refresh_ui(
            &flow_box_clone,
            &list_box_clone,
            &stack_clone,
            &search_clone,
            &overlay_clone,
            &parent_clone,
            &view_mode,
        );

        overlay_clone.add_toast(adw::Toast::new("Game added successfully"));
        dialog_clone_2.destroy();
    });

    dialog.present();
}

#[allow(clippy::too_many_arguments)]
pub fn show_edit_game_dialog(
    parent: &adw::ApplicationWindow,
    flow_box: &gtk4::FlowBox,
    list_box: &gtk4::ListBox,
    stack: &gtk4::Stack,
    search_entry: &gtk4::SearchEntry,
    overlay: &adw::ToastOverlay,
    view_mode: &ViewMode,
    game: &Game,
) {
    let settings = load_settings();
    let game_id = game.id.clone();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(450)
        .default_height(640)
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

    let title_row = adw::EntryRow::builder()
        .title("Title")
        .text(&game.title)
        .build();
    let path_row = adw::EntryRow::builder()
        .title("Executable")
        .text(&game.exe_path)
        .build();

    let browse_btn = gtk4::Button::builder().label("Browse...").build();
    path_row.add_suffix(&browse_btn);

    let game_group = adw::PreferencesGroup::builder().title("Game").build();
    game_group.add(&title_row);
    game_group.add(&path_row);
    let (cover_row, _cover_picture, cover_path_label) =
        build_cover_row(parent, game.cover_path.as_deref());
    game_group.add(&cover_row);

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

    let prefix_browse_btn = gtk4::Button::builder().label("Browse...").build();
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

    let deps_btn = gtk4::Button::builder().label("Manage Dependencies").build();

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

    let dialog_clone_2 = dialog.clone();
    let flow_box_clone = flow_box.clone();
    let list_box_clone = list_box.clone();
    let stack_clone = stack.clone();
    let search_clone = search_entry.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let view_mode = view_mode.clone();

    save_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        let exe = path_row.text().to_string();

        if title.is_empty() || exe.is_empty() {
            overlay_clone.add_toast(adw::Toast::new("Title and executable path are required"));
            return;
        }

        let cover_value = cover_path_label.label().to_string();
        let cover_path = if cover_value == "No cover selected" {
            None
        } else {
            Some(cover_value)
        };

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
            cover_path,
        };

        let mut games = load_games();
        if let Some(pos) = games.iter().position(|g| g.id == game_id) {
            games[pos] = edited_game;
            save_games(&games);
            refresh_ui(
                &flow_box_clone,
                &list_box_clone,
                &stack_clone,
                &search_clone,
                &overlay_clone,
                &parent_clone,
                &view_mode,
            );
            overlay_clone.add_toast(adw::Toast::new("Game updated successfully"));
            dialog_clone_2.destroy();
        } else {
            overlay_clone.add_toast(adw::Toast::new("Error: Game not found"));
        }
    });

    dialog.present();
}

#[allow(clippy::too_many_arguments)]
pub fn show_delete_confirmation(
    parent: &adw::ApplicationWindow,
    flow_box: &gtk4::FlowBox,
    list_box: &gtk4::ListBox,
    stack: &gtk4::Stack,
    search_entry: &gtk4::SearchEntry,
    overlay: &adw::ToastOverlay,
    view_mode: &ViewMode,
    game_id: &str,
) {
    let games = load_games();
    let game = games.iter().find(|g| g.id == game_id);

    let game_title = game.map(|g| g.title.as_str()).unwrap_or("Unknown Game");

    let dialog = gtk4::AlertDialog::builder()
        .message("Delete Game?")
        .detail(&format!(
            "Are you sure you want to delete '{}' ?\n\nThis action cannot be undone.",
            game_title
        ))
        .buttons(vec!["Cancel".to_string(), "Delete".to_string()])
        .cancel_button(0)
        .default_button(0)
        .build();

    let game_id = game_id.to_string();
    let flow_box_clone = flow_box.clone();
    let list_box_clone = list_box.clone();
    let stack_clone = stack.clone();
    let search_clone = search_entry.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();
    let view_mode = view_mode.clone();

    dialog.choose(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Ok(1) = result {
            let mut games = load_games();
            if let Some(pos) = games.iter().position(|g| g.id == game_id) {
                let deleted_title = games[pos].title.clone();
                games.remove(pos);
                save_games(&games);
                refresh_ui(
                    &flow_box_clone,
                    &list_box_clone,
                    &stack_clone,
                    &search_clone,
                    &overlay_clone,
                    &parent_clone,
                    &view_mode,
                );
                overlay_clone.add_toast(adw::Toast::new(&format!(
                    "'{}' deleted successfully",
                    deleted_title
                )));
            }
        }
    });
}
