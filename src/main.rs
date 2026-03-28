use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_ID: &str = "com.github.umu_launcher_gui";

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

// --- FILE IO ---

fn get_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let config_dir = PathBuf::from(format!("{}/.config/umu_gui", home));
    if !config_dir.exists() {
        let _ = fs::create_dir_all(&config_dir);
    }
    config_dir.join("games.json")
}

fn load_games() -> Vec<Game> {
    let path = get_config_path();
    if let Ok(data) = fs::read_to_string(path) {
        serde_json::from_str(&data).unwrap_or_else(|_| Vec::new())
    } else {
        Vec::new()
    }
}

fn save_games(games: &[Game]) {
    let path = get_config_path();
    if let Ok(data) = serde_json::to_string_pretty(games) {
        let _ = fs::write(path, data);
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
        .title("umu-launcher")
        .default_width(700)
        .default_height(600)
        .content(&toolbar_view)
        .build();

    // Load games from disk and populate the list
    let games = load_games();
    populate_game_list(&game_list_box, &games, &toast_overlay);

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

fn populate_game_list(list_box: &gtk4::ListBox, games: &[Game], overlay: &adw::ToastOverlay) {
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

        let play_btn = gtk4::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .css_classes(["suggested-action", "circular"])
            .valign(gtk4::Align::Center)
            .margin_top(8)
            .margin_bottom(8)
            .build();

        // Launch Logic!
        let game_clone = game.clone();
        let overlay_clone = overlay.clone();
        play_btn.connect_clicked(move |_| {
            launch_game(&game_clone, &overlay_clone);
        });

        row.add_prefix(&icon);
        row.add_suffix(&play_btn);
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
    paths_group.add(
        &adw::EntryRow::builder()
            .title("Default Prefix Path")
            .build(),
    );
    paths_group.add(
        &adw::ComboRow::builder()
            .title("Default Proton")
            .model(&gtk4::StringList::new(&[
                "GE-Proton Latest",
                "System Default",
            ]))
            .build(),
    );

    let tools_group = adw::PreferencesGroup::builder()
        .title("Global Environment")
        .build();
    tools_group.add(&adw::SwitchRow::builder().title("MangoHud Overlay").build());
    tools_group.add(&adw::SwitchRow::builder().title("GameMode").build());

    page.add(&paths_group);
    page.add(&tools_group);
    pref_window.add(&page);

    pref_window.present();
}

// --- ADD GAME DIALOG ---

fn show_add_game_dialog(
    parent: &adw::ApplicationWindow,
    list_box: &gtk4::ListBox,
    overlay: &adw::ToastOverlay,
) {
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
    let game_group = adw::PreferencesGroup::builder().title("Executable").build();
    game_group.add(&title_row);
    game_group.add(&path_row);

    let prefix_row = adw::EntryRow::builder()
        .title("Prefix Path (Leave blank for global)")
        .build();
    let proton_row = adw::ComboRow::builder()
        .title("Proton")
        .model(&gtk4::StringList::new(&["Default", "GE-Proton Latest"]))
        .build();
    let env_group = adw::PreferencesGroup::builder()
        .title("Environment")
        .build();
    env_group.add(&prefix_row);
    env_group.add(&proton_row);

    let args_row = adw::EntryRow::builder().title("Launch Arguments").build();
    let mangohud_row = adw::SwitchRow::builder().title("Force MangoHud").build();
    let gamemode_row = adw::SwitchRow::builder().title("Force GameMode").build();
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

    add_btn.connect_clicked(move |_| {
        let title = title_row.text().to_string();
        let exe = path_row.text().to_string();

        if title.is_empty() || exe.is_empty() {
            return; // Simple validation: prevent adding empty games
        }

        let new_game = Game {
            id: uuid::Uuid::new_v4().to_string(), // Requires `uuid` crate, or we can just use the title for now
            title,
            exe_path: exe,
            prefix_path: prefix_row.text().to_string(),
            proton: if proton_row.selected() == 1 {
                "GE-Proton Latest".to_string()
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
        populate_game_list(&list_box_clone, &games, &overlay_clone);

        dialog_clone_2.destroy();
    });

    dialog.present();
}
