use directories::ProjectDirs;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::thread::sleep;
use std::time::Duration;

use crate::models::{
    Game, GameGroup, GamesConfig, GlobalSettings, GroupLaunchDefaults, LibraryItem,
};

const LEYEN_ID_PREFIX: &str = "ly-";
const LEYEN_ID_DIGITS: usize = 4;

pub fn get_project_dirs() -> ProjectDirs {
    ProjectDirs::from("com.github.sachesi", "leyen", "leyen")
        .expect("Could not determine home directory")
}

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn get_config_dir() -> PathBuf {
    CONFIG_DIR
        .get_or_init(|| {
            let dir = get_project_dirs().config_dir().to_path_buf();
            let _ = fs::create_dir_all(&dir);
            dir
        })
        .clone()
}

pub fn get_data_dir() -> PathBuf {
    DATA_DIR
        .get_or_init(|| {
            let dir = get_project_dirs().data_dir().to_path_buf();
            let _ = fs::create_dir_all(&dir);
            dir
        })
        .clone()
}

pub fn get_config_path() -> PathBuf {
    get_config_dir().join("games.toml")
}

pub fn get_settings_path() -> PathBuf {
    get_config_dir().join("settings.toml")
}

fn config_lock_path() -> PathBuf {
    get_config_dir().join(".games.lock")
}

/// RAII guard that acquires `LOCK_EX | LOCK_NB` with retry + timeout.
/// Releases the lock on Drop — panic-safe.
struct FlockGuard {
    file: File,
}

impl FlockGuard {
    fn lock(path: &Path, timeout: Duration) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        let fd = file.as_raw_fd();

        let start = std::time::Instant::now();
        loop {
            let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Self { file });
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::WouldBlock {
                return Err(err);
            }
            if start.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Timed out waiting for lock on '{}' after {:?}",
                        path.display(),
                        timeout
                    ),
                ));
            }
            sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn with_library_exclusive<F, T>(f: F) -> T
where
    F: FnOnce(&mut Vec<LibraryItem>) -> T,
{
    let _guard = FlockGuard::lock(&config_lock_path(), Duration::from_secs(5))
        .expect("Failed to acquire games config lock (is another instance running?)");

    let path = get_config_path();
    let mut items = fs::read_to_string(&path)
        .ok()
        .and_then(|data| toml::from_str::<GamesConfig>(&data).ok())
        .map(|config| config.items)
        .unwrap_or_default();

    let result = f(&mut items);

    if let Ok(data) = toml::to_string_pretty(&GamesConfig { items }) {
        let _ = fs::write(path, data);
    }
    result
}

