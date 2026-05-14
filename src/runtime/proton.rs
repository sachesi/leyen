use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::get_data_dir;
use crate::models::GlobalSettings;

static PROTONGE_DOWNLOAD_STARTED: AtomicBool = AtomicBool::new(false);

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
    if PROTONGE_DOWNLOAD_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }

    let proton_dir = get_data_dir().join("proton");

    gtk4::glib::spawn_future_local(async move {
        let _ = tokio::task::spawn_blocking(move || {
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
                    "--connect-timeout",
                    "15",
                    "--max-time",
                    "300",
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
                    "--connect-timeout",
                    "15",
                    "--max-time",
                    "300",
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
                let status = std::process::Command::new("tar")
                    .args([
                        "-xzf",
                        &tarball_path.to_string_lossy(),
                        "-C",
                        &proton_dir_str,
                    ])
                    .status();

                match status {
                    Ok(s) if s.success() => {
                        log::info!("Successfully extracted ProtonGE");
                    }
                    Ok(s) => {
                        log::error!("Failed to extract ProtonGE: tar exited with status {}", s);
                        let _ = fs::remove_dir_all(&proton_dir);
                    }
                    Err(e) => {
                        log::error!("Failed to extract ProtonGE: failed to spawn tar: {}", e);
                        let _ = fs::remove_dir_all(&proton_dir);
                    }
                }
                let _ = fs::remove_file(&tarball_path);
            }
        })
        .await;
    });
}

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
        global_mangohud: false,
        global_gamemode: false,
        global_wayland: false,
        global_wow64: false,
        global_ntsync: false,
        global_hdr: false,
        global_proton_log: false,
        available_proton_versions: versions,
        log_errors: true,
        log_warnings: false,
        log_operations: false,
    }
}
