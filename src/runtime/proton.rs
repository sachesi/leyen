use std::fs;
use std::path::PathBuf;
use crate::config::{get_data_dir, get_config_dir};
use crate::models::GlobalSettings;

static PROTONGE_DOWNLOAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Resolves a Proton value stored in config.
/// Returns `None` when the value represents the "Default" / unset state.
pub fn resolve_proton_path(proton: &str) -> Option<String> {
    if proton.is_empty() || proton == "Default" {
        return None;
    }

    Some(proton.to_string())
}

/// If no Proton installation is available, downloads the latest ProtonGE
/// release from GitHub into the leyen data directory in a background
/// thread.  Only one download attempt is made per application lifetime.
pub fn check_or_install_protonge() {
    if PROTONGE_DOWNLOAD_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let proton_dir = get_data_dir().join("proton");

    std::thread::spawn(move || {
        let _ = fs::create_dir_all(&proton_dir);
        let proton_dir_str = proton_dir.to_string_lossy();

        // Resolve the latest release tag via the GitHub redirect
        let tag_output = std::process::Command::new("curl")
            .args([
                "--proto",
                "=https",
                "--tlsv1.2",
                "--silent",
                "--show-error",
                "--location",
                "--fail",
                "-o",
                "/dev/null",
                "-w",
                "%{url_effective}",
                "https://github.com/GloriousEggroll/proton-ge-custom/releases/latest",
            ])
            .output();

        let tag = match tag_output {
            Ok(o) if o.status.success() => {
                let url = String::from_utf8_lossy(&o.stdout);
                url.trim()
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string()
            }
            _ => return,
        };

        if tag.is_empty() || !tag.starts_with("GE-Proton") {
            return;
        }

        let tarball = format!("{}.tar.gz", tag);
        let tarball_path = proton_dir.join(&tarball);
        let download_url = format!(
            "https://github.com/GloriousEggroll/proton-ge-custom/releases/download/{}/{}",
            tag, tarball
        );

        let ok = std::process::Command::new("curl")
            .args([
                "--proto",
                "=https",
                "--tlsv1.2",
                "--location",
                "--silent",
                "--show-error",
                "--fail",
                "--retry",
                "3",
                "--retry-delay",
                "1",
                "-o",
                &tarball_path.to_string_lossy(),
                &download_url,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ok {
            let _ = std::process::Command::new("tar")
                .args(["-xzf", &tarball_path.to_string_lossy(), "-C", &proton_dir_str])
                .status();
            let _ = fs::remove_file(&tarball_path);
        }
    });
}

pub fn detect_proton_versions() -> GlobalSettings {
    let mut versions = vec!["Default".to_string()];

    let leyen_proton = get_data_dir().join("proton");
    if leyen_proton.exists() {
        if let Ok(entries) = fs::read_dir(&leyen_proton) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("proton").is_file() && path.join("version").is_file() {
                    versions.push(path.to_string_lossy().to_string());
                }
            }
        }
    } else {
        let _ = fs::create_dir_all(&leyen_proton);
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let steam_compat = PathBuf::from(format!("{}/.steam/steam/compatibilitytools.d", home));
    if steam_compat.exists()
        && let Ok(entries) = fs::read_dir(steam_compat)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("proton").is_file() && path.join("version").is_file() {
                versions.push(path.to_string_lossy().to_string());
            }
        }
    }

    let steam_root = PathBuf::from(format!("{}/.steam/steam/steamapps/common", home));
    if steam_root.exists()
        && let Ok(entries) = fs::read_dir(steam_root)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = entry.file_name().to_str()
                && name.contains("Proton")
                && path.join("proton").is_file()
                && path.join("version").is_file()
            {
                versions.push(path.to_string_lossy().to_string());
            }
        }
    }

    let default_prefix_path = get_data_dir().join("prefixes").join("default");
    if !default_prefix_path.exists() {
        let _ = fs::create_dir_all(&default_prefix_path);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_game(name: &str) -> Game {
        Game {
            id: format!("internal-{name}"),
            title: name.to_string(),
            exe_path: format!("/tmp/{name}.exe"),
            prefix_path: String::new(),
            proton: "Default".to_string(),
            launch_args: String::new(),
            force_mangohud: false,
            custom_icon: false,
            game_wayland: false,
            game_wow64: false,
            game_ntsync: false,
            leyen_id: String::new(),
            game_id: String::new(),
            playtime_seconds: 0,
            last_played_epoch_seconds: 0,
            last_run_duration_seconds: 0,
            last_run_status: String::new(),
        }
    }
}
