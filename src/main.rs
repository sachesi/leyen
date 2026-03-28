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

/// Directory where umu-run stores the Steam Linux Runtime (steamrt3).
/// Deleting this directory forces umu-run to re-download a clean runtime on
/// the next launch — useful when pressure-vessel-wrap fails due to a
/// corrupted or incomplete sniper_platform installation.
fn get_umu_runtime_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/umu/steamrt3", home)
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
    let overlay_for_settings = toast_overlay.clone();
    settings_btn.connect_clicked(move |_| {
        show_global_settings(&window_clone, &overlay_for_settings);
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

fn show_global_settings(parent: &adw::ApplicationWindow, overlay: &adw::ToastOverlay) {
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

    // ── Maintenance ────────────────────────────────────────────────────────
    let maintenance_group = adw::PreferencesGroup::builder()
        .title("Maintenance")
        .description("Use these actions to fix runtime issues.")
        .build();

    let reset_btn = gtk4::Button::builder()
        .label("Reset umu Runtime")
        .css_classes(["destructive-action"])
        .halign(gtk4::Align::Start)
        .build();

    let pref_window_for_reset = pref_window.clone();
    let overlay_for_reset = overlay.clone();
    reset_btn.connect_clicked(move |_| {
        let confirm = gtk4::AlertDialog::builder()
            .message("Reset umu Runtime?")
            .detail(
                "This deletes the Steam Linux Runtime (steamrt3) directory. \
umu-launcher will re-download a clean copy the next time a dependency is installed.\n\n\
Use this to fix \"pressure-vessel-wrap\" errors during dependency installations.",
            )
            .buttons(vec!["Cancel".to_string(), "Reset".to_string()])
            .cancel_button(0)
            .default_button(0)
            .build();

        let overlay_clone = overlay_for_reset.clone();
        confirm.choose(
            Some(&pref_window_for_reset),
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(1) = result {
                    let runtime_dir = get_umu_runtime_dir();
                    match fs::remove_dir_all(&runtime_dir) {
                        Ok(_) => {
                            overlay_clone.add_toast(adw::Toast::new(
                                "umu runtime reset. Re-run any dependency install to download a fresh copy.",
                            ));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            overlay_clone.add_toast(adw::Toast::new(
                                "umu runtime directory not found — nothing to reset.",
                            ));
                        }
                        Err(e) => {
                            overlay_clone.add_toast(adw::Toast::new(&format!(
                                "Failed to reset umu runtime: {}",
                                e
                            )));
                        }
                    }
                }
            },
        );
    });

    maintenance_group.add(&reset_btn);
    page.add(&maintenance_group);

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


// ─────────────────────────────────────────────────────────────────────────────
// DEPENDENCY MANAGEMENT SYSTEM
// ─────────────────────────────────────────────────────────────────────────────

// ── Data Structures ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct DepCatalogEntry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    category: &'static str,
}

#[derive(Clone)]
enum DepStepAction {
    DownloadFile {
        url: &'static str,
        file_name: &'static str,
    },
    RunExe {
        file_name: &'static str,
        args: &'static str,
        extra_env: &'static str,
    },
    RunMsi {
        file_name: &'static str,
        args: &'static str,
    },
    OverrideDlls {
        dlls: &'static str,
        override_type: &'static str,
    },
    RegisterDll {
        dll: &'static str,
    },
    /// Extract a tar.gz or tar.zst archive from cache into a sub-directory of
    /// cache.  The top-level directory inside the archive is stripped so that
    /// the archive contents land directly in `dest_subdir`.
    ExtractArchive {
        archive_name: &'static str,
        dest_subdir: &'static str,
    },
    /// Copy compiled DLLs from an extracted directory into the Wine prefix
    /// system directories (system32 or syswow64).
    CopyDllsToPrefix {
        src_subdir: &'static str,
        dlls: &'static str,
        wine_dir: &'static str,
    },
    /// Run a winetricks verb inside the prefix using `umu-run winetricks`.
    RunWinetricks {
        verb: &'static str,
    },
}

#[derive(Clone)]
struct DepStep {
    description: &'static str,
    action: DepStepAction,
}

// ── Progress messages (sent from background thread → GTK main loop) ──────────

enum DepInstallMsg {
    Progress {
        step: usize,
        total: usize,
        description: String,
    },
    Done,
    Failed(String),
}

// ── Built-in catalog ─────────────────────────────────────────────────────────

const DEP_CATALOG: &[DepCatalogEntry] = &[
    // ── Runtime ──────────────────────────────────────────────────────────────
    DepCatalogEntry {
        id: "vcredist2022",
        name: "Visual C++ 2015-2022 Redistributable",
        description: "Microsoft Visual C++ runtime libraries required by most modern Windows applications",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "vcredist2013",
        name: "Visual C++ 2013 Redistributable",
        description: "Microsoft Visual C++ 2013 runtime libraries — required by many older Windows games and apps",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "vcredist2010",
        name: "Visual C++ 2010 SP1 Redistributable",
        description: "Microsoft Visual C++ 2010 SP1 runtime libraries — required by games built with MSVC 2010",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "vcredist2008",
        name: "Visual C++ 2008 SP1 Redistributable",
        description: "Microsoft Visual C++ 2008 SP1 runtime libraries — required by legacy Windows software",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "dotnet48",
        name: ".NET Framework 4.8",
        description: "Microsoft .NET Framework 4.8 — required by many Windows desktop applications",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "dotnet40",
        name: ".NET Framework 4.0",
        description: "Microsoft .NET Framework 4.0 — required by older .NET applications that predate 4.5+",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "dotnet35",
        name: ".NET Framework 3.5 SP1",
        description: "Microsoft .NET Framework 3.5 SP1 — required by many older .NET applications and games",
        category: "Runtime",
    },
    DepCatalogEntry {
        id: "xna40",
        name: "XNA Framework 4.0",
        description: "Microsoft XNA Framework 4.0 Redistributable — required to run XNA-based games",
        category: "Runtime",
    },
    // ── DirectX ──────────────────────────────────────────────────────────────
    DepCatalogEntry {
        id: "directx",
        name: "DirectX End-User Runtime (June 2010)",
        description: "Installs legacy DirectX 9/10 components (d3dx9, d3dx10, xinput, etc.) required by older games",
        category: "DirectX",
    },
    DepCatalogEntry {
        id: "d3dcompiler47",
        name: "D3D Compiler 47",
        description: "D3D shader compiler DLL required by DXVK and many modern Direct3D applications",
        category: "DirectX",
    },
    DepCatalogEntry {
        id: "dxvk",
        name: "DXVK 2.4",
        description: "Vulkan-based Direct3D 9/10/11 implementation — improves performance for DX9-DX11 games. Note: Proton bundles DXVK automatically",
        category: "DirectX",
    },
    DepCatalogEntry {
        id: "vkd3d",
        name: "VKD3D-Proton 2.12",
        description: "Vulkan-based Direct3D 12 implementation — enables DX12 games on Linux. Note: Proton bundles VKD3D-Proton automatically",
        category: "DirectX",
    },
    // ── Media ─────────────────────────────────────────────────────────────────
    DepCatalogEntry {
        id: "xact",
        name: "XACT Audio",
        description: "Microsoft Cross-Platform Audio Creation Tool runtime — required by many older DirectX games for audio",
        category: "Media",
    },
    DepCatalogEntry {
        id: "wmp11",
        name: "Windows Media Player 11",
        description: "Windows Media Player 11 codecs and runtime — required by games and apps that use Windows media APIs",
        category: "Media",
    },
    // ── Wine Components ───────────────────────────────────────────────────────
    DepCatalogEntry {
        id: "mono",
        name: "Wine Mono",
        description: "Wine's built-in .NET Framework replacement — lighter alternative to installing MS .NET",
        category: "Wine Components",
    },
    DepCatalogEntry {
        id: "gecko",
        name: "Wine Gecko",
        description: "Wine's Internet Explorer engine — needed for applications that embed a browser",
        category: "Wine Components",
    },
];

fn get_dep_steps(id: &str) -> Vec<DepStep> {
    match id {
        "vcredist2022" => vcredist2022_steps(),
        "vcredist2013" => vcredist2013_steps(),
        "vcredist2010" => vcredist2010_steps(),
        "vcredist2008" => vcredist2008_steps(),
        "dotnet48" => dotnet48_steps(),
        "dotnet40" => dotnet40_steps(),
        "dotnet35" => dotnet35_steps(),
        "xna40" => xna40_steps(),
        "directx" => directx_steps(),
        "d3dcompiler47" => d3dcompiler47_steps(),
        "dxvk" => dxvk_steps(),
        "vkd3d" => vkd3d_steps(),
        "xact" => xact_steps(),
        "wmp11" => wmp11_steps(),
        "mono" => mono_steps(),
        "gecko" => gecko_steps(),
        _ => Vec::new(),
    }
}

fn vcredist2022_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/vs/17/release/vc_redist.x86.exe",
                file_name: "vcredist2022_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2022_x86.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/vs/17/release/vc_redist.x64.exe",
                file_name: "vcredist2022_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2022_x64.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "vcruntime140,vcruntime140_1,msvcp140,msvcp140_1,msvcp140_2,concrt140,atl140,vcomp140",
                override_type: "native,builtin",
            },
        },
    ]
}

