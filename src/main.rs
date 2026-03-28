use adw::prelude::*;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::fs;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

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
    #[serde(default)]
    game_wayland: bool,
    #[serde(default)]
    game_wow64: bool,
    #[serde(default)]
    game_ntsync: bool,
    #[serde(default)]
    game_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct GlobalSettings {
    default_prefix_path: String,
    default_proton: String,
    global_mangohud: bool,
    global_gamemode: bool,
    global_wayland: bool,
    global_wow64: bool,
    global_ntsync: bool,
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
    let mut settings: GlobalSettings = if let Ok(data) = fs::read_to_string(&path) {
        toml::from_str(&data).unwrap_or_default()
    } else {
        GlobalSettings::default()
    };
    // Always refresh available Proton versions from the current filesystem state
    let fresh = detect_proton_versions();
    settings.available_proton_versions = fresh.available_proton_versions;
    if settings.default_prefix_path.is_empty() {
        settings.default_prefix_path = fresh.default_prefix_path;
    }
    // If no Proton is installed, download the latest ProtonGE in the background
    if settings.available_proton_versions.len() <= 1 {
        check_or_install_protonge();
    }
    save_settings(&settings);
    settings
}

fn save_settings(settings: &GlobalSettings) {
    let path = get_settings_path();
    if let Ok(data) = toml::to_string_pretty(settings) {
        let _ = fs::write(path, data);
    }
}

// --- UMU LAUNCHER HELPERS ---

static UMU_DOWNLOAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// `true` while the background download thread is actively running.
/// The UI polls this to show/hide the download status banner.
static UMU_DOWNLOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Directory where the umu-launcher zipapp is extracted.
fn get_umu_core_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/leyen/core/umu-launcher", home)
}

/// Full path to the `umu-run` binary inside the extracted zipapp (`umu/umu-run`).
fn get_local_umu_run_path() -> String {
    format!("{}/umu/umu-run", get_umu_core_dir())
}

/// Returns the command / path to use when invoking `umu-run`.
/// Prefers the system-wide binary; falls back to the locally downloaded copy.
fn get_umu_run_path() -> String {
    if std::process::Command::new("which")
        .arg("umu-run")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return "umu-run".to_string();
    }

    let local_path = get_local_umu_run_path();
    if std::path::Path::new(&local_path).exists() {
        return local_path;
    }

    "umu-run".to_string()
}

/// Returns `true` when `umu-run` is actually available (system PATH or local
/// install).  Unlike `get_umu_run_path()` this does not return a fallback
/// string when umu-run is absent.
fn is_umu_run_available() -> bool {
    if std::process::Command::new("which")
        .arg("umu-run")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }
    std::path::Path::new(&get_local_umu_run_path()).exists()
}

/// Checks whether `umu-run` is available.  If it is not found in the system
/// PATH or in the local leyen data directory, spawns a background thread that
/// downloads the latest zipapp release from the umu-launcher GitHub repository
/// and extracts it to `~/.local/share/leyen/core/umu-launcher/`.
fn check_or_install_umu() {
    if is_umu_run_available() {
        return;
    }

    if UMU_DOWNLOAD_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    UMU_DOWNLOADING.store(true, std::sync::atomic::Ordering::Relaxed);

    let umu_core_dir = get_umu_core_dir();

    std::thread::spawn(move || {
        let result = download_and_install_umu(&umu_core_dir);
        if !result {
            // Reset so the next application start can retry.
            UMU_DOWNLOAD_STARTED.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        UMU_DOWNLOADING.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Downloads the latest umu-launcher zipapp tarball and extracts it into
/// `dest_dir`.  Returns `true` on success.
fn download_and_install_umu(dest_dir: &str) -> bool {
    let _ = fs::create_dir_all(dest_dir);

    // Resolve the latest release tag via the GitHub redirect.
    let tag_output = std::process::Command::new("curl")
        .args([
            "-sI",
            "-L",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "https://github.com/Open-Wine-Components/umu-launcher/releases/latest",
        ])
        .output();

    let version = match tag_output {
        Ok(o) if o.status.success() => {
            let url = String::from_utf8_lossy(&o.stdout);
            url.trim()
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        }
        _ => return false,
    };

    if version.is_empty() {
        return false;
    }

    let tarball_name = format!("umu-launcher-{}-zipapp.tar", version);
    let tarball_path = format!("{}/{}", dest_dir, tarball_name);
    let download_url = format!(
        "https://github.com/Open-Wine-Components/umu-launcher/releases/download/{}/{}",
        version, tarball_name
    );

    let ok = std::process::Command::new("curl")
        .args(["-sL", "--fail", "-o", &tarball_path, &download_url])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        let _ = fs::remove_file(&tarball_path);
        return false;
    }

    // Extract: the tarball contains an `umu/` directory with `umu-run` inside.
    let extracted = std::process::Command::new("tar")
        .args(["-xf", &tarball_path, "-C", dest_dir])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let _ = fs::remove_file(&tarball_path);

    if extracted {
        // Ensure the binary is executable.
        let umu_run = format!("{}/umu/umu-run", dest_dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&umu_run) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&umu_run, perms);
            }
        }
    }

    extracted
}

