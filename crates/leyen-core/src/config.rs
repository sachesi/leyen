//! Library + settings **writer** (daemon-owned) and the in-memory library cache.
//!
//! The daemon is the only process that persists `games.toml`. Pure parse/find/
//! mutate helpers live in `leyen-model::library`; this module adds the flocked,
//! atomic, cache-coherent write path and the daemon-authoritative field merge
//! used by the D-Bus `SaveLibrary`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::{OnceLock, RwLock};
use std::thread::sleep;
use std::time::Duration;

use uuid::Uuid;

use leyen_model::library::{find_game_mut, flatten_games};
use leyen_model::models::{GamesConfig, GlobalSettings, LibraryItem};
use leyen_model::paths::{get_config_path, get_settings_path};

fn config_lock_path() -> std::path::PathBuf {
    leyen_model::paths::get_config_dir().join(".games.lock")
}

fn settings_lock_path() -> std::path::PathBuf {
    leyen_model::paths::get_config_dir().join(".settings.lock")
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

fn with_library_exclusive<F, T>(f: F) -> Option<T>
where
    F: FnOnce(&mut Vec<LibraryItem>) -> T,
{
    let _guard = match FlockGuard::lock(&config_lock_path(), Duration::from_secs(5)) {
        Ok(g) => g,
        Err(e) => {
            log::error!("Failed to acquire games config lock: {}", e);
            return None;
        }
    };

    let path = get_config_path();
    // Distinguish "file absent" (safe to default) from "present but unreadable /
    // unparseable" (must NOT overwrite — would erase the entire library).
    let mut items = match fs::read_to_string(&path) {
        Ok(data) => match toml::from_str::<GamesConfig>(&data) {
            Ok(config) => config.items,
            Err(e) => {
                log::error!(
                    "Refusing to mutate games config: parse failed for '{}': {}",
                    path.display(),
                    e
                );
                return None;
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            log::error!(
                "Refusing to mutate games config: read failed for '{}': {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let result = f(&mut items);

    // Keep the in-memory cache in lock-step with the on-disk library so the next
    // `load_library` sees this change without a disk read.
    update_library_cache(&items);

    if let Ok(data) = toml::to_string_pretty(&GamesConfig { items }) {
        let temp_path = path.with_extension(format!(
            "toml.tmp.{}.{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        if fs::write(&temp_path, data).is_ok() {
            let _ = fs::rename(&temp_path, path);
        } else {
            let _ = fs::remove_file(&temp_path);
        }
    }
    Some(result)
}

/// In-memory mirror of the library, kept in sync by every in-process write.
static LIBRARY_CACHE: OnceLock<RwLock<Option<Vec<LibraryItem>>>> = OnceLock::new();

fn library_cache() -> &'static RwLock<Option<Vec<LibraryItem>>> {
    LIBRARY_CACHE.get_or_init(|| RwLock::new(None))
}

fn update_library_cache(items: &[LibraryItem]) {
    if let Ok(mut cache) = library_cache().write() {
        *cache = Some(items.to_vec());
    }
}

pub async fn load_library() -> Result<Vec<LibraryItem>, String> {
    if let Some(items) = library_cache().read().ok().and_then(|guard| guard.clone()) {
        return Ok(items);
    }
    let items = tokio::task::spawn_blocking(leyen_model::library::read_library_from_disk)
        .await
        .unwrap_or_else(|e| Err(format!("Task failed: {}", e)))?;
    update_library_cache(&items);
    Ok(items)
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

/// Persists a client-submitted library while preserving daemon-authoritative
/// per-game fields (playtime + last-run), matched by `game.id`. This is the
/// `SaveLibrary` path: a client builds the new library from a possibly-stale
/// read, but the daemon's playtime accounting must never be clobbered.
pub async fn save_library_merged(incoming: Vec<LibraryItem>) {
    tokio::task::spawn_blocking(move || {
        with_library_exclusive(|current| {
            // Snapshot authoritative fields from the freshly-read on-disk copy.
            let authoritative: HashMap<String, (u64, u64, u64, String)> = flatten_games(current)
                .into_iter()
                .map(|g| {
                    (
                        g.id,
                        (
                            g.playtime_seconds,
                            g.last_played_epoch_seconds,
                            g.last_run_duration_seconds,
                            g.last_run_status,
                        ),
                    )
                })
                .collect();

            let mut merged = incoming;
            for item in &mut merged {
                match item {
                    LibraryItem::Game(game) => apply_authoritative(game, &authoritative),
                    LibraryItem::Group(group) => {
                        for game in &mut group.games {
                            apply_authoritative(game, &authoritative);
                        }
                    }
                }
            }
            *current = merged;
        });
    })
    .await
    .ok();
}

fn apply_authoritative(
    game: &mut leyen_model::models::Game,
    authoritative: &HashMap<String, (u64, u64, u64, String)>,
) {
    if let Some((playtime, last_played, last_run_duration, last_run_status)) =
        authoritative.get(&game.id)
    {
        game.playtime_seconds = *playtime;
        game.last_played_epoch_seconds = *last_played;
        game.last_run_duration_seconds = *last_run_duration;
        game.last_run_status = last_run_status.clone();
    }
}

pub async fn load_games() -> Vec<leyen_model::models::Game> {
    flatten_games(&load_library().await.unwrap_or_default())
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
        let merged: std::collections::HashSet<String> = settings
            .available_proton_versions
            .iter()
            .chain(&fresh.available_proton_versions)
            .cloned()
            .collect();
        let mut merged_vec: Vec<String> = merged.into_iter().filter(|v| v != "Default").collect();
        merged_vec.sort();
        merged_vec.insert(0, "Default".to_string());
        settings.available_proton_versions = merged_vec;
        if settings.default_prefix_path.is_empty() {
            settings.default_prefix_path = fresh.default_prefix_path;
        }
        settings
    })
    .await
    .unwrap_or_else(|e| {
        log::error!("load_settings task failed: {e}");
        GlobalSettings::default()
    });

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
        let _guard = match FlockGuard::lock(&settings_lock_path(), Duration::from_secs(5)) {
            Ok(g) => g,
            Err(e) => {
                log::error!("Failed to acquire settings lock: {}", e);
                return;
            }
        };
        if let Ok(data) = toml::to_string_pretty(&settings) {
            let temp_path = path.with_extension(format!(
                "toml.tmp.{}.{}",
                std::process::id(),
                Uuid::new_v4()
            ));
            if fs::write(&temp_path, data).is_ok() {
                let _ = fs::rename(&temp_path, path);
            } else {
                let _ = fs::remove_file(&temp_path);
            }
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
    .and_then(|v| v.flatten())
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