fn dotnet48_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading .NET Framework 4.8…",
            action: DepStepAction::DownloadFile {
                url: "https://go.microsoft.com/fwlink/?linkid=2088631",
                file_name: "dotnet48.exe",
            },
        },
        DepStep {
            description: "Installing .NET Framework 4.8…",
            action: DepStepAction::RunExe {
                file_name: "dotnet48.exe",
                args: "/sfxlang:1027 /q /norestart",
                extra_env: "WINEDLLOVERRIDES=fusion=b",
            },
        },
        DepStep {
            description: "Configuring mscoree DLL override…",
            action: DepStepAction::OverrideDlls {
                dlls: "mscoree",
                override_type: "native",
            },
        },
    ]
}

fn mono_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Wine Mono…",
            action: DepStepAction::DownloadFile {
                url: "https://dl.winehq.org/wine/wine-mono/10.3.0/wine-mono-10.3.0-x86.msi",
                file_name: "wine-mono-10.3.0-x86.msi",
            },
        },
        DepStep {
            description: "Installing Wine Mono…",
            action: DepStepAction::RunMsi {
                file_name: "wine-mono-10.3.0-x86.msi",
                args: "/qn",
            },
        },
        DepStep {
            description: "Configuring mscoree DLL override…",
            action: DepStepAction::OverrideDlls {
                dlls: "mscoree",
                override_type: "native,builtin",
            },
        },
    ]
}

