//! Global settings (`settings.toml`): read (shared) + write (client-owned).
//!
//! Settings are *not* in the playtime race, so — unlike the library — they stay
//! client-writable. The daemon only reads them (read-only [`load_settings`]);
//! the GUI preferences dialog is the sole writer ([`save_settings`]), guarded by
//! a flock + atomic temp-rename.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use uuid::Uuid;

use crate::models::GlobalSettings;
use crate::paths::{get_config_dir, get_settings_path};
use crate::runtime::detect_proton_versions;

fn settings_lock_path() -> std::path::PathBuf {
    get_config_dir().join(".settings.lock")
}

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
            if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self { file });
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::WouldBlock {
                return Err(err);
            }
            if start.elapsed() >= timeout {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "settings lock timeout"));
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

/// Reads `settings.toml` (defaults if absent), merging freshly-detected Proton
/// versions and a default prefix path in memory. **Does not persist** — so the
/// daemon can call it without racing the client's writes.
pub fn load_settings() -> GlobalSettings {
    let path = get_settings_path();
    let mut settings: GlobalSettings = fs::read_to_string(&path)
        .ok()
        .and_then(|data| toml::from_str(&data).ok())
        .unwrap_or_default();

    let fresh = detect_proton_versions();
    let merged: HashSet<String> = settings
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
}

/// Persists settings (flock + atomic temp-rename). Client-only.
pub fn save_settings(settings: &GlobalSettings) {
    let _guard = match FlockGuard::lock(&settings_lock_path(), Duration::from_secs(5)) {
        Ok(g) => g,
        Err(e) => {
            log::error!("Failed to acquire settings lock: {e}");
            return;
        }
    };
    let path = get_settings_path();
    if let Ok(data) = toml::to_string_pretty(settings) {
        let temp_path = path.with_extension(format!("toml.tmp.{}.{}", std::process::id(), Uuid::new_v4()));
        if fs::write(&temp_path, data).is_ok() {
            let _ = fs::rename(&temp_path, &path);
        } else {
            let _ = fs::remove_file(&temp_path);
        }
    }
}
