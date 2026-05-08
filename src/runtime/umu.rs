use crate::config::get_data_dir;
use directories::ProjectDirs;
use gtk4::glib;
use log::{info, warn};
use std::fs;
use thiserror::Error;

pub static UMU_DOWNLOAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// `true` while the background download thread is actively running.
/// The UI polls this to show/hide the download status banner.
pub static UMU_DOWNLOADING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub static WINETRICKS_DOWNLOAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// `true` while the background winetricks download thread is actively running.
/// The UI polls this to show/hide the download status banner.
pub static WINETRICKS_DOWNLOADING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Error, Debug)]
pub enum UmuError {
    #[error("Failed to create directory: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Failed to resolve latest version: {0}")]
    VersionResolveError(String),
    #[error("Download failed: {0}")]
    DownloadError(String),
    #[error("Extraction failed: {0}")]
    ExtractionError(String),
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
/// Deleting this directory forces umu-run to re-download a clean runtime on
/// the next launch — useful when pressure-vessel-wrap fails due to a
/// corrupted or incomplete sniper_platform installation.
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

/// Returns true if running on NixOS.
pub fn is_nixos() -> bool {
    if std::path::Path::new("/etc/NIXOS").exists() {
        return true;
    }

    // Fallback: check /etc/os-release for ID=nixos
    if let Ok(content) = fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if line == "ID=nixos" || line == "ID=\"nixos\"" {
                return true;
            }
        }
    }

    false
}

/// Full path to the `umu-run` binary inside the extracted zipapp (`umu/umu-run`).
pub fn get_local_umu_run_path() -> String {
    format!("{}/umu/umu-run", get_umu_core_dir())
}

/// Returns the command / path to use when invoking `umu-run`.
/// Prefers the system-wide binary; falls back to the locally downloaded copy.
pub fn get_umu_run_path() -> String {
    if is_nixos() {
        return "umu-run".to_string();
    }

    if std::process::Command::new("which")
        .arg("umu-run")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return "umu-run".to_string();
    }

    let local_path = get_local_umu_run_path();
    if std::path::Path::new(&local_path).exists() {
        return local_path;
    }

    "umu-run".to_string()
}

/// Returns `true` when `umu-run` is actually available (system PATH or local
/// install).  Unlike `get_umu_run_path()` this does not return a fallback
/// string when umu-run is absent.
/// Not cached — always re-checks so a download started during the
/// session is detected immediately.
pub fn is_umu_run_available() -> bool {
    if std::process::Command::new("which")
        .arg("umu-run")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
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

/// Returns the command / path to use when invoking `winetricks`.
/// Prefers the system-wide binary; falls back to the locally downloaded copy.
/// Not cached — always re-checks so a download started during the
/// session is detected immediately.
pub fn get_winetricks_path() -> String {
    if is_nixos() {
        return "winetricks".to_string();
    }

    if std::process::Command::new("which")
        .arg("winetricks")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return "winetricks".to_string();
    }

    let local_path = get_local_winetricks_path();
    if std::path::Path::new(&local_path).exists() {
        return local_path;
    }

    "winetricks".to_string()
}

/// Returns `true` when `winetricks` is actually available (system PATH or local
/// download). Not cached — always re-checks so a download started during the
/// session is detected immediately.
pub fn is_winetricks_available() -> bool {
    if std::process::Command::new("which")
        .arg("winetricks")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }
    std::path::Path::new(&get_local_winetricks_path()).exists()
}

/// Downloads the latest winetricks script from GitHub into the local data directory.
pub fn download_winetricks() -> Result<(), UmuError> {
    let dest_dir = get_winetricks_dir();
    fs::create_dir_all(&dest_dir)?;
    let dest_path = format!("{}/winetricks", dest_dir);

    let status = std::process::Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--silent",
            "--show-error",
            "--location",
            "--fail",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "-o",
            &dest_path,
            "https://raw.githubusercontent.com/Winetricks/winetricks/master/src/winetricks",
        ])
        .status()
        .map_err(|e| UmuError::DownloadError(e.to_string()))?;

    if !status.success() {
        let _ = fs::remove_file(&dest_path);
        return Err(UmuError::DownloadError(
            "Failed to download winetricks".to_string(),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&dest_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&dest_path, perms);
        }
    }

    Ok(())
}