fn gecko_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Wine Gecko (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://dl.winehq.org/wine/wine-gecko/2.47.4/wine-gecko-2.47.4-x86.msi",
                file_name: "wine-gecko-x86.msi",
            },
        },
        DepStep {
            description: "Installing Wine Gecko (x86)…",
            action: DepStepAction::RunMsi {
                file_name: "wine-gecko-x86.msi",
                args: "/qn",
            },
        },
        DepStep {
            description: "Downloading Wine Gecko (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://dl.winehq.org/wine/wine-gecko/2.47.4/wine-gecko-2.47.4-x86_64.msi",
                file_name: "wine-gecko-x64.msi",
            },
        },
        DepStep {
            description: "Installing Wine Gecko (x64)…",
            action: DepStepAction::RunMsi {
                file_name: "wine-gecko-x64.msi",
                args: "/qn",
            },
        },
    ]
}

fn vcredist2013_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ 2013 Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/highdpimfc2013x86enu",
                file_name: "vcredist2013_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2013 Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2013_x86.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ 2013 Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://aka.ms/highdpimfc2013x64enu",
                file_name: "vcredist2013_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2013 Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2013_x64.exe",
                args: "/quiet /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ 2013 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "msvcr120,msvcp120,vccorlib120",
                override_type: "native,builtin",
            },
        },
    ]
}

fn vcredist2010_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ 2010 SP1 Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x86.exe",
                file_name: "vcredist2010_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2010 SP1 Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2010_x86.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ 2010 SP1 Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/1/6/5/165255E7-1014-4D0A-B094-B6A430A6BFFC/vcredist_x64.exe",
                file_name: "vcredist2010_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2010 SP1 Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2010_x64.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ 2010 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "msvcr100,msvcp100",
                override_type: "native,builtin",
            },
        },
    ]
}

fn vcredist2008_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading Visual C++ 2008 SP1 Redistributable (x86)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/5/D/8/5D8C65CB-C849-4025-8E95-C3966CAFD8AE/vcredist_x86.exe",
                file_name: "vcredist2008_x86.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2008 SP1 Redistributable (x86)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2008_x86.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Downloading Visual C++ 2008 SP1 Redistributable (x64)…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/5/D/8/5D8C65CB-C849-4025-8E95-C3966CAFD8AE/vcredist_x64.exe",
                file_name: "vcredist2008_x64.exe",
            },
        },
        DepStep {
            description: "Installing Visual C++ 2008 SP1 Redistributable (x64)…",
            action: DepStepAction::RunExe {
                file_name: "vcredist2008_x64.exe",
                args: "/q /norestart",
                extra_env: "",
            },
        },
        DepStep {
            description: "Configuring Visual C++ 2008 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "msvcr90,msvcp90",
                override_type: "native,builtin",
            },
        },
    ]
}

fn dotnet40_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading .NET Framework 4.0…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/9/5/A/95A9616B-7A37-4AF6-BC36-D6EA96C8DAAE/dotNetFx40_Full_x86_x64.exe",
                file_name: "dotnet40.exe",
            },
        },
        DepStep {
            description: "Installing .NET Framework 4.0…",
            action: DepStepAction::RunExe {
                file_name: "dotnet40.exe",
                args: "/sfxlang:1027 /q /norestart",
                extra_env: "WINEDLLOVERRIDES=fusion=b",
            },
        },
        DepStep {
            description: "Configuring mscoree DLL override…",
            action: DepStepAction::OverrideDlls {
                dlls: "mscoree",
                override_type: "native",
            },
        },
    ]
}

fn dotnet35_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading .NET Framework 3.5 SP1…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/2/0/E/20E90413-712F-438C-988E-FDAA79A8AC3D/dotnetfx35.exe",
                file_name: "dotnet35.exe",
            },
        },
        DepStep {
            description: "Installing .NET Framework 3.5 SP1…",
            action: DepStepAction::RunExe {
                file_name: "dotnet35.exe",
                args: "/sfxlang:1027 /q /norestart",
                extra_env: "WINEDLLOVERRIDES=fusion=b",
            },
        },
        DepStep {
            description: "Configuring mscoree DLL override…",
            action: DepStepAction::OverrideDlls {
                dlls: "mscoree",
                override_type: "native",
            },
        },
    ]
}

fn xna40_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading XNA Framework 4.0…",
            action: DepStepAction::DownloadFile {
                url: "https://download.microsoft.com/download/A/C/2/AC2C903B-E6E8-42C2-9FD7-BEBAC362A930/xnafx40_redist.msi",
                file_name: "xnafx40_redist.msi",
            },
        },
        DepStep {
            description: "Installing XNA Framework 4.0…",
            action: DepStepAction::RunMsi {
                file_name: "xnafx40_redist.msi",
                args: "/qn",
            },
        },
    ]
}

