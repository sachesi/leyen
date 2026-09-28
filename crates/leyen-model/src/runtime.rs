//! Pure umu-launcher / winetricks path + availability helpers.
//!
//! These compute install locations and probe PATH/local installs — no network,
//! no downloads (those live in `leyen-core`). Shared so the GUI client can spawn
//! prefix tools and gate UI without depending on the engine.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

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

/// umu-launcher's own folder as a sandboxed launch names it: `XDG_DATA_HOME`
/// does not reach inside the sandbox.
pub fn get_umu_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/share/umu")
}

/// Deletes every Steam Linux Runtime (`steamrt3`, `steamrt4`, …) in `umu_dir`,
/// which umu-launcher downloads again when it next needs one. Returns how many
/// there were. A link in a runtime's place goes, not what it names.
pub fn remove_umu_runtimes(umu_dir: &Path) -> std::io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(umu_dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("steamrt") {
            continue;
        }
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
        removed += 1;
    }
    Ok(removed)
}

/// Where `cmd` is installed system-wide, resolved. A copy under the home
/// directory does not count: the sandbox does not have it.
fn system_program(cmd: &str) -> Option<String> {
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path_env)
        .map(|dir| dir.join(cmd))
        .filter(|candidate| candidate.is_file())
        .filter_map(|candidate| fs::canonicalize(candidate).ok())
        .find(|resolved| {
            ["/usr", "/bin", "/sbin", "/nix"]
                .iter()
                .any(|root| resolved.starts_with(root))
        })
        .map(|resolved| resolved.to_string_lossy().into_owned())
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
            if let Some(system) = system_program("umu-run") {
                return system;
            }
            if is_nixos() {
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
/// A hit is cached for 1s — avoids a `which`-style probe on hot paths.
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
    // Only a hit is reused: a miss cached across a download would outlive it.
    if let Ok(guard) = cache.read()
        && guard.0
        && guard.1.elapsed() < Duration::from_secs(1)
    {
        return true;
    }
    let result = is_umu_run_available_impl();
    if let Ok(mut guard) = cache.write() {
        *guard = (result, Instant::now());
    }
    result
}

fn is_umu_run_available_impl() -> bool {
    if system_program("umu-run").is_some() {
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
            if let Some(system) = system_program("winetricks") {
                return system;
            }
            if is_nixos() {
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
/// A hit is cached for 1s.
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
    // Only a hit is reused: a miss cached across a download would outlive it.
    if let Ok(guard) = cache.read()
        && guard.0
        && guard.1.elapsed() < Duration::from_secs(1)
    {
        return true;
    }
    let result = is_winetricks_available_impl();
    if let Ok(mut guard) = cache.write() {
        *guard = (result, Instant::now());
    }
    result
}

fn is_winetricks_available_impl() -> bool {
    if system_program("winetricks").is_some() {
        return true;
    }
    std::path::Path::new(&get_local_winetricks_path()).exists()
}

#[cfg(test)]
mod tests {
    use super::remove_umu_runtimes;

    #[test]
    fn every_runtime_goes_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let umu = temp.path().join("umu");
        let outside = temp.path().join("outside");
        for runtime in ["steamrt3", "steamrt4"] {
            std::fs::create_dir_all(umu.join(runtime).join("var")).unwrap();
        }
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, umu.join("steamrt3-arm64")).unwrap();
        std::fs::write(umu.join("umu-shim"), "").unwrap();

        assert_eq!(remove_umu_runtimes(&umu).unwrap(), 3);
        let left: Vec<_> = std::fs::read_dir(&umu)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["umu-shim"]);
        assert!(outside.is_dir(), "a link is removed, not followed");
        assert_eq!(remove_umu_runtimes(&umu).unwrap(), 0);
    }
}
