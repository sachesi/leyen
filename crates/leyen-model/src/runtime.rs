//! Pure umu-launcher / winetricks path + availability helpers.
//!
//! These compute install locations and probe PATH/local installs — no network,
//! no downloads (those live in `leyen-core`). Shared so the GUI client can spawn
//! prefix tools and gate UI without depending on the engine.

use std::fs;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use directories::ProjectDirs;

use crate::models::GlobalSettings;
use crate::paths::get_data_dir;

/// Resolves a Proton value stored in config. Returns `None` for the
/// "Default"/unset state.
pub fn resolve_proton_path(proton: &str) -> Option<String> {
    if proton.is_empty() || proton == "Default" {
        return None;
    }
    Some(proton.to_string())
}

/// Scans for installed Proton versions and the default prefix location, building
/// a fresh `GlobalSettings` skeleton. Pure filesystem inspection (no downloads).
pub fn detect_proton_versions() -> GlobalSettings {
    let mut versions = vec!["Default".to_string()];

    let leyen_proton = get_data_dir().join("proton");
    if leyen_proton.exists() {
        if let Ok(entries) = fs::read_dir(&leyen_proton) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("proton").is_file() && path.join("version").is_file()
                {
                    versions.push(path.to_string_lossy().to_string());
                }
            }
        }
    } else {
        let _ = fs::create_dir_all(&leyen_proton);
    }

    let default_prefix_path = get_data_dir().join("prefixes").join("default");
    if !default_prefix_path.exists() {
        let _ = fs::create_dir_all(&default_prefix_path);
    }

    GlobalSettings {
        default_prefix_path: default_prefix_path.to_string_lossy().to_string(),
        default_proton: "Default".to_string(),
        available_proton_versions: versions,
        log_errors: true,
        ..GlobalSettings::default()
    }
}

/// Directory where the umu-launcher zipapp is extracted.
pub fn get_umu_core_dir() -> String {
    get_data_dir()
        .join("core")
        .join("umu-launcher")
        .to_string_lossy()
        .to_string()
}

/// Directory where umu-run stores the Steam Linux Runtime (steamrt3).
pub fn get_umu_runtime_dir() -> String {
    ProjectDirs::from("", "", "umu")
        .map(|p| p.data_dir().join("steamrt3"))
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            std::path::PathBuf::from(format!("{}/.local/share/umu/steamrt3", home))
        })
        .to_string_lossy()
        .to_string()
}

/// Returns `true` if `cmd` is found in `$PATH`.
fn is_in_path(cmd: &str) -> bool {
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path_env).any(|dir| dir.join(cmd).is_file())
}

static NIXOS: OnceLock<bool> = OnceLock::new();

/// Returns true if running on NixOS.
pub fn is_nixos() -> bool {
    *NIXOS.get_or_init(|| {
        if std::path::Path::new("/etc/NIXOS").exists() {
            return true;
        }
        if let Ok(content) = fs::read_to_string("/etc/os-release") {
            for line in content.lines() {
                if line == "ID=nixos" || line == "ID=\"nixos\"" {
                    return true;
                }
            }
        }
        false
    })
}

/// Full path to the `umu-run` binary inside the extracted zipapp (`umu/umu-run`).
pub fn get_local_umu_run_path() -> String {
    format!("{}/umu/umu-run", get_umu_core_dir())
}

/// Command / path to use when invoking `umu-run`. Prefers the system-wide
/// binary; falls back to the locally downloaded copy.
pub fn get_umu_run_path() -> String {
    static CACHED_PATH: OnceLock<String> = OnceLock::new();
    CACHED_PATH
        .get_or_init(|| {
            if is_nixos() {
                return "umu-run".to_string();
            }
            if is_in_path("umu-run") {
                return "umu-run".to_string();
            }
            let local_path = get_local_umu_run_path();
            if std::path::Path::new(&local_path).exists() {
                return local_path;
            }
            "umu-run".to_string()
        })
        .clone()
}

/// `true` when `umu-run` is actually available (system PATH or local install).
/// Cached with 1s TTL — avoids a `which`-style probe on hot paths.
pub fn is_umu_run_available() -> bool {
    static CACHE: OnceLock<RwLock<(bool, Instant)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        RwLock::new((
            false,
            Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or_else(Instant::now),
        ))
    });
    if let Ok(guard) = cache.read()
        && guard.1.elapsed() < Duration::from_secs(1)
    {
        return guard.0;
    }
    let result = is_umu_run_available_impl();
    if let Ok(mut guard) = cache.write() {
        *guard = (result, Instant::now());
    }
    result
}

fn is_umu_run_available_impl() -> bool {
    if is_in_path("umu-run") {
        return true;
    }
    if is_nixos() {
        return false;
    }
    std::path::Path::new(&get_local_umu_run_path()).exists()
}

/// Directory where the winetricks script is stored.
pub fn get_winetricks_dir() -> String {
    get_data_dir()
        .join("core")
        .join("winetricks")
        .to_string_lossy()
        .to_string()
}

/// Full path to the locally downloaded winetricks script.
pub fn get_local_winetricks_path() -> String {
    format!("{}/winetricks", get_winetricks_dir())
}

/// Command / path to use when invoking `winetricks`.
pub fn get_winetricks_path() -> String {
    static CACHED_PATH: OnceLock<String> = OnceLock::new();
    CACHED_PATH
        .get_or_init(|| {
            if is_nixos() {
                return "winetricks".to_string();
            }
            if is_in_path("winetricks") {
                return "winetricks".to_string();
            }
            let local_path = get_local_winetricks_path();
            if std::path::Path::new(&local_path).exists() {
                return local_path;
            }
            "winetricks".to_string()
        })
        .clone()
}

/// `true` when `winetricks` is available (system PATH or local download).
/// Cached with 1s TTL.
pub fn is_winetricks_available() -> bool {
    static CACHE: OnceLock<RwLock<(bool, Instant)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        RwLock::new((
            false,
            Instant::now()
                .checked_sub(Duration::from_secs(2))
                .unwrap_or_else(Instant::now),
        ))
    });
    if let Ok(guard) = cache.read()
        && guard.1.elapsed() < Duration::from_secs(1)
    {
        return guard.0;
    }
    let result = is_winetricks_available_impl();
    if let Ok(mut guard) = cache.write() {
        *guard = (result, Instant::now());
    }
    result
}

fn is_winetricks_available_impl() -> bool {
    if is_in_path("winetricks") {
        return true;
    }
    std::path::Path::new(&get_local_winetricks_path()).exists()
}