fn directx_steps() -> Vec<DepStep> {
    // The June 2010 DirectX End-User Runtime is installed via winetricks
    // which handles the self-extracting package and DXSETUP correctly.
    vec![
        DepStep {
            description: "Installing DirectX 9 components (d3dx9_xx)…",
            action: DepStepAction::RunWinetricks { verb: "d3dx9" },
        },
        DepStep {
            description: "Installing DirectX 10 components (d3dx10)…",
            action: DepStepAction::RunWinetricks { verb: "d3dx10" },
        },
        DepStep {
            description: "Installing DirectX 11 components (d3dx11_43)…",
            action: DepStepAction::RunWinetricks { verb: "d3dx11_43" },
        },
        DepStep {
            description: "Configuring d3dx9 DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "d3dx9_24,d3dx9_25,d3dx9_26,d3dx9_27,d3dx9_28,d3dx9_29,d3dx9_30,\
                       d3dx9_31,d3dx9_32,d3dx9_33,d3dx9_34,d3dx9_35,d3dx9_36,d3dx9_37,\
                       d3dx9_38,d3dx9_39,d3dx9_40,d3dx9_41,d3dx9_42,d3dx9_43",
                override_type: "native,builtin",
            },
        },
    ]
}

fn d3dcompiler47_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Installing D3D Compiler 47 via winetricks…",
            action: DepStepAction::RunWinetricks {
                verb: "d3dcompiler_47",
            },
        },
    ]
}

fn dxvk_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading DXVK 2.4…",
            action: DepStepAction::DownloadFile {
                url: "https://github.com/doitsujin/dxvk/releases/download/v2.4/dxvk-2.4.tar.gz",
                file_name: "dxvk-2.4.tar.gz",
            },
        },
        DepStep {
            description: "Extracting DXVK archive…",
            action: DepStepAction::ExtractArchive {
                archive_name: "dxvk-2.4.tar.gz",
                dest_subdir: "dxvk",
            },
        },
        DepStep {
            description: "Installing DXVK DLLs (64-bit)…",
            action: DepStepAction::CopyDllsToPrefix {
                src_subdir: "dxvk/x64",
                dlls: "d3d9,d3d10core,d3d11,dxgi",
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Installing DXVK DLLs (32-bit)…",
            action: DepStepAction::CopyDllsToPrefix {
                src_subdir: "dxvk/x32",
                dlls: "d3d9,d3d10core,d3d11,dxgi",
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Configuring DXVK DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "d3d9,d3d10core,d3d11,dxgi",
                override_type: "native",
            },
        },
    ]
}

fn vkd3d_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Downloading VKD3D-Proton 2.12…",
            action: DepStepAction::DownloadFile {
                url: "https://github.com/HansKristian-Work/vkd3d-proton/releases/download/v2.12/vkd3d-proton-2.12.tar.zst",
                file_name: "vkd3d-proton-2.12.tar.zst",
            },
        },
        DepStep {
            description: "Extracting VKD3D-Proton archive…",
            action: DepStepAction::ExtractArchive {
                archive_name: "vkd3d-proton-2.12.tar.zst",
                dest_subdir: "vkd3d",
            },
        },
        DepStep {
            description: "Installing VKD3D-Proton DLLs (64-bit)…",
            action: DepStepAction::CopyDllsToPrefix {
                src_subdir: "vkd3d/x64",
                dlls: "d3d12,d3d12core",
                wine_dir: "system32",
            },
        },
        DepStep {
            description: "Installing VKD3D-Proton DLLs (32-bit)…",
            action: DepStepAction::CopyDllsToPrefix {
                src_subdir: "vkd3d/x86",
                dlls: "d3d12,d3d12core",
                wine_dir: "syswow64",
            },
        },
        DepStep {
            description: "Configuring VKD3D DLL overrides…",
            action: DepStepAction::OverrideDlls {
                dlls: "d3d12,d3d12core",
                override_type: "native",
            },
        },
    ]
}

fn xact_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Installing XACT audio runtime via winetricks…",
            action: DepStepAction::RunWinetricks { verb: "xact" },
        },
    ]
}

fn wmp11_steps() -> Vec<DepStep> {
    vec![
        DepStep {
            description: "Installing Windows Media Player 11 via winetricks…",
            action: DepStepAction::RunWinetricks { verb: "wmp11" },
        },
    ]
}

// ── Cache & tracking helpers ──────────────────────────────────────────────────

fn get_deps_cache_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/leyen/deps/cache", home)
}

fn get_prefix_deps_file(prefix_path: &str) -> PathBuf {
    PathBuf::from(prefix_path).join(".leyen/deps/installed.txt")
}