/// Checks whether `umu-run` is available.
/// If it is not found in the system
/// PATH or in the local leyen data directory, spawns a background thread that
/// downloads the latest zipapp release from the umu-launcher GitHub repository
/// and extracts it to `~/.local/share/leyen/core/umu-launcher/`.
pub fn check_or_install_umu() {
    // If we're on NixOS, we expect umu-run to be provided by the system/flake.
    // We don't want to download a generic linux zipapp.
    if is_nixos() {
        return;
    }

    if is_umu_run_available() {
        return;
    }

    if UMU_DOWNLOAD_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    UMU_DOWNLOADING.store(true, std::sync::atomic::Ordering::Relaxed);

    let umu_core_dir = get_umu_core_dir();

    info!(
        "[dbg] umu-launcher not found, starting background download to {}",
        umu_core_dir
    );
    glib::spawn_future_local(async move {
        let result = tokio::task::spawn_blocking(move || download_and_install_umu(&umu_core_dir))
            .await
            .unwrap();
        match &result {
            Ok(()) => info!("[dbg] umu-launcher download+install completed"),
            Err(e) => warn!("[dbg] umu-launcher download+install failed: {e}"),
        }
        if result.is_err() {
            // Reset so the next application start can retry.
            UMU_DOWNLOAD_STARTED.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        UMU_DOWNLOADING.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Checks whether `winetricks` is available.
/// If it is not found in the system PATH or in the local leyen data directory,
/// spawns a background thread that downloads the latest winetricks script
/// from GitHub to `~/.local/share/leyen/core/winetricks/`.
pub fn check_or_install_winetricks() {
    if is_nixos() {
        return;
    }

    if is_winetricks_available() {
        return;
    }

    if WINETRICKS_DOWNLOAD_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    WINETRICKS_DOWNLOADING.store(true, std::sync::atomic::Ordering::Relaxed);

    info!("[dbg] winetricks not found, starting background download");
    glib::spawn_future_local(async move {
        let result = tokio::task::spawn_blocking(|| download_winetricks())
            .await
            .unwrap();
        match &result {
            Ok(()) => info!("[dbg] winetricks download completed"),
            Err(e) => warn!("[dbg] winetricks download failed: {e}"),
        }
        if result.is_err() {
            WINETRICKS_DOWNLOAD_STARTED.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        WINETRICKS_DOWNLOADING.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Downloads the latest umu-launcher zipapp tarball and extracts it into
/// `dest_dir`.
fn download_and_install_umu(dest_dir: &str) -> Result<(), UmuError> {
    fs::create_dir_all(dest_dir)?;
    info!("[dbg] download_and_install_umu: resolving latest version tag");

    // Resolve the latest release tag via the GitHub redirect.
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
            "https://github.com/Open-Wine-Components/umu-launcher/releases/latest",
        ])
        .output()
        .map_err(|e| UmuError::VersionResolveError(e.to_string()))?;

    let version = if tag_output.status.success() {
        let url = String::from_utf8_lossy(&tag_output.stdout);
        url.trim()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        return Err(UmuError::VersionResolveError(
            "Failed to fetch latest version tag".to_string(),
        ));
    };

    if version.is_empty() {
        return Err(UmuError::VersionResolveError(
            "Resolved version tag is empty".to_string(),
        ));
    }

    info!("[dbg] download_and_install_umu: resolved version={version}");
    let tarball_name = format!("umu-launcher-{}-zipapp.tar", version);
    let tarball_path = format!("{}/{}", dest_dir, tarball_name);
    let download_url = format!(
        "https://github.com/Open-Wine-Components/umu-launcher/releases/download/{}/{}",
        version, tarball_name
    );

    info!("[dbg] download_and_install_umu: downloading {download_url}");
    let ok = std::process::Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--silent",
            "--show-error",
            "--location",
            "--fail",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "-o",
            &tarball_path,
            &download_url,
        ])
        .status()
        .map_err(|e| UmuError::DownloadError(e.to_string()))?
        .success();

    if !ok {
        let _ = fs::remove_file(&tarball_path);
        return Err(UmuError::DownloadError("Download failed".to_string()));
    }

    info!("[dbg] download_and_install_umu: download done, extracting");
    // Extract: the tarball contains an `umu/` directory with `umu-run` inside.
    let extracted = std::process::Command::new("tar")
        .args(["-xf", &tarball_path, "-C", dest_dir])
        .status()
        .map_err(|e| UmuError::ExtractionError(e.to_string()))?
        .success();

    let _ = fs::remove_file(&tarball_path);

    if extracted {
        // Ensure the binary is executable.
        let umu_run = format!("{}/umu/umu-run", dest_dir);
        let version_file = format!("{}/version", dest_dir);
        let _ = fs::write(version_file, version);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&umu_run) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&umu_run, perms);
            }
        }
        Ok(())
    } else {
        Err(UmuError::ExtractionError("Extraction failed".to_string()))
    }
}

/// Returns the current local version of umu-launcher.
pub fn get_local_umu_version() -> Option<String> {
    let core_dir = get_umu_core_dir();
    let version_file = std::path::Path::new(&core_dir).join("version");
    fs::read_to_string(version_file)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Checks if an update for umu-launcher is available.
pub fn check_for_umu_updates() -> Result<bool, UmuError> {
    let current_version = get_local_umu_version();

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
            "https://github.com/Open-Wine-Components/umu-launcher/releases/latest",
        ])
        .output()
        .map_err(|e| UmuError::VersionResolveError(e.to_string()))?;

    let latest_version = if tag_output.status.success() {
        let url = String::from_utf8_lossy(&tag_output.stdout);
        url.trim()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        return Err(UmuError::VersionResolveError(
            "Failed to fetch latest version tag".to_string(),
        ));
    };

    match current_version {
        Some(version) => Ok(version != latest_version),
        None => Ok(true),
    }
}
