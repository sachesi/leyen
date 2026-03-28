use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_ID: &str = "com.github.leyen";

// --- DATA STRUCTURES ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Game {
    id: String,
    title: String,
    exe_path: String,
    prefix_path: String,
    proton: String,
    launch_args: String,
    force_mangohud: bool,
    force_gamemode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GlobalSettings {
    default_prefix_path: String,
    default_proton: String,
    global_mangohud: bool,
    global_gamemode: bool,
    available_proton_versions: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GamesConfig {
    games: Vec<Game>,
}

// --- FILE IO ---

fn get_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let config_dir = PathBuf::from(format!("{}/.config/leyen", home));
    if !config_dir.exists() {
        let _ = fs::create_dir_all(&config_dir);
    }
    config_dir
}

fn get_config_path() -> PathBuf {
    get_config_dir().join("games.toml")
}

fn get_settings_path() -> PathBuf {
    get_config_dir().join("settings.toml")
}

fn load_games() -> Vec<Game> {
    let path = get_config_path();
    if let Ok(data) = fs::read_to_string(path) {
        toml::from_str::<GamesConfig>(&data)
            .map(|config| config.games)
            .unwrap_or_else(|_| Vec::new())
    } else {
        Vec::new()
    }
}

fn save_games(games: &[Game]) {
    let path = get_config_path();
    let config = GamesConfig {
        games: games.to_vec(),
    };
    if let Ok(data) = toml::to_string_pretty(&config) {
        let _ = fs::write(path, data);
    }
}

fn load_settings() -> GlobalSettings {
    let path = get_settings_path();
    if let Ok(data) = fs::read_to_string(path) {
        toml::from_str(&data).unwrap_or_else(|_| {
            let settings = detect_proton_versions();
            save_settings(&settings);
            settings
        })
    } else {
        let settings = detect_proton_versions();
        save_settings(&settings);
        settings
    }
}

fn save_settings(settings: &GlobalSettings) {
    let path = get_settings_path();
    if let Ok(data) = toml::to_string_pretty(settings) {
        let _ = fs::write(path, data);
    }
}

fn detect_proton_versions() -> GlobalSettings {
    let mut versions = vec!["Default".to_string()];

    // Check common Proton installation locations
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());

    // Check local leyen Proton directory first
    let leyen_proton = PathBuf::from(format!("{}/.local/share/leyen/proton", home));
    if leyen_proton.exists() {
        if let Ok(entries) = fs::read_dir(&leyen_proton) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        versions.push(name.to_string());
                    }
                }
            }
        }
    } else {
        // Create the directory if it doesn't exist
        let _ = fs::create_dir_all(&leyen_proton);
    }

    // Steam's compatibility tools
    let steam_compat = PathBuf::from(format!("{}/.steam/steam/compatibilitytools.d", home));
    if steam_compat.exists() {
        if let Ok(entries) = fs::read_dir(steam_compat) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        versions.push(name.to_string());
                    }
                }
            }
        }
    }

    // Check for system-installed Proton
    let steam_root = PathBuf::from(format!("{}/.steam/steam/steamapps/common", home));
    if steam_root.exists() {
        if let Ok(entries) = fs::read_dir(steam_root) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.contains("Proton") {
                            versions.push(name.to_string());
                        }
                    }
                }
            }
        }
    }

    GlobalSettings {
        default_prefix_path: format!("{}/.wine", home),
        default_proton: "Default".to_string(),
        global_mangohud: false,
        global_gamemode: false,
        available_proton_versions: versions,
    }
}

// --- MAIN UI ---

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let header = adw::HeaderBar::builder().build();

    let add_btn = gtk4::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Game")
        .build();

    let settings_btn = gtk4::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Preferences")
        .build();

    header.pack_start(&add_btn);
    header.pack_end(&settings_btn);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);

    let clamp = adw::Clamp::builder()
        .maximum_size(800)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .build();

    let game_list_box = gtk4::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk4::SelectionMode::None)
        .build();

    clamp.set_child(Some(&game_list_box));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&scroll));
    toolbar_view.set_content(Some(&toast_overlay));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Leyen")
        .default_width(700)
        .default_height(600)
        .content(&toolbar_view)
        .build();

    // Load games from disk and populate the list
    let games = load_games();
    populate_game_list(&game_list_box, &games, &toast_overlay, &window);

    /* --- EVENT HANDLERS --- */

    let window_clone = window.clone();
    settings_btn.connect_clicked(move |_| {
        show_global_settings(&window_clone);
    });

    let window_clone_2 = window.clone();
    let list_box_clone = game_list_box.clone();
    let overlay_clone = toast_overlay.clone();
    add_btn.connect_clicked(move |_| {
        show_add_game_dialog(&window_clone_2, &list_box_clone, &overlay_clone);
    });

    window.present();
}