fn read_installed_deps(prefix_path: &str) -> std::collections::HashSet<String> {
    let path = get_prefix_deps_file(prefix_path);
    fs::read_to_string(&path)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn add_installed_dep(prefix_path: &str, dep_id: &str) {
    let path = get_prefix_deps_file(prefix_path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut installed = read_installed_deps(prefix_path);
    installed.insert(dep_id.to_string());
    let mut sorted: Vec<String> = installed.into_iter().collect();
    sorted.sort();
    let _ = fs::write(&path, format!("{}\n", sorted.join("\n")));
}

fn remove_installed_dep(prefix_path: &str, dep_id: &str) {
    let path = get_prefix_deps_file(prefix_path);
    let mut installed = read_installed_deps(prefix_path);
    installed.remove(dep_id);
    let mut sorted: Vec<String> = installed.into_iter().collect();
    sorted.sort();
    let _ = fs::write(&path, format!("{}\n", sorted.join("\n")));
}

// ── Step execution engine ─────────────────────────────────────────────────────

fn execute_dep_step(
    step: &DepStep,
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
) -> Result<(), String> {
    match &step.action {
        DepStepAction::DownloadFile { url, file_name } => {
            let dest = format!("{}/{}", cache_dir, file_name);
            if std::path::Path::new(&dest).exists() {
                return Ok(());
            }
            let _ = fs::create_dir_all(cache_dir);
            let status = std::process::Command::new("curl")
                .args(["-sL", "--fail", "--location", "-o", &dest, url])
                .status()
                .map_err(|e| format!("curl unavailable: {}", e))?;
            if !status.success() {
                let _ = fs::remove_file(&dest);
                return Err(format!("Download failed for {}", file_name));
            }
            Ok(())
        }

        DepStepAction::RunExe {
            file_name,
            args,
            extra_env,
        } => {
            let exe_path = format!("{}/{}", cache_dir, file_name);
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
            cmd.env("WINEPREFIX", prefix_path);
            if !proton_path.is_empty() {
                cmd.env("PROTONPATH", proton_path);
            }
            cmd.env("GAMEID", "leyen-dep-install");
            for pair in extra_env.split_whitespace() {
                if let Some(eq) = pair.find('=') {
                    cmd.env(&pair[..eq], &pair[eq + 1..]);
                }
            }
            let mut run_args = vec![exe_path];
            for arg in args.split_whitespace() {
                run_args.push(arg.to_string());
            }
            cmd.args(&run_args);
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to launch {}: {}", file_name, e))?;
            if !status.success() {
                return Err(format!("Installer '{}' exited with an error", file_name));
            }
            Ok(())
        }

        DepStepAction::RunMsi { file_name, args } => {
            let msi_path = format!("{}/{}", cache_dir, file_name);
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
            cmd.env("WINEPREFIX", prefix_path);
            if !proton_path.is_empty() {
                cmd.env("PROTONPATH", proton_path);
            }
            cmd.env("GAMEID", "leyen-dep-install");
            let mut run_args = vec![
                "msiexec.exe".to_string(),
                "/i".to_string(),
                msi_path,
            ];
            for arg in args.split_whitespace() {
                run_args.push(arg.to_string());
            }
            cmd.args(&run_args);
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run msiexec for {}: {}", file_name, e))?;
            if !status.success() {
                return Err(format!("MSI install '{}' failed", file_name));
            }
            Ok(())
        }

        DepStepAction::OverrideDlls {
            dlls,
            override_type,
        } => {
            let reg_lines: Vec<String> = dlls
                .split(',')
                .map(|d| format!("\"{}\"=\"{}\"", d.trim(), override_type))
                .collect();
            let reg_content = format!(
                "Windows Registry Editor Version 5.00\r\n\r\n\
                 [HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\
                 {}\r\n",
                reg_lines.join("\r\n")
            );
            let safe_name = dlls
                .split(',')
                .next()
                .unwrap_or("dll")
                .trim()
                .replace(['-', '.'], "_");
            let reg_path = format!("{}/override_{}.reg", cache_dir, safe_name);
            fs::write(&reg_path, reg_content)
                .map_err(|e| format!("Failed to write .reg file: {}", e))?;

            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
            cmd.env("WINEPREFIX", prefix_path);
            if !proton_path.is_empty() {
                cmd.env("PROTONPATH", proton_path);
            }
            cmd.env("GAMEID", "leyen-dep-install");
            cmd.args(["regedit.exe", "/S", &reg_path]);
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run regedit: {}", e))?;
            let _ = fs::remove_file(&reg_path);
            if !status.success() {
                return Err(format!("DLL override registration failed for: {}", dlls));
            }
            Ok(())
        }

        DepStepAction::RegisterDll { dll } => {
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
            cmd.env("WINEPREFIX", prefix_path);
            if !proton_path.is_empty() {
                cmd.env("PROTONPATH", proton_path);
            }
            cmd.env("GAMEID", "leyen-dep-install");
            cmd.args(["regsvr32.exe", "/s", dll]);
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run regsvr32: {}", e))?;
            if !status.success() {
                return Err(format!("Failed to register DLL '{}'", dll));
            }
            Ok(())
        }

        DepStepAction::ExtractArchive {
            archive_name,
            dest_subdir,
        } => {
            let archive_path = format!("{}/{}", cache_dir, archive_name);
            let dest_path = format!("{}/{}", cache_dir, dest_subdir);
            fs::create_dir_all(&dest_path)
                .map_err(|e| format!("Failed to create extraction directory '{}': {}", dest_path, e))?;
            let mut cmd = std::process::Command::new("tar");
            if archive_name.ends_with(".tar.zst") || archive_name.ends_with(".tzst") {
                cmd.args(["-I", "zstd", "-xf", &archive_path, "-C", &dest_path, "--strip-components=1"]);
            } else {
                cmd.args(["-xf", &archive_path, "-C", &dest_path, "--strip-components=1"]);
            }
            let status = cmd
                .status()
                .map_err(|e| format!("tar unavailable: {}", e))?;
            if !status.success() {
                return Err(format!("Failed to extract '{}'", archive_name));
            }
            Ok(())
        }

        DepStepAction::CopyDllsToPrefix {
            src_subdir,
            dlls,
            wine_dir,
        } => {
            let src_dir = format!("{}/{}", cache_dir, src_subdir);
            let dst_dir = format!("{}/drive_c/windows/{}", prefix_path, wine_dir);
            fs::create_dir_all(&dst_dir)
                .map_err(|e| format!("Failed to create target directory '{}': {}", dst_dir, e))?;
            for dll in dlls.split(',') {
                let dll = dll.trim();
                let src = format!("{}/{}.dll", src_dir, dll);
                let dst = format!("{}/{}.dll", dst_dir, dll);
                fs::copy(&src, &dst)
                    .map_err(|e| format!("Failed to copy {}.dll: {}", dll, e))?;
            }
            Ok(())
        }

        DepStepAction::RunWinetricks { verb } => {
            let umu = get_umu_run_path();
            let mut cmd = std::process::Command::new(&umu);
            cmd.env("WINEPREFIX", prefix_path);
            if !proton_path.is_empty() {
                cmd.env("PROTONPATH", proton_path);
            }
            cmd.env("GAMEID", "leyen-dep-install");
            cmd.args(["winetricks", "-q", verb]);
            let status = cmd
                .status()
                .map_err(|e| format!("Failed to run winetricks {}: {}", verb, e))?;
            if !status.success() {
                return Err(format!("winetricks '{}' failed", verb));
            }
            Ok(())
        }
    }
}

// ── Async orchestrator ────────────────────────────────────────────────────────

fn install_dep_async(
    dep_id: &str,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
    on_progress: impl Fn(usize, usize, String) + 'static,
    on_finish: impl FnOnce(bool, Option<String>) + 'static,
) {
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        on_finish(false, Some("umu-launcher not ready".to_string()));
        return;
    }
    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        on_finish(false, Some("umu-launcher not available".to_string()));
        return;
    }

    let steps = get_dep_steps(dep_id);
    if steps.is_empty() {
        let msg = format!("No install steps defined for '{}'", dep_id);
        overlay.add_toast(adw::Toast::new(&msg));
        on_finish(false, Some(msg));
        return;
    }

    let total = steps.len();
    let prefix_t = prefix_path.to_string();
    let proton_t = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();

    // Shared queue: background thread pushes messages; idle callback drains them on GTK thread.
    let queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<DepInstallMsg>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let queue_bg = queue.clone();

    let on_finish = std::rc::Rc::new(std::cell::RefCell::new(Some(on_finish)));
    let on_progress = std::rc::Rc::new(on_progress);

    glib::idle_add_local(move || {
        let mut q = queue.lock().unwrap();
        while let Some(msg) = q.pop_front() {
            match msg {
                DepInstallMsg::Progress { step, total, description } => {
                    on_progress(step, total, description);
                }
                DepInstallMsg::Done => {
                    if let Some(f) = on_finish.borrow_mut().take() { f(true, None); }
                    return glib::ControlFlow::Break;
                }
                DepInstallMsg::Failed(err) => {
                    if let Some(f) = on_finish.borrow_mut().take() { f(false, Some(err)); }
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });

    std::thread::spawn(move || {
        for (i, step) in steps.iter().enumerate() {
            queue_bg.lock().unwrap().push_back(DepInstallMsg::Progress {
                step: i + 1,
                total,
                description: step.description.to_string(),
            });
            if let Err(e) = execute_dep_step(step, &prefix_t, &proton_t, &cache_dir) {
                queue_bg.lock().unwrap().push_back(DepInstallMsg::Failed(e));
                return;
            }
        }
        queue_bg.lock().unwrap().push_back(DepInstallMsg::Done);
    });
}

// ── Markup escaping helper ───────────────────────────────────────────────────

fn escape_dep_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ── Dependencies dialog ───────────────────────────────────────────────────────

const DEP_CATEGORY_ORDER: &[&str] = &["Runtime", "DirectX", "Media", "Wine Components"];

fn dep_category_order(cat: &str) -> usize {
    DEP_CATEGORY_ORDER
        .iter()
        .position(|&c| c == cat)
        .unwrap_or(usize::MAX)
}

fn show_dependencies_dialog(
    parent: &adw::ApplicationWindow,
    prefix_path: &str,
    proton_path: &str,
    overlay: &adw::ToastOverlay,
) {
    let resolved_prefix = if !prefix_path.is_empty() {
        prefix_path.to_string()
    } else {
        let s = load_settings();
        if !s.default_prefix_path.is_empty() {
            s.default_prefix_path
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{}/.local/share/leyen/prefixes/default", home)
        }
    };

    let installed = read_installed_deps(&resolved_prefix);
    let installed_count = installed.len();

    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .default_height(640)
        .destroy_with_parent(true)
        .build();

    let subtitle = match installed_count {
        0 => "No components installed".to_string(),
        1 => "1 component installed".to_string(),
        n => format!("{} components installed", n),
    };

    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Manage Dependencies", &subtitle))
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();

    let close_btn = gtk4::Button::builder().label("Close").build();
    header.pack_start(&close_btn);

    let search_entry = gtk4::SearchEntry::builder()
        .placeholder_text("Search dependencies…")
        .margin_top(8)
        .margin_bottom(4)
        .margin_start(12)
        .margin_end(12)
        .build();

    let clamp = adw::Clamp::builder()
        .margin_top(4)
        .margin_bottom(8)
        .build();

    let dep_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    clamp.set_child(Some(&dep_box));

    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .child(&clamp)
        .build();

    let content_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    content_box.append(&search_entry);
    content_box.append(&scroll);

    let toolbar_view = adw::ToolbarView::builder().build();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_box));
    dialog.set_content(Some(&toolbar_view));

    let mut entries: Vec<&DepCatalogEntry> = DEP_CATALOG.iter().collect();
    entries.sort_by(|a, b| {
        dep_category_order(a.category)
            .cmp(&dep_category_order(b.category))
            .then(a.name.cmp(b.name))
    });

    let mut categories: Vec<&str> = Vec::new();
    for e in &entries {
        if !categories.contains(&e.category) {
            categories.push(e.category);
        }
    }

    let mut groups: Vec<(adw::PreferencesGroup, Vec<(adw::ActionRow, &'static str)>)> = Vec::new();

    for cat in &categories {
        let group = adw::PreferencesGroup::builder().title(*cat).build();
        let mut rows_in_group: Vec<(adw::ActionRow, &'static str)> = Vec::new();

        for entry in entries.iter().filter(|e| e.category == *cat) {
            let dep_id = entry.id;
            let is_installed = installed.contains(dep_id);

            let row = adw::ActionRow::builder()
                .title(entry.name)
                .subtitle(&escape_dep_markup(entry.description))
                .build();

            let spinner = gtk4::Spinner::builder()
                .valign(gtk4::Align::Center)
                .visible(false)
                .build();

            let progress_label = gtk4::Label::builder()
                .label("")
                .css_classes(["caption", "dim-label"])
                .valign(gtk4::Align::Center)
                .visible(false)
                .max_width_chars(24)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build();

            let install_btn = gtk4::Button::builder()
                .label("Install")
                .css_classes(["suggested-action"])
                .valign(gtk4::Align::Center)
                .visible(!is_installed)
                .build();

            let reinstall_btn = gtk4::Button::builder()
                .icon_name("view-refresh-symbolic")
                .tooltip_text("Reinstall")
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            let remove_btn = gtk4::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove")
                .css_classes(["destructive-action"])
                .valign(gtk4::Align::Center)
                .visible(is_installed)
                .build();

            if is_installed {
                let badge = gtk4::Label::builder()
                    .label("✓ Installed")
                    .css_classes(["success", "caption"])
                    .valign(gtk4::Align::Center)
                    .build();
                row.add_suffix(&badge);
            }

            row.add_suffix(&spinner);
            row.add_suffix(&progress_label);
            row.add_suffix(&install_btn);
            row.add_suffix(&reinstall_btn);
            row.add_suffix(&remove_btn);

            // ── Install button ─────────────────────────────────────────────
            {
                let install_btn2 = install_btn.clone();
                let reinstall_btn2 = reinstall_btn.clone();
                let remove_btn2 = remove_btn.clone();
                let spinner2 = spinner.clone();
                let progress_label2 = progress_label.clone();
                let row2 = row.clone();
                let prefix2 = resolved_prefix.clone();
                let proton2 = proton_path.to_string();
                let overlay2 = overlay.clone();

                install_btn.connect_clicked(move |_| {
                    install_btn2.set_visible(false);
                    spinner2.set_visible(true);
                    spinner2.start();
                    progress_label2.set_visible(true);
                    row2.set_sensitive(false);

                    let install_btn3 = install_btn2.clone();
                    let reinstall_btn3 = reinstall_btn2.clone();
                    let remove_btn3 = remove_btn2.clone();
                    let spinner3 = spinner2.clone();
                    let progress_label3 = progress_label2.clone();
                    let row3 = row2.clone();
                    let prefix3 = prefix2.clone();
                    let overlay3 = overlay2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, err: Option<String>| {
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        if success {
                            add_installed_dep(&prefix3, dep_id);
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            overlay3.add_toast(adw::Toast::new(&format!(
                                "'{}' installed successfully.",
                                dep_id
                            )));
                        } else {
                            install_btn3.set_visible(true);
                            let msg = err.unwrap_or_else(|| "Installation failed.".to_string());
                            overlay3.add_toast(adw::Toast::new(&msg));
                        }
                    };

                    install_dep_async(
                        dep_id,
                        &prefix2,
                        &proton2,
                        &overlay2,
                        on_progress,
                        on_finish,
                    );
                });
            }

            // ── Reinstall button ───────────────────────────────────────────
            {
                let install_btn2 = install_btn.clone();
                let reinstall_btn2 = reinstall_btn.clone();
                let remove_btn2 = remove_btn.clone();
                let spinner2 = spinner.clone();
                let progress_label2 = progress_label.clone();
                let row2 = row.clone();
                let prefix2 = resolved_prefix.clone();
                let proton2 = proton_path.to_string();
                let overlay2 = overlay.clone();

                reinstall_btn.connect_clicked(move |_| {
                    reinstall_btn2.set_visible(false);
                    remove_btn2.set_visible(false);
                    spinner2.set_visible(true);
                    spinner2.start();
                    progress_label2.set_visible(true);
                    row2.set_sensitive(false);

                    let install_btn3 = install_btn2.clone();
                    let reinstall_btn3 = reinstall_btn2.clone();
                    let remove_btn3 = remove_btn2.clone();
                    let spinner3 = spinner2.clone();
                    let progress_label3 = progress_label2.clone();
                    let row3 = row2.clone();
                    let prefix3 = prefix2.clone();
                    let overlay3 = overlay2.clone();

                    let progress_label_p = progress_label2.clone();
                    let on_progress = move |_step: usize, _total: usize, desc: String| {
                        progress_label_p.set_label(&desc);
                    };

                    let on_finish = move |success: bool, err: Option<String>| {
                        spinner3.stop();
                        spinner3.set_visible(false);
                        progress_label3.set_visible(false);
                        row3.set_sensitive(true);
                        if success {
                            add_installed_dep(&prefix3, dep_id);
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            overlay3.add_toast(adw::Toast::new(&format!(
                                "'{}' reinstalled successfully.",
                                dep_id
                            )));
                        } else {
                            install_btn3.set_visible(false);
                            reinstall_btn3.set_visible(true);
                            remove_btn3.set_visible(true);
                            let msg = err.unwrap_or_else(|| "Reinstall failed.".to_string());
                            overlay3.add_toast(adw::Toast::new(&msg));
                        }
                    };

                    install_dep_async(
                        dep_id,
                        &prefix2,
                        &proton2,
                        &overlay2,
                        on_progress,
                        on_finish,
                    );
                });
            }

            // ── Remove button ──────────────────────────────────────────────
            {
                let install_btn2 = install_btn.clone();
                let reinstall_btn2 = reinstall_btn.clone();
                let remove_btn2 = remove_btn.clone();
                let row2 = row.clone();
                let prefix2 = resolved_prefix.clone();
                let overlay2 = overlay.clone();
                let dialog2 = dialog.clone();

                remove_btn.connect_clicked(move |_| {
                    let confirm = gtk4::AlertDialog::builder()
                        .message(&format!("Remove '{}'?", dep_id))
                        .detail(
                            "This removes the dependency from leyen's tracking. \
                             Installed files may remain in the Wine prefix — use \
                             Wine's Add/Remove Programs for a full uninstall.",
                        )
                        .buttons(vec!["Cancel".to_string(), "Remove".to_string()])
                        .cancel_button(0)
                        .default_button(0)
                        .build();

                    let install_btn3 = install_btn2.clone();
                    let reinstall_btn3 = reinstall_btn2.clone();
                    let remove_btn3 = remove_btn2.clone();
                    let row3 = row2.clone();
                    let prefix3 = prefix2.clone();
                    let overlay3 = overlay2.clone();

                    confirm.choose(
                        Some(&dialog2),
                        gio::Cancellable::NONE,
                        move |result| {
                            if let Ok(1) = result {
                                remove_installed_dep(&prefix3, dep_id);
                                row3.set_sensitive(true);
                                install_btn3.set_visible(true);
                                reinstall_btn3.set_visible(false);
                                remove_btn3.set_visible(false);
                                overlay3.add_toast(adw::Toast::new(&format!(
                                    "'{}' removed from tracking.",
                                    dep_id
                                )));
                            }
                        },
                    );
                });
            }

            group.add(&row);
            rows_in_group.push((row, dep_id));
        }

        dep_box.append(&group);
        groups.push((group, rows_in_group));
    }

    // ── Search filtering ──────────────────────────────────────────────────
    let groups_for_search = groups.clone();
    search_entry.connect_search_changed(move |entry| {
        let query = entry.text().to_lowercase();
        for (group, rows) in &groups_for_search {
            let mut any_visible = false;
            for (row, dep_id) in rows {
                let visible = if query.is_empty() {
                    true
                } else {
                    let title = row.title().to_lowercase();
                    let subtitle = row
                        .subtitle()
                        .map(|s| s.to_lowercase())
                        .unwrap_or_default();
                    title.contains(&query)
                        || subtitle.contains(&query)
                        || dep_id.contains(&query)
                };
                row.set_visible(visible);
                if visible {
                    any_visible = true;
                }
            }
            group.set_visible(query.is_empty() || any_visible);
        }
    });

    let dialog_close = dialog.clone();
    close_btn.connect_clicked(move |_| dialog_close.destroy());

    dialog.present();
}
