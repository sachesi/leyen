use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;

use crate::logging::apply_log_settings;
use crate::models::{
    Game, GameGroup, GamesConfig, GlobalSettings, GroupLaunchDefaults, LibraryItem,
};
use crate::proton::check_or_install_protonge;

#[derive(Debug, Deserialize)]
struct LegacyGamesConfig {
    #[serde(default)]
    games: Vec<Game>,
}

pub fn get_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let config_dir = PathBuf::from(format!("{}/.config/leyen", home));
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

pub fn load_library() -> Vec<LibraryItem> {
    let path = get_config_path();
    let Ok(data) = fs::read_to_string(path) else {
        return Vec::new();
    };

    if let Ok(config) = toml::from_str::<GamesConfig>(&data) {
        return config.items;
    }

    toml::from_str::<LegacyGamesConfig>(&data)
        .map(|legacy| legacy.games.into_iter().map(LibraryItem::Game).collect())
        .unwrap_or_default()
}

pub fn save_library(items: &[LibraryItem]) {
    let path = get_config_path();
    let config = GamesConfig {
        items: items.to_vec(),
    };
    if let Ok(data) = toml::to_string_pretty(&config) {
        let _ = fs::write(path, data);
    }
}

pub fn load_games() -> Vec<Game> {
    flatten_games(&load_library())
}

pub fn flatten_games(items: &[LibraryItem]) -> Vec<Game> {
    let mut games = Vec::new();
    for item in items {
        match item {
            LibraryItem::Game(game) => games.push(game.clone()),
            LibraryItem::Group(group) => games.extend(group.games.clone()),
        }
    }
    games
}

pub fn find_group<'a>(items: &'a [LibraryItem], group_id: &str) -> Option<&'a GameGroup> {
    items.iter().find_map(|item| match item {
        LibraryItem::Group(group) if group.id == group_id => Some(group),
        _ => None,
    })
}

pub fn find_game_and_group<'a>(
    items: &'a [LibraryItem],
    game_id: &str,
) -> Option<(&'a Game, Option<&'a GameGroup>)> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == game_id => return Some((game, None)),
            LibraryItem::Group(group) => {
                if let Some(game) = group.games.iter().find(|game| game.id == game_id) {
                    return Some((game, Some(group)));
                }
            }
            _ => {}
        }
    }

    None
}

pub fn format_launch_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut needs_separator = false;

    for ch in title.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            if needs_separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(ch.to_ascii_lowercase());
            needs_separator = false;
        } else if !slug.is_empty() {
            needs_separator = true;
        }
    }

    if slug.is_empty() {
        "game".to_string()
    } else {
        slug
    }
}

pub fn normalize_game_id_from_executable(exe_path: &str) -> String {
    let exe_path = exe_path.trim();
    if exe_path.is_empty() {
        return String::new();
    }

    Path::new(exe_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| exe_path.to_lowercase())
}

pub fn effective_game_id(game: &Game) -> String {
    let normalized = normalize_game_id_from_executable(&game.exe_path);
    if !normalized.is_empty() {
        normalized
    } else if !game.game_id.trim().is_empty() {
        game.game_id.trim().to_string()
    } else {
        format_launch_slug(&game.title)
    }
}

pub fn suggest_prefix_path(base_prefix: &str, title: &str) -> String {
    let base_prefix = base_prefix.trim();
    if base_prefix.is_empty() {
        return String::new();
    }

    let slug = format_launch_slug(title);
    if slug.is_empty() {
        return base_prefix.to_string();
    }

    let base_path = Path::new(base_prefix);
    match base_path.file_name().and_then(|value| value.to_str()) {
        Some("default") => base_path
            .parent()
            .map(|parent| parent.join(slug).to_string_lossy().to_string())
            .unwrap_or_else(|| base_prefix.to_string()),
        _ => base_prefix.to_string(),
    }
}

pub fn game_parent_group_id(items: &[LibraryItem], game_id: &str) -> Option<Option<String>> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == game_id => return Some(None),
            LibraryItem::Group(group) => {
                if group.games.iter().any(|game| game.id == game_id) {
                    return Some(Some(group.id.clone()));
                }
            }
            _ => {}
        }
    }
    None
}