static PROTONGE_DOWNLOAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// If no Proton installation is available, downloads the latest ProtonGE
/// release from GitHub into `~/.local/share/leyen/proton/` in a background
/// thread.  Only one download attempt is made per application lifetime.
fn check_or_install_protonge() {
    if PROTONGE_DOWNLOAD_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let proton_dir = format!("{}/.local/share/leyen/proton", home);

    std::thread::spawn(move || {
        let _ = fs::create_dir_all(&proton_dir);

        // Resolve the latest release tag via the GitHub redirect
        let tag_output = std::process::Command::new("curl")
            .args([
                "-Ls",
                "-o",
                "/dev/null",
                "-w",
                "%{url_effective}",
                "https://github.com/GloriousEggroll/proton-ge-custom/releases/latest",
            ])
            .output();

        let tag = match tag_output {
            Ok(o) if o.status.success() => {
                let url = String::from_utf8_lossy(&o.stdout);
                url.trim()
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string()
            }
            _ => return,
        };

        if tag.is_empty() || !tag.starts_with("GE-Proton") {
            return;
        }

        let tarball = format!("{}.tar.gz", tag);
        let tarball_path = format!("{}/{}", proton_dir, tarball);
        let download_url = format!(
            "https://github.com/GloriousEggroll/proton-ge-custom/releases/download/{}/{}",
            tag, tarball
        );

        let ok = std::process::Command::new("curl")
            .args(["-L", "--fail", "-o", &tarball_path, &download_url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ok {
            let _ = std::process::Command::new("tar")
                .args(["-xzf", &tarball_path, "-C", &proton_dir])
                .status();
            let _ = fs::remove_file(&tarball_path);
        }
    });
}

/// Resolves a Proton value stored in a game config (which may be a full path
/// or, for configs written before the path-storage change, just a directory
/// name) into the full path expected by `PROTONPATH`.
/// Returns `None` when the value represents the "Default" / unset state.
fn resolve_proton_path(proton: &str) -> Option<String> {
    if proton.is_empty() || proton == "Default" {
        return None;
    }

    // Already a full path
    if proton.starts_with('/') {
        return Some(proton.to_string());
    }

    // Backward-compat: resolve a bare directory name to its full path
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let candidates = [
        format!("{}/.local/share/leyen/proton/{}", home, proton),
        format!("{}/.steam/steam/compatibilitytools.d/{}", home, proton),
        format!("{}/.steam/steam/steamapps/common/{}", home, proton),
    ];
    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return Some(path.clone());
        }
    }

    Some(proton.to_string())
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
                    versions.push(entry.path().to_string_lossy().to_string());
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
                    versions.push(entry.path().to_string_lossy().to_string());
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
                            versions.push(entry.path().to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }

    let default_prefix_path = format!("{}/.local/share/leyen/prefixes/default", home);
    let default_prefix_dir = PathBuf::from(&default_prefix_path);
    if !default_prefix_dir.exists() {
        let _ = fs::create_dir_all(&default_prefix_dir);
    }

    GlobalSettings {
        default_prefix_path,
        default_proton: "Default".to_string(),
        global_mangohud: false,
        global_gamemode: false,
        global_wayland: false,
        global_wow64: false,
        global_ntsync: false,
        available_proton_versions: versions,
    }
}

// --- MAIN UI ---