// --- DYNAMIC UI GENERATOR ---

fn populate_game_list(
    list_box: &gtk4::ListBox,
    games: &[Game],
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    // Clear existing children
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    for game in games {
        let row = adw::ActionRow::builder()
            .title(&game.title)
            .subtitle(&game.exe_path)
            .build();

        let icon = gtk4::Image::builder()
            .icon_name("application-x-executable-symbolic")
            .pixel_size(48)
            .margin_top(8)
            .margin_bottom(8)
            .build();

        // Button box for actions
        let button_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk4::Align::Center)
            .margin_top(8)
            .margin_bottom(8)
            .build();

        let edit_btn = gtk4::Button::builder()
            .icon_name("document-edit-symbolic")
            .valign(gtk4::Align::Center)
            .tooltip_text("Edit Game")
            .build();

        let delete_btn = gtk4::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk4::Align::Center)
            .tooltip_text("Delete Game")
            .css_classes(["destructive-action"])
            .build();

        let play_btn = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .css_classes(["suggested-action", "circular"])
            .valign(gtk4::Align::Center)
            .tooltip_text("Launch Game")
            .build();

        // Launch Logic!
        let game_clone = game.clone();
        let overlay_clone = overlay.clone();
        play_btn.connect_clicked(move |_| {
            launch_game(&game_clone, &overlay_clone);
        });

        // Edit Logic
        let game_clone = game.clone();
        let list_box_clone = list_box.clone();
        let overlay_clone = overlay.clone();
        let window_clone = window.clone();
        edit_btn.connect_clicked(move |_| {
            show_edit_game_dialog(&window_clone, &list_box_clone, &overlay_clone, &game_clone);
        });

        // Delete Logic
        let game_id = game.id.clone();
        let list_box_clone = list_box.clone();
        let overlay_clone = overlay.clone();
        let window_clone = window.clone();
        delete_btn.connect_clicked(move |_| {
            show_delete_confirmation(&window_clone, &list_box_clone, &overlay_clone, &game_id);
        });

        button_box.append(&edit_btn);
        button_box.append(&delete_btn);
        button_box.append(&play_btn);

        row.add_prefix(&icon);
        row.add_suffix(&button_box);
        list_box.append(&row);
    }
}

// --- CORE LAUNCH LOGIC ---

fn launch_game(game: &Game, overlay: &adw::ToastOverlay) {
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);

    // Apply Game's specific environment overrides
    if !game.prefix_path.is_empty() {
        launcher.setenv("WINEPREFIX", &game.prefix_path, true);
    }
    if !game.proton.is_empty() && game.proton != "Default" {
        launcher.setenv("PROTONPATH", &game.proton, true);
    }
    if game.force_mangohud {
        launcher.setenv("MANGOHUD", "1", true);
    }

    let mut cmd_args: Vec<String> = Vec::new();
    if game.force_gamemode {
        cmd_args.push("gamemoderun".to_string());
    }
    cmd_args.push("umu-run".to_string());
    cmd_args.push(game.exe_path.clone());

    if !game.launch_args.is_empty() {
        cmd_args.push(game.launch_args.clone());
    }

    let os_args: Vec<&std::ffi::OsStr> = cmd_args.iter().map(std::ffi::OsStr::new).collect();

    match launcher.spawn(&os_args) {
        Ok(_) => {
            overlay.add_toast(adw::Toast::new(&format!("Launching {}...", game.title)));
        }
        Err(e) => {
            overlay.add_toast(adw::Toast::new(&format!("Failed to launch: {}", e)));
        }
    }
}

// --- GLOBAL SETTINGS DIALOG ---

