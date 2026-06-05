//! Filesystem location helpers shared by every Leyen binary.
//!
//! These are pure path computations (no I/O beyond `create_dir_all` for the
//! base dirs) so clients and the daemon agree on where `games.toml`,
//! `settings.toml`, data and caches live.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use directories::ProjectDirs;

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