fn main() -> glib::ExitCode {
    check_or_install_umu();
    ensure_winetricks_verbs_cached();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    // Hide the built-in pencil/edit indicator that AdwEntryRow shows by default
    let css = gtk4::CssProvider::new();
    css.load_from_string(
        "image.edit-icon { min-width: 0px; min-height: 0px; \
         margin: 0px; padding: 0px; opacity: 0; }",
    );
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

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

    let game_list_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .hexpand(true)
        .build();

    let empty_state = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .spacing(6)
        .build();

    let empty_label = gtk4::Label::builder()
        .label("No games added yet")
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .css_classes(["title-3"])
        .build();

    let empty_hint = gtk4::Label::builder()
        .label("Add a game to see it listed here.")
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .css_classes(["dim-label"])
        .build();

    empty_state.append(&empty_label);
    empty_state.append(&empty_hint);

    clamp.set_child(Some(&game_list_box));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&clamp)
        .build();

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&scroll));

    // Banner shown while umu-launcher is being downloaded in the background.
    let download_banner = adw::Banner::builder()
        .title("Downloading umu-launcher… Please wait before starting games.")
        .revealed(UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed))
        .build();
    toolbar_view.add_top_bar(&download_banner);

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
    populate_game_list(
        &game_list_box,
        &empty_state,
        &games,
        &toast_overlay,
        &window,
    );

    // Poll every 2 seconds; hide the banner and show a toast once the download
    // completes (or if it was never needed).
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        let banner_clone = download_banner.clone();
        let overlay_clone = toast_overlay.clone();
        glib::timeout_add_seconds_local(2, move || {
            if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
                return glib::ControlFlow::Continue;
            }
            banner_clone.set_revealed(false);
            if is_umu_run_available() {
                overlay_clone.add_toast(adw::Toast::new("umu-launcher downloaded. Ready to play!"));
            } else {
                overlay_clone.add_toast(adw::Toast::new(
                    "Failed to download umu-launcher. Check your internet connection.",
                ));
            }
            glib::ControlFlow::Break
        });
    }

    /* --- EVENT HANDLERS --- */

    let window_clone = window.clone();
    settings_btn.connect_clicked(move |_| {
        show_global_settings(&window_clone);
    });

    let window_clone_2 = window.clone();
    let list_box_clone = game_list_box.clone();
    let empty_state_clone = empty_state.clone();
    let overlay_clone = toast_overlay.clone();
    add_btn.connect_clicked(move |_| {
        show_add_game_dialog(
            &window_clone_2,
            &list_box_clone,
            &empty_state_clone,
            &overlay_clone,
        );
    });

    window.present();
}

// --- DYNAMIC UI GENERATOR ---

fn populate_game_list(
    list_box: &gtk4::Box,
    empty_state: &gtk4::Box,
    games: &[Game],
    overlay: &adw::ToastOverlay,
    window: &adw::ApplicationWindow,
) {
    // Clear existing children
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    if games.is_empty() {
        list_box.append(empty_state);
        return;
    }

    for game in games {
        let card = gtk4::Frame::builder()
            .hexpand(true)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        card.add_css_class("card");

        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        let icon = gtk4::Image::builder()
            .icon_name("application-x-executable-symbolic")
            .pixel_size(48)
            .valign(gtk4::Align::Start)
            .build();

        let info_column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .build();

        let title_label = gtk4::Label::builder()
            .label(&game.title)
            .xalign(0.0)
            .css_classes(["title-4"])
            .build();

        let path_label = gtk4::Label::builder()
            .label(&game.exe_path)
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();

        info_column.append(&title_label);
        info_column.append(&path_label);

        // Button box for actions
        let button_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk4::Align::Center)
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
        let empty_state_clone = empty_state.clone();
        let overlay_clone = overlay.clone();
        let window_clone = window.clone();
        edit_btn.connect_clicked(move |_| {
            show_edit_game_dialog(
                &window_clone,
                &list_box_clone,
                &empty_state_clone,
                &overlay_clone,
                &game_clone,
            );
        });

        // Delete Logic
        let game_id = game.id.clone();
        let list_box_clone = list_box.clone();
        let empty_state_clone = empty_state.clone();
        let overlay_clone = overlay.clone();
        let window_clone = window.clone();
        delete_btn.connect_clicked(move |_| {
            show_delete_confirmation(
                &window_clone,
                &list_box_clone,
                &empty_state_clone,
                &overlay_clone,
                &game_id,
            );
        });

        button_box.append(&edit_btn);
        button_box.append(&delete_btn);
        button_box.append(&play_btn);

        content.append(&icon);
        content.append(&info_column);
        content.append(&button_box);

        card.set_child(Some(&content));
        list_box.append(&card);
    }
}

// --- CORE LAUNCH LOGIC ---

