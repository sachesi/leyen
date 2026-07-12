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

use crate::models::{GLOBAL_SETTINGS_VERSION, GlobalSettings};
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

    // No Result channel here — settings are read unconditionally on every
    // launch. Refusing would mean no settings at all; defaulting the version
    // back down would risk a later save clobbering a newer-versioned file. So:
    // warn and keep using the parsed data (unknown future fields are already
    // dropped by serde, which matches today's additive-only reality).
    if settings.version > GLOBAL_SETTINGS_VERSION {
        log::warn!(
            "Settings file '{}' was written by a newer version of leyen (file version {}, this build supports {}); continuing with the parsed data",
            path.display(),
            settings.version,
            GLOBAL_SETTINGS_VERSION
        );
    }

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
    let settings = &GlobalSettings {
        version: GLOBAL_SETTINGS_VERSION,
        ..settings.clone()
    };
    match toml::to_string_pretty(settings) {
        Ok(data) => {
            if let Err(e) = crate::paths::atomic_write(&path, &data) {
                log::error!("Failed to persist settings: {e}");
            }
        }
        Err(e) => log::error!("Failed to serialize settings: {e}"),
    }
}
