use std::fs;
use std::path::PathBuf;

use crate::logging::apply_log_settings;
use crate::models::{Game, GamesConfig, GlobalSettings};
use crate::paths::{config_dir, local_share_leyen_dir, steam_root_dir};
use crate::proton::check_or_install_protonge;

pub fn get_config_dir() -> PathBuf {
    let config_dir = config_dir();
    if !config_dir.exists() {
        let _ = fs::create_dir_all(&config_dir);
    }
    config_dir
}

pub fn get_config_path() -> PathBuf {
    get_config_dir().join("games.toml")
}

pub fn get_settings_path() -> PathBuf {
    get_config_dir().join("settings.toml")
}

pub fn load_games() -> Vec<Game> {
    let path = get_config_path();
    if let Ok(data) = fs::read_to_string(path) {
        toml::from_str::<GamesConfig>(&data)
            .map(|config| config.games)
            .unwrap_or_else(|_| Vec::new())
    } else {
        Vec::new()
    }
}

pub fn save_games(games: &[Game]) {
    let path = get_config_path();
    let config = GamesConfig {
        games: games.to_vec(),
    };
    if let Ok(data) = toml::to_string_pretty(&config) {
        let _ = fs::write(path, data);
    }
}

pub fn load_settings() -> GlobalSettings {
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
    // save_settings calls apply_log_settings internally
    save_settings(&settings);
    settings
}

pub fn save_settings(settings: &GlobalSettings) {
    apply_log_settings(settings);
    let path = get_settings_path();
    if let Ok(data) = toml::to_string_pretty(settings) {
        let _ = fs::write(path, data);
    }
}

pub fn detect_proton_versions() -> GlobalSettings {
    let mut versions = vec!["Default".to_string()];

    // Check local leyen Proton directory first
    let leyen_proton = local_share_leyen_dir().join("proton");
    if leyen_proton.exists() {
        if let Ok(entries) = fs::read_dir(&leyen_proton) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    versions.push(entry.path().to_string_lossy().to_string());
                }
            }
        }
    } else {
        let _ = fs::create_dir_all(&leyen_proton);
    }

    // Steam's compatibility tools
    let steam_compat = steam_root_dir().join("compatibilitytools.d");
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
    let steam_root = steam_root_dir().join("steamapps/common");
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

    let default_prefix_path = local_share_leyen_dir().join("prefixes/default");
    let default_prefix_dir = default_prefix_path.clone();
    if !default_prefix_dir.exists() {
        let _ = fs::create_dir_all(&default_prefix_dir);
    }

    GlobalSettings {
        default_prefix_path: default_prefix_path.to_string_lossy().to_string(),
        default_proton: "Default".to_string(),
        global_mangohud: false,
        global_gamemode: false,
        global_wayland: false,
        global_wow64: false,
        global_ntsync: false,
        available_proton_versions: versions,
        log_errors: true,
        log_warnings: false,
        log_operations: false,
    }
}