fn launch_game(game: &Game, overlay: &adw::ToastOverlay) {
    // Block launch while umu-launcher is being downloaded.
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }

    // Block launch if umu-run is simply not available.
    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return;
    }

    let settings = load_settings();
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);

    // Wine prefix
    if !game.prefix_path.is_empty() {
        launcher.setenv("WINEPREFIX", &game.prefix_path, true);
    }

    // Game ID for umu-run
    if !game.game_id.is_empty() {
        launcher.setenv("GAMEID", &game.game_id, true);
    }

    // Proton path (resolve backward-compat names to full paths)
    if let Some(proton_path) = resolve_proton_path(&game.proton) {
        launcher.setenv("PROTONPATH", &proton_path, true);
    }

    // MangoHud – per-game flag OR global setting
    if game.force_mangohud || settings.global_mangohud {
        launcher.setenv("MANGOHUD", "1", true);
    }

    // Wayland: per-game override OR global setting
    launcher.setenv(
        "PROTON_ENABLE_WAYLAND",
        if game.game_wayland || settings.global_wayland {
            "1"
        } else {
            "0"
        },
        true,
    );

    // WoW64: per-game override OR global setting
    launcher.setenv(
        "PROTON_USE_WOW64",
        if game.game_wow64 || settings.global_wow64 {
            "1"
        } else {
            "0"
        },
        true,
    );

    // NTSync: per-game override OR global setting
    let ntsync_val = if game.game_ntsync || settings.global_ntsync {
        "1"
    } else {
        "0"
    };
    launcher.setenv("PROTON_USE_NTSYNC", ntsync_val, true);
    launcher.setenv("WINENTSYNC", ntsync_val, true);

    // Build the argument list, honouring Steam-style %command% substitution.
    // If the launch-args field contains "%command%", everything before it is
    // examined token by token:
    //   • KEY=VALUE tokens are applied as environment variables
    //   • other tokens (e.g. "gamemoderun") are prepended as command wrappers
    // Everything after "%command%" is appended after the executable path.
    // Without "%command%", extra args are appended after the executable path as before.
    let umu = get_umu_run_path();
    let mut cmd_args: Vec<String> = Vec::new();

    if game.launch_args.contains("%command%") {
        let parts: Vec<&str> = game.launch_args.splitn(2, "%command%").collect();
        let postfix: Vec<String> = parts
            .get(1)
            .unwrap_or(&"")
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        // Classify prefix tokens as env vars or command wrappers
        let mut cmd_wrappers: Vec<String> = Vec::new();
        for token in parts[0].split_whitespace() {
            // A token is an env var if it looks like KEY=VALUE (no spaces, contains '=')
            if let Some(eq_pos) = token.find('=') {
                let key = &token[..eq_pos];
                let val = &token[eq_pos + 1..];
                // Only treat as env var if the key is a valid identifier (no special chars)
                if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    launcher.setenv(key, val, true);
                    continue;
                }
            }
            cmd_wrappers.push(token.to_string());
        }

        if game.force_gamemode || settings.global_gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.extend(cmd_wrappers);
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        cmd_args.extend(postfix);
    } else {
        if game.force_gamemode || settings.global_gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        if !game.launch_args.is_empty() {
            cmd_args.extend(game.launch_args.split_whitespace().map(|s| s.to_string()));
        }
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
    let settings = load_settings();

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

    // Build Proton dropdown – display basenames, store full paths via index
    let available_versions = settings.available_proton_versions.clone();
    let display_names: Vec<String> = available_versions
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
    let proton_list = gtk4::StringList::new(&[]);
    for name in &display_names {
        proton_list.append(name);
    }

    let proton_row = adw::ComboRow::builder()
        .title("Default Proton")
        .model(&proton_list)
        .build();

    // Set selected index by matching full path
    if let Some(pos) = available_versions
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
        .title("MangoHud")
        .active(settings.global_mangohud)
        .build();

    let gamemode_row = adw::SwitchRow::builder()
        .title("GameMode")
        .active(settings.global_gamemode)
        .build();

    let wayland_row = adw::SwitchRow::builder()
        .title("Wayland")
        .active(settings.global_wayland)
        .build();

    let wow64_row = adw::SwitchRow::builder()
        .title("WoW64")
        .active(settings.global_wow64)
        .build();

    let ntsync_row = adw::SwitchRow::builder()
        .title("NTSync")
        .active(settings.global_ntsync)
        .build();

    tools_group.add(&mangohud_row);
    tools_group.add(&gamemode_row);
    tools_group.add(&wayland_row);
    tools_group.add(&wow64_row);
    tools_group.add(&ntsync_row);

    page.add(&paths_group);
    page.add(&tools_group);
    pref_window.add(&page);

    // Save settings when window is closed
    pref_window.connect_close_request(move |_| {
        let updated_settings = GlobalSettings {
            default_prefix_path: prefix_row.text().to_string(),
            default_proton: if (proton_row.selected() as usize) < available_versions.len() {
                available_versions[proton_row.selected() as usize].clone()
            } else {
                "Default".to_string()
            },
            global_mangohud: mangohud_row.is_active(),
            global_gamemode: gamemode_row.is_active(),
            global_wayland: wayland_row.is_active(),
            global_wow64: wow64_row.is_active(),
            global_ntsync: ntsync_row.is_active(),
            available_proton_versions: available_versions.clone(),
        };
        save_settings(&updated_settings);
        glib::Propagation::Proceed
    });

    pref_window.present();
}