fn show_global_settings(parent: &adw::ApplicationWindow) {
    let mut settings = load_settings();

    let pref_window = adw::PreferencesWindow::builder()
        .transient_for(parent)
        .modal(true)
        .search_enabled(true)
        .default_width(700)
        .default_height(500)
        .build();

    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("emblem-system-symbolic")
        .build();

    let paths_group = adw::PreferencesGroup::builder()
        .title("Default Paths")
        .build();

    let prefix_row = adw::EntryRow::builder()
        .title("Default Prefix Path")
        .text(&settings.default_prefix_path)
        .build();

    // Build Proton dropdown list
    let proton_list = gtk4::StringList::new(&[]);
    for version in &settings.available_proton_versions {
        proton_list.append(version);
    }

    let proton_row = adw::ComboRow::builder()
        .title("Default Proton")
        .model(&proton_list)
        .build();

    // Set selected index
    if let Some(pos) = settings
        .available_proton_versions
        .iter()
        .position(|v| v == &settings.default_proton)
    {
        proton_row.set_selected(pos as u32);
    }

    paths_group.add(&prefix_row);
    paths_group.add(&proton_row);

    let tools_group = adw::PreferencesGroup::builder()
        .title("Global Environment")
        .build();

    let mangohud_row = adw::SwitchRow::builder()
        .title("MangoHud Overlay")
        .active(settings.global_mangohud)
        .build();

    let gamemode_row = adw::SwitchRow::builder()
        .title("GameMode")
        .active(settings.global_gamemode)
        .build();

    tools_group.add(&mangohud_row);
    tools_group.add(&gamemode_row);

    page.add(&paths_group);
    page.add(&tools_group);
    pref_window.add(&page);

    // Save settings when window is closed
    pref_window.connect_close_request(move |_| {
        settings.default_prefix_path = prefix_row.text().to_string();
        settings.default_proton = if proton_row.selected() < proton_list.n_items() {
            proton_list
                .string(proton_row.selected())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Default".to_string())
        } else {
            "Default".to_string()
        };
        settings.global_mangohud = mangohud_row.is_active();
        settings.global_gamemode = gamemode_row.is_active();
        save_settings(&settings);
        glib::Propagation::Proceed
    });

    pref_window.present();
}

// --- ADD GAME DIALOG ---

fn show_add_game_dialog(
    parent: &adw::ApplicationWindow,
    list_box: &gtk4::ListBox,
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
    let path_row = adw::EntryRow::builder().title("Path (.exe)").build();

    let browse_btn = gtk4::Button::builder()
        .label("Browse...")
        .valign(gtk4::Align::Center)
        .build();

    path_row.add_suffix(&browse_btn);

    let game_group = adw::PreferencesGroup::builder().title("Executable").build();
    game_group.add(&title_row);
    game_group.add(&path_row);

    // File chooser for executable
    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let file_dialog = gtk4::FileDialog::builder().title("Select Executable").build();
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    path_row_clone.set_text(&path.to_string_lossy());
                }
            }
        });
    });

    let prefix_row = adw::EntryRow::builder()
        .title("Prefix Path (Leave blank for global)")
        .text(&settings.default_prefix_path)
        .build();

    // Build Proton dropdown
    let proton_strings: Vec<&str> = settings.available_proton_versions.iter().map(|s| s.as_str()).collect();
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&gtk4::StringList::new(&proton_strings))
        .build();

    let env_group = adw::PreferencesGroup::builder()
        .title("Environment")
        .build();
    env_group.add(&prefix_row);
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
    let advanced_group = adw::PreferencesGroup::builder().title("Overrides").build();
    advanced_group.add(&args_row);
    advanced_group.add(&mangohud_row);
    advanced_group.add(&gamemode_row);

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
        };

        // Load existing games, add new one, save back to disk
        let mut games = load_games();
        games.push(new_game);
        save_games(&games);

        // Refresh UI
        populate_game_list(&list_box_clone, &games, &overlay_clone, &parent_clone);

        overlay_clone.add_toast(adw::Toast::new("Game added successfully"));
        dialog_clone_2.destroy();
    });

    dialog.present();
}

// --- EDIT GAME DIALOG ---

