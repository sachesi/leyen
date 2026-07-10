//! Filesystem location helpers shared by every Leyen binary, plus the durable
//! [`atomic_write`] used by every config writer.
//!
//! The path getters are pure computations (no I/O beyond `create_dir_all` for
//! the base dirs) so clients and the daemon agree on where `games.toml`,
//! `settings.toml`, data and caches live.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::ProjectDirs;
use uuid::Uuid;

pub fn get_project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("com.github.sachesi", "leyen", "leyen")
}

/// `$HOME`, falling back to `/tmp` when even that isn't set — mirrors
/// `leyen_model::runtime::get_umu_runtime_dir`'s fallback.
fn home_or_tmp() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn get_config_dir() -> PathBuf {
    CONFIG_DIR
        .get_or_init(|| {
            let dir = get_project_dirs()
                .map(|p| p.config_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(format!("{}/.config/leyen", home_or_tmp())));
            let _ = fs::create_dir_all(&dir);
            dir
        })
        .clone()
}

pub fn get_data_dir() -> PathBuf {
    DATA_DIR
        .get_or_init(|| {
            let dir = get_project_dirs()
                .map(|p| p.data_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(format!("{}/.local/share/leyen", home_or_tmp())));
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

/// Durably replaces `path` with `contents`: writes a unique temp file in the
/// same directory, fsyncs it, renames it over `path`, then fsyncs the directory
/// so the rename itself survives a crash. The temp file is removed on failure.
pub fn atomic_write(path: &Path, contents: &str) -> io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "atomic_write: path has no file name")
    })?;
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".tmp.{}.{}", std::process::id(), Uuid::new_v4()));
    let temp_path = path.with_file_name(temp_name);

    let result = (|| {
        let mut file = File::create(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}