pub async fn load_library() -> Vec<LibraryItem> {
    let path = get_config_path();
    tokio::task::spawn_blocking(move || {
        let Ok(data) = fs::read_to_string(path) else {
            return Vec::new();
        };

        toml::from_str::<GamesConfig>(&data)
            .map(|config| config.items)
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

pub async fn save_library(items: Vec<LibraryItem>) {
    tokio::task::spawn_blocking(move || {
        with_library_exclusive(|lib| {
            *lib = items;
        });
    })
    .await
    .ok();
}

pub async fn load_games() -> Vec<Game> {
    flatten_games(&load_library().await)
}

pub fn flatten_games(items: &[LibraryItem]) -> Vec<Game> {
    items
        .iter()
        .flat_map(|item| match item {
            LibraryItem::Game(game) => std::slice::from_ref(game),
            LibraryItem::Group(group) => &group.games,
        })
        .cloned()
        .collect()
}

pub async fn load_settings_with_auto_install(auto_install_proton: bool) -> GlobalSettings {
    let path = get_settings_path();
    let settings = tokio::task::spawn_blocking(move || {
        let mut settings: GlobalSettings = if let Ok(data) = fs::read_to_string(&path) {
            toml::from_str(&data).unwrap_or_default()
        } else {
            GlobalSettings::default()
        };

        let fresh = crate::runtime::detect_proton_versions();
        let merged: HashSet<String> = settings
            .available_proton_versions
            .iter()
            .chain(&fresh.available_proton_versions)
            .cloned()
            .collect();
        let mut merged_vec: Vec<String> = merged
            .into_iter()
            .filter(|v| v != "Default")
            .collect();
        merged_vec.sort();
        merged_vec.insert(0, "Default".to_string());
        settings.available_proton_versions = merged_vec;
        if settings.default_prefix_path.is_empty() {
            settings.default_prefix_path = fresh.default_prefix_path;
        }
        settings
    })
    .await
    .unwrap();

    if auto_install_proton && settings.available_proton_versions.len() <= 1 {
        crate::runtime::check_or_install_protonge();
    }
    save_settings(settings.clone()).await;
    settings
}

pub async fn load_settings() -> GlobalSettings {
    load_settings_with_auto_install(false).await
}

pub async fn save_settings(settings: GlobalSettings) {
    let path = get_settings_path();
    tokio::task::spawn_blocking(move || {
        if let Ok(data) = toml::to_string_pretty(&settings) {
            let _ = fs::write(path, data);
        }
    })
    .await
    .ok();
}

pub async fn add_game_playtime(game_id: &str, seconds: u64) -> Option<u64> {
    let game_id = game_id.to_string();
    tokio::task::spawn_blocking(move || {
        with_library_exclusive(|items| {
            find_game_mut(items, &game_id).map(|game| {
                game.playtime_seconds += seconds;
                game.playtime_seconds
            })
        })
    })
    .await
    .ok()
    .flatten()
}

pub async fn record_game_launch_start(game_id: &str, epoch_seconds: u64) -> bool {
    let game_id = game_id.to_string();
    tokio::task::spawn_blocking(move || {
        with_library_exclusive(|items| {
            find_game_mut(items, &game_id).map(|game| {
                game.last_played_epoch_seconds = epoch_seconds;
            })
        })
        .is_some()
    })
    .await
    .unwrap_or(false)
}

pub async fn record_game_launch_result(game_id: &str, duration_seconds: u64, status: &str) -> bool {
    let game_id = game_id.to_string();
    let status = status.to_string();
    tokio::task::spawn_blocking(move || {
        with_library_exclusive(|items| {
            find_game_mut(items, &game_id).map(|game| {
                game.last_run_duration_seconds = duration_seconds;
                game.last_run_status = status;
            })
        })
        .is_some()
    })
    .await
    .unwrap_or(false)
}

fn find_game_mut<'a>(items: &'a mut [LibraryItem], game_id: &str) -> Option<&'a mut Game> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == game_id => return Some(game),
            LibraryItem::Group(group) => {
                for game in &mut group.games {
                    if game.id == game_id {
                        return Some(game);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn find_game_and_group<'a>(
    items: &'a [LibraryItem],
    game_id: &str,
) -> Option<(&'a Game, Option<&'a GameGroup>)> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == game_id => return Some((game, None)),
            LibraryItem::Group(group) => {
                for game in &group.games {
                    if game.id == game_id {
                        return Some((game, Some(group)));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn find_game_by_leyen_id<'a>(
    items: &'a [LibraryItem],
    leyen_id: &str,
) -> Option<(&'a Game, Option<&'a GameGroup>)> {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.leyen_id == leyen_id => return Some((game, None)),
            LibraryItem::Group(group) => {
                for game in &group.games {
                    if game.leyen_id == leyen_id {
                        return Some((game, Some(group)));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn find_group<'a>(items: &'a [LibraryItem], group_id: &str) -> Option<&'a GameGroup> {
    items.iter().find_map(|item| {
        if let LibraryItem::Group(group) = item
            && group.id == group_id
        {
            Some(group)
        } else {
            None
        }
    })
}

pub fn game_parent_group_id(items: &[LibraryItem], game_id: &str) -> Option<String> {
    items.iter().find_map(|item| {
        if let LibraryItem::Group(group) = item
            && group.games.iter().any(|g| g.id == game_id) {
                return Some(group.id.clone());
            }
        None
    })
}

pub fn insert_game(items: &mut Vec<LibraryItem>, group_id: Option<&str>, game: Game) -> bool {
    if let Some(gid) = group_id {
        for item in items {
            if let LibraryItem::Group(group) = item
                && group.id == gid
            {
                group.games.push(game);
                return true;
            }
        }
        false
    } else {
        items.push(LibraryItem::Game(game));
        true
    }
}

pub fn replace_game(items: &mut [LibraryItem], updated_game: &Game) -> bool {
    for item in items {
        match item {
            LibraryItem::Game(game) if game.id == updated_game.id => {
                *game = updated_game.clone();
                return true;
            }
            LibraryItem::Group(group) => {
                for game in &mut group.games {
                    if game.id == updated_game.id {
                        *game = updated_game.clone();
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

pub fn replace_group(
    items: &mut [LibraryItem],
    group_id: &str,
    new_title: String,
    new_defaults: GroupLaunchDefaults,
) -> bool {
    for item in items {
        if let LibraryItem::Group(group) = item
            && group.id == group_id
        {
            group.title = new_title;
            group.defaults = new_defaults;
            return true;
        }
    }
    false
}

pub fn remove_game(items: &mut Vec<LibraryItem>, game_id: &str) -> Option<Game> {
    if let Some(pos) = items.iter().position(|item| {
        if let LibraryItem::Game(game) = item {
            game.id == game_id
        } else {
            false
        }
    })
        && let LibraryItem::Game(game) = items.remove(pos) {
            return Some(game);
        }

    for item in items {
        if let LibraryItem::Group(group) = item
            && let Some(pos) = group.games.iter().position(|g| g.id == game_id) {
                return Some(group.games.remove(pos));
            }
    }
    None
}

pub fn remove_group(items: &mut Vec<LibraryItem>, group_id: &str) -> Option<GameGroup> {
    if let Some(pos) = items.iter().position(|item| {
        if let LibraryItem::Group(group) = item {
            group.id == group_id
        } else {
            false
        }
    })
        && let LibraryItem::Group(group) = items.remove(pos) {
            return Some(group);
        }
    None
}

pub fn generate_unique_leyen_id(items: &[LibraryItem]) -> String {
    let existing_ids: HashSet<String> = flatten_games(items)
        .into_iter()
        .map(|g| g.leyen_id)
        .collect();

    for _ in 0..100 {
        let id = format!(
            "{}{:0width$}",
            LEYEN_ID_PREFIX,
            fastrand::u32(1..10u32.pow(LEYEN_ID_DIGITS as u32)),
            width = LEYEN_ID_DIGITS
        );
        if !existing_ids.contains(&id) {
            return id;
        }
    }

    // Sequential fallback if random attempts exhausted
    for n in 1..10u32.pow(LEYEN_ID_DIGITS as u32) {
        let id = format!("{}{:0width$}", LEYEN_ID_PREFIX, n, width = LEYEN_ID_DIGITS);
        if !existing_ids.contains(&id) {
            return id;
        }
    }

    // All IDs exhausted — use a wider range but still 4 digits
    format!(
        "{}{:0width$}",
        LEYEN_ID_PREFIX,
        fastrand::u32(10000..100000),
        width = LEYEN_ID_DIGITS
    )
}

#[cfg(test)]
fn is_valid_leyen_id(id: &str) -> bool {
    id.starts_with(LEYEN_ID_PREFIX)
        && id.len() == LEYEN_ID_PREFIX.len() + LEYEN_ID_DIGITS
        && id[LEYEN_ID_PREFIX.len()..]
            .chars()
            .all(|c| c.is_ascii_digit())
}

pub fn effective_game_id(game: &Game) -> String {
    if game.game_id.trim().is_empty() {
        game.leyen_id.clone()
    } else {
        game.game_id.clone()
    }
}

pub fn normalize_game_id_from_executable(exe_path: &str) -> String {
    Path::new(exe_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase().replace(' ', "-"))
        .unwrap_or_else(|| "unknown-game".to_string())
}

pub fn suggest_prefix_path(default_prefix: &str, title: &str) -> String {
    if default_prefix.is_empty() {
        return String::new();
    }
    let sanitized_title = title.to_lowercase().replace(' ', "-");
    let mut path = PathBuf::from(default_prefix);
    if let Some(parent) = path.parent() {
        path = parent.join(sanitized_title);
    } else {
        path = PathBuf::from(sanitized_title);
    }
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_leyen_id_matches_expected_shape() {
        assert!(is_valid_leyen_id("ly-2534"));
        assert!(!is_valid_leyen_id("ly-253"));
        assert!(!is_valid_leyen_id("ly-25a4"));
        assert!(!is_valid_leyen_id("game-2534"));
    }

    #[test]
    fn generate_unique_leyen_id_avoids_existing_ids() {
        let items = vec![
            LibraryItem::Game(Game {
                leyen_id: "ly-1234".to_string(),
                ..Game::default()
            }),
            LibraryItem::Group(GameGroup {
                id: "group-1".to_string(),
                title: "Group 1".to_string(),
                defaults: GroupLaunchDefaults::default(),
                games: vec![Game {
                    leyen_id: "ly-5678".to_string(),
                    ..Game::default()
                }],
            }),
        ];

        let generated = generate_unique_leyen_id(&items);
        assert!(is_valid_leyen_id(&generated));
        assert_ne!(generated, "ly-1234");
        assert_ne!(generated, "ly-5678");
    }
}