// --- ADD GAME DIALOG ---

fn show_add_game_dialog(
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

fn show_edit_game_dialog(
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

    // Add winetricks button
    let winetricks_btn = gtk4::Button::builder().label("Open Winetricks").build();

    let game_prefix = game.prefix_path.clone();
    let game_proton = resolve_proton_path(&game.proton).unwrap_or_default();
    let overlay_clone_wt = overlay.clone();
    let dialog_parent = parent.clone();
    winetricks_btn.connect_clicked(move |_| {
        show_winetricks_dialog(
            &dialog_parent,
            &game_prefix,
            &game_proton,
            &overlay_clone_wt,
        );
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

fn show_delete_confirmation(
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

// --- WINETRICKS VERB LIST CACHE ---

const WINETRICKS_VERBS_URL: &str =
    "https://raw.githubusercontent.com/Winetricks/winetricks/master/files/verbs/all.txt";
const WINETRICKS_PROGRESS_PULSE_INTERVAL_MS: u64 = 120;

fn get_winetricks_verbs_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/leyen/core/winetricks/all.txt", home)
}

fn resolve_winetricks_prefix_path(prefix_path: &str) -> String {
    if !prefix_path.is_empty() {
        return prefix_path.to_string();
    }
    let from_settings = load_settings().default_prefix_path;
    if !from_settings.is_empty() {
        from_settings
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{}/.local/share/leyen/prefixes/default", home)
    }
}

fn get_prefix_winetricks_info_dir(prefix_path: &str) -> PathBuf {
    PathBuf::from(prefix_path).join(".leyen/winetricks")
}

fn now_unix_timestamp() -> String {
    match SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
    {
        Ok(ts) => ts.to_string(),
        Err(e) => {
            eprintln!("Failed to compute unix timestamp for winetricks log: {}", e);
            "unknown-ts".to_string()
        }
    }
}

fn write_winetricks_log(prefix_path: &str, message: &str) {
    let info_dir = get_prefix_winetricks_info_dir(prefix_path);
    if let Err(e) = fs::create_dir_all(&info_dir) {
        eprintln!(
            "Failed to create winetricks log dir '{}': {}",
            info_dir.display(),
            e
        );
        return;
    }
    let log_path = info_dir.join("install.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        if let Err(e) = std::io::Write::write_all(
            &mut file,
            format!("[{}] {}\n", now_unix_timestamp(), message).as_bytes(),
        ) {
            eprintln!(
                "Failed to write winetricks log entry '{}': {}",
                log_path.display(),
                e
            );
        }
    } else {
        eprintln!("Failed to open winetricks log '{}'", log_path.display());
    }
}

fn read_installed_winetricks_components(prefix_path: &str) -> std::collections::HashSet<String> {
    let components_path =
        get_prefix_winetricks_info_dir(prefix_path).join("installed_components.txt");
    fs::read_to_string(&components_path)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn write_installed_winetricks_components(prefix_path: &str, verbs: &[String]) {
    let info_dir = get_prefix_winetricks_info_dir(prefix_path);
    if let Err(e) = fs::create_dir_all(&info_dir) {
        eprintln!(
            "Failed to create winetricks info dir '{}': {}",
            info_dir.display(),
            e
        );
        return;
    }
    // Merge with whatever is already on disk (reuses the shared reader).
    let mut all_components: std::collections::BTreeSet<String> =
        read_installed_winetricks_components(prefix_path)
            .into_iter()
            .collect();
    for verb in verbs {
        all_components.insert(verb.clone());
    }
    let components_path = info_dir.join("installed_components.txt");
    let data = all_components.into_iter().collect::<Vec<_>>().join("\n");
    if let Err(e) = fs::write(&components_path, format!("{}\n", data)) {
        eprintln!(
            "Failed to write installed winetricks components '{}': {}",
            components_path.display(),
            e
        );
    }
}

/// Downloads the winetricks verb list in the background if it is not yet cached.
fn ensure_winetricks_verbs_cached() {
    let cache_path = get_winetricks_verbs_path();
    if std::path::Path::new(&cache_path).exists() {
        return;
    }
    std::thread::spawn(move || {
        if let Some(parent) = std::path::Path::new(&cache_path).parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = std::process::Command::new("curl")
            .args(["-sL", "--fail", "-o", &cache_path, WINETRICKS_VERBS_URL])
            .status();
    });
}

#[derive(Clone)]
struct WinetricksVerb {
    category: String,
    verb: String,
    description: String,
}

/// Parses the `all.txt` format:
///   `===== category =====`
///   `verb_name    Description text [optional tags]`
fn parse_winetricks_verbs(content: &str) -> Vec<WinetricksVerb> {
    let mut result = Vec::new();
    let mut current_cat = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("=====") {
            current_cat = trimmed.trim_matches('=').trim().to_string();
        } else if !trimmed.is_empty() && !current_cat.is_empty() && current_cat != "prefix" {
            // Verb lines: verb followed by whitespace then description.
            // Some lines use a single space, others use many — split on first space.
            let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
            if parts.len() == 2 {
                let verb = parts[0].trim().to_string();
                let description = parts[1].trim().to_string();
                if !verb.is_empty() {
                    result.push(WinetricksVerb {
                        category: current_cat.clone(),
                        verb,
                        description,
                    });
                }
            }
        }
    }
    result
}

fn load_winetricks_verbs() -> Vec<WinetricksVerb> {
    match fs::read_to_string(get_winetricks_verbs_path()) {
        Ok(content) => parse_winetricks_verbs(&content),
        Err(_) => Vec::new(),
    }
}

fn category_display_name(cat: &str) -> &str {
    match cat {
        "dlls" => "DLLs",
        "fonts" => "Fonts",
        "apps" => "Applications",
        "benchmarks" => "Benchmarks",
        "settings" => "Settings",
        _ => cat,
    }
}

/// Escapes the five XML/Pango special characters so the text can be safely
/// passed to widgets that interpret their label/subtitle as Pango markup.
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// Display order for categories.
const CATEGORY_ORDER: &[&str] = &["dlls", "fonts", "apps", "settings", "benchmarks"];

// --- WINETRICKS INTEGRATION ---

/// Opens a verb-browser dialog: search bar + grouped, checkable verb list, then
/// calls `launch_winetricks` with the user's selection.
fn show_winetricks_dialog(
    parent: &adw::ApplicationWindow,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
) {
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }

    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return;
    }

    // Load verbs from disk cache.
    let verbs = load_winetricks_verbs();
    if verbs.is_empty() {
        // Cache not ready yet — trigger download and ask user to retry.
        ensure_winetricks_verbs_cached();
        overlay.add_toast(adw::Toast::new(
            "Downloading verb list… Please re-open Winetricks in a moment.",
        ));
        return;
    }

    // Resolve prefix now so we can load the installed-components list for
    // this prefix and display it in the dialog.
    let resolved_prefix = resolve_winetricks_prefix_path(prefix_path);
    let installed = read_installed_winetricks_components(&resolved_prefix);

    // ── Dialog window ──────────────────────────────────────────────────────
    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(480)
        .default_height(600)
        .destroy_with_parent(true)
        .build();

    let installed_count = installed.len();
    let subtitle = if installed_count == 0 {
        "Install Windows components".to_string()
    } else {
        format!("{} component(s) already installed", installed_count)
    };
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Winetricks", &subtitle))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();

    let cancel_btn = gtk4::Button::builder().label("Cancel").build();
    let run_btn = gtk4::Button::builder()
        .label("Install")
        .css_classes(["suggested-action"])
        .build();

    header.pack_start(&cancel_btn);
    header.pack_end(&run_btn);

    let progress_bar = gtk4::ProgressBar::builder()
        .visible(false)
        .hexpand(true)
        .build();

    // ── Search entry (pinned above the scroll) ─────────────────────────────
    let search_entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search verbs…")
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    // ── Scrollable content: one PreferencesGroup per category ─────────────
    let clamp = adw::Clamp::builder().margin_top(4).margin_bottom(8).build();

    let verb_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    clamp.set_child(Some(&verb_box));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();

    // ── Selected-verbs footer ──────────────────────────────────────────────
    let footer_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .margin_top(6)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let footer_caption = gtk4::Label::builder()
        .label("Selected:")
        .css_classes(["dim-label"])
        .build();

    let footer_label = gtk4::Label::builder()
        .label("none")
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .hexpand(true)
        .build();

    footer_box.append(&footer_caption);
    footer_box.append(&footer_label);

    // ── Outer layout ───────────────────────────────────────────────────────
    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    content_box.append(&search_entry);
    content_box.append(&scroll);

    let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
    content_box.append(&sep);
    content_box.append(&footer_box);

    let outer = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    outer.append(&progress_bar);
    outer.append(&content_box);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&outer));
    dialog.set_content(Some(&toolbar_view));

    // ── Build grouped rows ─────────────────────────────────────────────────
    // selected: shared mutable set of chosen verb names
    let selected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    // groups: (group_widget, Vec<(row, verb_name)>) — used for search filtering
    let mut groups: Vec<(adw::PreferencesGroup, Vec<(adw::ActionRow, String)>)> = Vec::new();

    // Collect verbs grouped and ordered by CATEGORY_ORDER.
    let mut ordered_cats: Vec<&str> = CATEGORY_ORDER.to_vec();
    // Append any categories not in CATEGORY_ORDER (forward-compat).
    for v in &verbs {
        if !ordered_cats.contains(&v.category.as_str()) {
            ordered_cats.push(&v.category);
        }
    }

    for cat in ordered_cats {
        let cat_verbs: Vec<&WinetricksVerb> = verbs.iter().filter(|v| v.category == cat).collect();
        if cat_verbs.is_empty() {
            continue;
        }

        let group = adw::PreferencesGroup::builder()
            .title(category_display_name(cat))
            .build();

        let mut group_rows: Vec<(adw::ActionRow, String)> = Vec::new();

        for wv in cat_verbs {
            let row = adw::ActionRow::builder()
                .title(&wv.verb)
                .subtitle(&escape_markup(&wv.description))
                .activatable(true)
                .build();

            // If this verb is already installed in the prefix, add a badge.
            if installed.contains(&wv.verb) {
                let badge = gtk4::Label::builder()
                    .label("✓ Installed")
                    .css_classes(["success", "caption"])
                    .valign(gtk4::Align::Center)
                    .build();
                row.add_suffix(&badge);
            }

            let check = gtk4::CheckButton::builder()
                .valign(gtk4::Align::Center)
                .build();
            row.add_suffix(&check);
            row.set_activatable_widget(Some(&check));

            // Checkbox toggled → update selected list + footer label.
            let selected_clone = Rc::clone(&selected);
            let footer_clone = footer_label.clone();
            let verb_name = wv.verb.clone();
            check.connect_toggled(move |b| {
                let mut sel = selected_clone.borrow_mut();
                if b.is_active() {
                    if !sel.contains(&verb_name) {
                        sel.push(verb_name.clone());
                    }
                } else {
                    sel.retain(|v| v != &verb_name);
                }
                footer_clone.set_text(
                    if sel.is_empty() {
                        "none".to_string()
                    } else {
                        format!("{} verb(s) selected", sel.len())
                    }
                    .as_str(),
                );
            });

            group.add(&row);
            group_rows.push((row, wv.verb.clone()));
        }

        verb_box.append(&group);
        groups.push((group, group_rows));
    }

    // ── Search filtering ───────────────────────────────────────────────────
    let groups_for_search = groups.clone();
    search_entry.connect_search_changed(move |entry| {
        let query = entry.text().to_lowercase();
        for (group, rows) in &groups_for_search {
            let mut any_visible = false;
            for (row, verb) in rows {
                let visible = if query.is_empty() {
                    true
                } else {
                    verb.to_lowercase().contains(&query)
                        || row
                            .subtitle()
                            .map(|s| s.to_lowercase().contains(&query))
                            .unwrap_or(false)
                };
                row.set_visible(visible);
                if visible {
                    any_visible = true;
                }
            }
            group.set_visible(query.is_empty() || any_visible);
        }
    });

    // ── Button actions ─────────────────────────────────────────────────────
    let dialog_cancel = dialog.clone();
    cancel_btn.connect_clicked(move |_| dialog_cancel.destroy());

    let prefix_owned = prefix_path.to_string();
    let proton_owned = proton_path.to_string();
    let overlay_clone = overlay.clone();
    let run_btn_run = run_btn.clone();
    let cancel_btn_run = cancel_btn.clone();
    let content_box_run = content_box.clone();
    let progress_bar_run = progress_bar.clone();
    let progress_pulse_source: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let progress_pulse_source_run = progress_pulse_source.clone();
    let progress_pulse_source_finish = progress_pulse_source.clone();
    let dialog_run = dialog.clone();
    run_btn.connect_clicked(move |_| {
        let sel = selected.borrow();
        if sel.is_empty() {
            overlay_clone.add_toast(adw::Toast::new(
                "Please select at least one verb to install.",
            ));
            return;
        }
        let selected_verbs: Vec<String> = sel.iter().cloned().collect();
        let verbs_str = selected_verbs.join(" ");
        let verb_count = selected_verbs.len();
        drop(sel);

        content_box_run.set_sensitive(false);
        run_btn_run.set_sensitive(false);
        cancel_btn_run.set_sensitive(false);
        progress_bar_run.set_visible(true);
        progress_bar_run.pulse();
        if let Some(source_id) = progress_pulse_source_run.borrow_mut().take() {
            source_id.remove();
        }
        let progress_bar_anim = progress_bar_run.clone();
        let source_id = glib::timeout_add_local(
            std::time::Duration::from_millis(WINETRICKS_PROGRESS_PULSE_INTERVAL_MS),
            move || {
                if progress_bar_anim.is_visible() {
                    progress_bar_anim.pulse();
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            },
        );
        *progress_pulse_source_run.borrow_mut() = Some(source_id);
        dialog_run.set_deletable(false);

        let content_box_finish = content_box_run.clone();
        let run_btn_finish = run_btn_run.clone();
        let cancel_btn_finish = cancel_btn_run.clone();
        let progress_bar_finish = progress_bar_run.clone();
        let progress_pulse_source_finish_local = progress_pulse_source_finish.clone();
        let dialog_finish = dialog_run.clone();
        launch_winetricks(
            &prefix_owned,
            &proton_owned,
            &verbs_str,
            verb_count,
            &overlay_clone,
            &selected_verbs,
            move |success| {
                if let Some(source_id) = progress_pulse_source_finish_local.borrow_mut().take() {
                    source_id.remove();
                }
                content_box_finish.set_sensitive(true);
                run_btn_finish.set_sensitive(true);
                cancel_btn_finish.set_sensitive(true);
                progress_bar_finish.set_visible(false);
                dialog_finish.set_deletable(true);
                if success {
                    dialog_finish.destroy();
                }
            },
        );
    });

    dialog.present();
}