pub fn replace_game(items: &mut [LibraryItem], updated_game: &Game) -> bool {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == updated_game.id => {
                *game = updated_game.clone();
                return true;
            }
            LibraryItem::Group(group) => {
                if let Some(game) = group
                    .games
                    .iter_mut()
                    .find(|game| game.id == updated_game.id)
                {
                    *game = updated_game.clone();
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

pub fn insert_game(
    items: &mut Vec<LibraryItem>,
    parent_group_id: Option<&str>,
    game: Game,
) -> bool {
    match parent_group_id {
        Some(group_id) => {
            let Some(group) = items.iter_mut().find_map(|item| match item {
                LibraryItem::Group(group) if group.id == group_id => Some(group),
                _ => None,
            }) else {
                return false;
            };
            group.games.push(game);
            true
        }
        None => {
            items.push(LibraryItem::Game(game));
            true
        }
    }
}

pub fn remove_game(items: &mut Vec<LibraryItem>, game_id: &str) -> Option<Game> {
    if let Some(pos) = items
        .iter()
        .position(|item| matches!(item, LibraryItem::Game(game) if game.id == game_id))
    {
        return match items.remove(pos) {
            LibraryItem::Game(game) => Some(game),
            LibraryItem::Group(_) => None,
        };
    }

    for item in items {
        if let LibraryItem::Group(group) = item
            && let Some(pos) = group.games.iter().position(|game| game.id == game_id)
        {
            return Some(group.games.remove(pos));
        }
    }

    None
}

pub fn replace_group(
    items: &mut [LibraryItem],
    group_id: &str,
    title: String,
    defaults: GroupLaunchDefaults,
) -> bool {
    let Some(group) = items.iter_mut().find_map(|item| match item {
        LibraryItem::Group(group) if group.id == group_id => Some(group),
        _ => None,
    }) else {
        return false;
    };
    group.title = title;
    group.defaults = defaults;
    true
}

pub fn remove_group(items: &mut Vec<LibraryItem>, group_id: &str) -> Option<String> {
    let pos = items
        .iter()
        .position(|item| matches!(item, LibraryItem::Group(group) if group.id == group_id))?;
    match items.remove(pos) {
        LibraryItem::Group(group) => Some(group.title),
        LibraryItem::Game(_) => None,
    }
}

pub fn add_game_playtime(game_id: &str, additional_seconds: u64) -> Option<u64> {
    let mut items = load_library();
    let total = {
        let game = find_game_mut(&mut items, game_id)?;

        if additional_seconds > 0 {
            game.playtime_seconds = game.playtime_seconds.saturating_add(additional_seconds);
        }

        game.playtime_seconds
    };

    if additional_seconds > 0 {
        save_library(&items);
    }

    Some(total)
}

pub fn record_game_launch_start(game_id: &str, started_at_epoch_seconds: u64) -> bool {
    let mut items = load_library();
    let Some(game) = find_game_mut(&mut items, game_id) else {
        return false;
    };

    game.last_played_epoch_seconds = started_at_epoch_seconds;
    game.last_run_status = "Running".to_string();
    save_library(&items);
    true
}

pub fn record_game_launch_result(game_id: &str, run_seconds: u64, status: &str) -> Option<u64> {
    let mut items = load_library();
    let game = find_game_mut(&mut items, game_id)?;
    game.last_run_duration_seconds = run_seconds;
    game.last_run_status = status.to_string();
    let total = game.playtime_seconds;
    save_library(&items);
    Some(total)
}

fn find_game_mut<'a>(items: &'a mut [LibraryItem], game_id: &str) -> Option<&'a mut Game> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == game_id => return Some(game),
            LibraryItem::Group(group) => {
                if let Some(game) = group.games.iter_mut().find(|game| game.id == game_id) {
                    return Some(game);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn load_settings() -> GlobalSettings {
    let path = get_settings_path();
    let mut settings: GlobalSettings = if let Ok(data) = fs::read_to_string(&path) {
        toml::from_str(&data).unwrap_or_default()
    } else {
        GlobalSettings::default()
    };
    let fresh = detect_proton_versions();
    settings.available_proton_versions = fresh.available_proton_versions;
    if settings.default_prefix_path.is_empty() {
        settings.default_prefix_path = fresh.default_prefix_path;
    }
    if settings.available_proton_versions.len() <= 1 {
        check_or_install_protonge();
    }
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

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());

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
        let _ = fs::create_dir_all(&leyen_proton);
    }

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

    let steam_root = PathBuf::from(format!("{}/.steam/steam/steamapps/common", home));
    if steam_root.exists() {
        if let Ok(entries) = fs::read_dir(steam_root) {
            for entry in entries.flatten() {
                if entry.path().is_dir()
                    && let Some(name) = entry.file_name().to_str()
                    && name.contains("Proton")
                {
                    versions.push(entry.path().to_string_lossy().to_string());
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
        log_errors: true,
        log_warnings: false,
        log_operations: false,
    }
}