fn show_edit_game_dialog(
    parent: &adw::ApplicationWindow,
    list_box: &gtk4::ListBox,
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
        .title("Path (.exe)")
        .text(&game.exe_path)
        .build();

    let browse_btn = gtk4::Button::builder()
        .label("Browse...")
        .valign(gtk4::Align::Center)
        .build();

    path_row.add_suffix(&browse_btn);

    let game_group = adw::PreferencesGroup::builder().title("Executable").build();
    game_group.add(&title_row);
    game_group.add(&path_row);

    // File chooser for executable
    let path_row_clone = path_row.clone();
    let parent_clone = parent.clone();
    browse_btn.connect_clicked(move |_| {
        let file_dialog = gtk4::FileDialog::builder().title("Select Executable").build();
        file_dialog.open(Some(&parent_clone), gio::Cancellable::NONE, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    path_row_clone.set_text(&path.to_string_lossy());
                }
            }
        });
    });

    let prefix_row = adw::EntryRow::builder()
        .title("Prefix Path (Leave blank for global)")
        .text(&game.prefix_path)
        .build();

    // Build Proton dropdown
    let proton_strings: Vec<&str> = settings
        .available_proton_versions
        .iter()
        .map(|s| s.as_str())
        .collect();
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&gtk4::StringList::new(&proton_strings))
        .build();

    // Set selected Proton version
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

    let advanced_group = adw::PreferencesGroup::builder().title("Overrides").build();
    advanced_group.add(&args_row);
    advanced_group.add(&mangohud_row);
    advanced_group.add(&gamemode_row);

    // Add winetricks button
    let winetricks_btn = gtk4::Button::builder()
        .label("Open Winetricks")
        .build();

    let game_prefix = game.prefix_path.clone();
    let overlay_clone_wt = overlay.clone();
    winetricks_btn.connect_clicked(move |_| {
        launch_winetricks(&game_prefix, &overlay_clone_wt);
    });

    let winetricks_group = adw::PreferencesGroup::builder().title("Tools").build();
    winetricks_group.add(&winetricks_btn);

    page.add(&game_group);
    page.add(&env_group);
    page.add(&advanced_group);
    page.add(&winetricks_group);

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
        };

        // Load games, find and replace the edited one
        let mut games = load_games();
        if let Some(pos) = games.iter().position(|g| g.id == game_id) {
            games[pos] = edited_game;
            save_games(&games);

            // Refresh UI
            populate_game_list(&list_box_clone, &games, &overlay_clone, &parent_clone);

            overlay_clone.add_toast(adw::Toast::new("Game updated successfully"));
            dialog_clone_2.destroy();
        } else {
            overlay_clone.add_toast(adw::Toast::new("Error: Game not found"));
        }
    });

    dialog.present();
}

// --- DELETE CONFIRMATION DIALOG ---

fn show_delete_confirmation(
    parent: &adw::ApplicationWindow,
    list_box: &gtk4::ListBox,
    overlay: &adw::ToastOverlay,
    game_id: &str,
) {
    let games = load_games();
    let game = games.iter().find(|g| g.id == game_id);

    let game_title = game.map(|g| g.title.as_str()).unwrap_or("Unknown Game");

    let dialog = adw::AlertDialog::builder()
        .heading("Delete Game?")
        .body(&format!(
            "Are you sure you want to delete '{}'?\n\nThis action cannot be undone.",
            game_title
        ))
        .build();

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let game_id = game_id.to_string();
    let list_box_clone = list_box.clone();
    let overlay_clone = overlay.clone();
    let parent_clone = parent.clone();

    dialog.connect_response(None, move |_, response| {
        if response == "delete" {
            let mut games = load_games();
            if let Some(pos) = games.iter().position(|g| g.id == game_id) {
                let deleted_title = games[pos].title.clone();
                games.remove(pos);
                save_games(&games);

                // Refresh UI
                populate_game_list(&list_box_clone, &games, &overlay_clone, &parent_clone);

                overlay_clone.add_toast(adw::Toast::new(&format!(
                    "'{}' deleted successfully",
                    deleted_title
                )));
            }
        }
    });

    dialog.present(Some(parent));
}

// --- WINETRICKS INTEGRATION ---

fn launch_winetricks(prefix_path: &str, overlay: &adw::ToastOverlay) {
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);

    // Set WINEPREFIX if provided
    if !prefix_path.is_empty() {
        launcher.setenv("WINEPREFIX", prefix_path, true);
    }

    // Try to launch winetricks
    let cmd_args = vec!["winetricks"];
    let os_args: Vec<&std::ffi::OsStr> = cmd_args.iter().map(std::ffi::OsStr::new).collect();

    match launcher.spawn(&os_args) {
        Ok(_) => {
            overlay.add_toast(adw::Toast::new("Launching winetricks..."));
        }
        Err(e) => {
            overlay.add_toast(adw::Toast::new(&format!(
                "Failed to launch winetricks: {}. Make sure winetricks is installed.",
                e
            )));
        }
    }
}
