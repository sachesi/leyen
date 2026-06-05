use log::{info, warn};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;

// Pure path + availability helpers live in `leyen-model::runtime` so clients can
// reuse them; re-export here so existing `crate::runtime::umu::*` call sites keep
// working.
pub use leyen_model::runtime::{
    get_local_umu_run_path, get_local_winetricks_path, get_umu_core_dir, get_umu_run_path,
    get_umu_runtime_dir, get_winetricks_dir, get_winetricks_path, is_nixos, is_umu_run_available,
    is_winetricks_available,
};

pub static UMU_DOWNLOAD_STARTED: AtomicBool = AtomicBool::new(false);

/// `true` while the background download thread is actively running.
/// The daemon emits `RuntimeStatus` based on readiness; this gates launches.
pub static UMU_DOWNLOADING: AtomicBool = AtomicBool::new(false);

pub static WINETRICKS_DOWNLOAD_STARTED: AtomicBool = AtomicBool::new(false);

/// `true` while the background winetricks download thread is actively running.
pub static WINETRICKS_DOWNLOADING: AtomicBool = AtomicBool::new(false);

#[derive(Error, Debug)]
pub enum UmuError {
    #[error("Failed to create directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to resolve latest version: {0}")]
    VersionResolve(String),
    #[error("Download failed: {0}")]
    Download(String),
    #[error("Extraction failed: {0}")]
    Extraction(String),
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
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "-o",
            &dest_path,
            "https://raw.githubusercontent.com/Winetricks/winetricks/master/src/winetricks",
        ])
        .status()
        .map_err(|e| UmuError::Download(e.to_string()))?;

    if !status.success() {
        let _ = fs::remove_file(&dest_path);
        return Err(UmuError::Download(
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
pub async fn check_or_install_umu() {
    // If we're on NixOS, we expect umu-run to be provided by the system/flake.
    // We don't want to download a generic linux zipapp.
    if is_nixos() {
        return;
    }

    let available = tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or(false);

    if available {
        // Prime the OnceLock cache so first game launch doesn't block the UI.
        let _ = tokio::task::spawn_blocking(get_umu_run_path).await;
        return;
    }

    if UMU_DOWNLOAD_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }

    UMU_DOWNLOADING.store(true, Ordering::Relaxed);

    let umu_core_dir = get_umu_core_dir();

    info!(
        "[dbg] umu-launcher not found, starting background download to {}",
        umu_core_dir
    );
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || download_and_install_umu(&umu_core_dir))
            .await
            .unwrap();
        match &result {
            Ok(()) => info!("[dbg] umu-launcher download+install completed"),
            Err(e) => warn!("[dbg] umu-launcher download+install failed: {e}"),
        }
        if result.is_err() {
            // Reset so the next application start can retry.
            UMU_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
        }
        UMU_DOWNLOADING.store(false, Ordering::Relaxed);
    });
}

/// Checks whether `winetricks` is available.
/// If it is not found in the system PATH or in the local leyen data directory,
/// spawns a background thread that downloads the latest winetricks script
/// from GitHub to `~/.local/share/leyen/core/winetricks/`.
pub async fn check_or_install_winetricks() {
    if is_nixos() {
        return;
    }

    let available = tokio::task::spawn_blocking(is_winetricks_available)
        .await
        .unwrap_or(false);

    if available {
        // Prime the OnceLock cache so first use doesn't block the UI.
        let _ = tokio::task::spawn_blocking(get_winetricks_path).await;
        return;
    }

    if WINETRICKS_DOWNLOAD_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }

    WINETRICKS_DOWNLOADING.store(true, Ordering::Relaxed);

    info!("[dbg] winetricks not found, starting background download");
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(download_winetricks)
            .await
            .unwrap();
        match &result {
            Ok(()) => info!("[dbg] winetricks download completed"),
            Err(e) => warn!("[dbg] winetricks download failed: {e}"),
        }
        if result.is_err() {
            WINETRICKS_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
        }
        WINETRICKS_DOWNLOADING.store(false, Ordering::Relaxed);
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
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "https://github.com/Open-Wine-Components/umu-launcher/releases/latest",
        ])
        .output()
        .map_err(|e| UmuError::VersionResolve(e.to_string()))?;

    let version = if tag_output.status.success() {
        let url = String::from_utf8_lossy(&tag_output.stdout);
        url.trim()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        return Err(UmuError::VersionResolve(
            "Failed to fetch latest version tag".to_string(),
        ));
    };

    if version.is_empty() {
        return Err(UmuError::VersionResolve(
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
            "--connect-timeout",
            "15",
            "--max-time",
            "300",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "-o",
            &tarball_path,
            &download_url,
        ])
        .status()
        .map_err(|e| UmuError::Download(e.to_string()))?
        .success();

    if !ok {
        let _ = fs::remove_file(&tarball_path);
        return Err(UmuError::Download("Download failed".to_string()));
    }

    info!("[dbg] download_and_install_umu: download done, extracting");
    // Extract: the tarball contains an `umu/` directory with `umu-run` inside.
    let extracted = std::process::Command::new("tar")
        .args(["-xf", &tarball_path, "-C", dest_dir])
        .status()
        .map_err(|e| UmuError::Extraction(e.to_string()))?
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
        Err(UmuError::Extraction("Extraction failed".to_string()))
    }
}