/// Launches `umu-run winetricks <verbs…>` for the given Wine prefix and Proton build.
/// Shows a persistent "Installing…" toast while the process runs, then a
/// completion/error toast once it exits.
fn launch_winetricks(
    prefix_path: &str,
    proton_path: &str,
    verbs: &str,
    verb_count: usize,
    overlay: &adw::ToastOverlay,
    selected_verbs: &[String],
    on_finish: impl FnOnce(bool) + 'static,
) {
    let umu = get_umu_run_path();
    // Do not pipe stdout/stderr — winetricks/wine output can contain non-UTF-8
    // bytes and an unread pipe buffer would deadlock the subprocess.  Inherit
    // the parent's file descriptors so output flows naturally to the terminal.
    let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);

    let resolved_prefix_path = resolve_winetricks_prefix_path(prefix_path);
    if !resolved_prefix_path.is_empty() {
        launcher.setenv("WINEPREFIX", &resolved_prefix_path, true);
    }

    if !proton_path.is_empty() {
        launcher.setenv("PROTONPATH", proton_path, true);
    }

    let mut cmd_args = vec![umu, "winetricks".to_string()];
    cmd_args.extend(verbs.split_whitespace().map(|s| s.to_string()));

    let os_args: Vec<&std::ffi::OsStr> = cmd_args.iter().map(std::ffi::OsStr::new).collect();

    match launcher.spawn(&os_args) {
        Ok(subprocess) => {
            let progress_toast = adw::Toast::builder()
                .title(&format!("Installing {} verb(s)…", verb_count))
                .timeout(0)
                .build();
            overlay.add_toast(progress_toast.clone());
            let overlay_clone = overlay.clone();
            let prefix_for_log = resolved_prefix_path.clone();
            let verbs_for_log = selected_verbs.to_vec();
            write_winetricks_log(
                &prefix_for_log,
                &format!("Starting winetricks install: {}", verbs_for_log.join(" ")),
            );
            // wait_check_async fires the callback with Ok(()) when the process
            // exits with code 0, Err otherwise — no output buffering needed.
            subprocess.wait_check_async(None::<&gio::Cancellable>, move |result| {
                let finished_ok = result.is_ok();
                if finished_ok {
                    write_installed_winetricks_components(&prefix_for_log, &verbs_for_log);
                }
                write_winetricks_log(
                    &prefix_for_log,
                    &format!(
                        "Finished winetricks install [{}]: {}",
                        if finished_ok { "success" } else { "error" },
                        verbs_for_log.join(" ")
                    ),
                );
                progress_toast.dismiss();
                let msg = if finished_ok {
                    format!("{} verb(s) installed successfully.", verb_count)
                } else {
                    "Winetricks installation encountered errors.".to_string()
                };
                overlay_clone.add_toast(adw::Toast::new(&msg));
                on_finish(finished_ok);
            });
        }
        Err(e) => {
            overlay.add_toast(adw::Toast::new(&format!(
                "Failed to launch winetricks: {}",
                e
            )));
            write_winetricks_log(
                &resolved_prefix_path,
                &format!("Failed to launch winetricks: {}", e),
            );
            on_finish(false);
        }
    }
}
