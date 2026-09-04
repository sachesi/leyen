use log::{info, warn};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
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

/// Claims a one-shot download: `true` for exactly one caller until the attempt
/// fails (`started` is then reset so a later start can retry). `downloading`
/// is the claim itself, so a concurrent caller that loses sees it set at once
/// — setting `started` first and `downloading` after left a gap in which a
/// loser saw "started, not downloading" and gave up as "not available".
pub fn claim_download(started: &AtomicBool, downloading: &AtomicBool) -> bool {
    if started.load(Ordering::SeqCst) {
        return false;
    }
    if downloading
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    started.store(true, Ordering::SeqCst);
    true
}

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
    #[error("Checksum verification failed: {0}")]
    Checksum(String),
}

/// Streams `path` through `D` a chunk at a time and returns the lowercase hex
/// digest, so verifying a multi-hundred-MB tarball never loads it into memory
/// whole. Generic over the hash algorithm so `proton.rs` (SHA-512) can reuse it
/// alongside umu-launcher's SHA-256 checks.
pub(crate) fn hash_file_hex<D: Digest>(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hasher = D::new();
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// GitHub computes and publishes a SHA-256 `digest` for every release asset
/// via the Releases API; umu-launcher does not additionally publish a
/// standalone checksums file, so this is the checksum source for its tarball.
const UMU_LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest";

/// Winetricks release tag pinned for reproducible, verifiable downloads.
/// Bump manually after checking the new tag's script against upstream.
const WINETRICKS_PINNED_TAG: &str = "20260125";

/// Downloads the latest winetricks script from GitHub into the local data directory.
pub fn download_winetricks() -> Result<(), UmuError> {
    let dest_dir = get_winetricks_dir();
    fs::create_dir_all(&dest_dir)?;
    let dest_path = format!("{}/winetricks", dest_dir);
    // Download to a uniquely-named temp file first and rename into place only
    // once curl succeeds — a process dying mid-download must not leave a
    // truncated script that `Path::exists()` treats as installed forever.
    let temp_path = format!(
        "{}/winetricks.tmp.{}.{}",
        dest_dir,
        std::process::id(),
        uuid::Uuid::new_v4()
    );

    let winetricks_url = format!(
        "https://raw.githubusercontent.com/Winetricks/winetricks/{}/src/winetricks",
        WINETRICKS_PINNED_TAG
    );

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
            &temp_path,
            &winetricks_url,
        ])
        .status()
        .map_err(|e| UmuError::Download(e.to_string()))?;

    if !status.success() {
        let _ = fs::remove_file(&temp_path);
        return Err(UmuError::Download(
            "Failed to download winetricks".to_string(),
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&temp_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&temp_path, perms);
        }
    }

    if let Err(e) = fs::rename(&temp_path, &dest_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(UmuError::Download(format!(
            "Failed to install winetricks: {e}"
        )));
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

    if !claim_download(&UMU_DOWNLOAD_STARTED, &UMU_DOWNLOADING) {
        return;
    }

    let umu_core_dir = get_umu_core_dir();

    info!(
        "[dbg] umu-launcher not found, starting background download to {}",
        umu_core_dir
    );
    tokio::spawn(async move {
        // A panic in the blocking task must still reset the flags below, or
        // UMU_DOWNLOADING stuck at `true` blocks every future launch.
        let result = tokio::task::spawn_blocking(move || download_and_install_umu(&umu_core_dir))
            .await
            .unwrap_or_else(|e| Err(UmuError::Download(format!("install task failed: {e}"))));
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

    if !claim_download(&WINETRICKS_DOWNLOAD_STARTED, &WINETRICKS_DOWNLOADING) {
        return;
    }

    info!("[dbg] winetricks not found, starting background download");
    tokio::spawn(async move {
        // Same as the umu install above: never leave WINETRICKS_DOWNLOADING set.
        let result = tokio::task::spawn_blocking(download_winetricks)
            .await
            .unwrap_or_else(|e| Err(UmuError::Download(format!("download task failed: {e}"))));
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
    info!("[dbg] download_and_install_umu: resolving latest release metadata");

    // Resolve the latest release via the GitHub API — this also gives us the
    // per-asset SHA-256 `digest` GitHub computes for every release asset,
    // which umu-launcher does not publish as a separate checksums file.
    let api_output = std::process::Command::new("curl")
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
            "-H",
            "Accept: application/vnd.github+json",
            UMU_LATEST_RELEASE_API_URL,
        ])
        .output()
        .map_err(|e| UmuError::VersionResolve(e.to_string()))?;

    if !api_output.status.success() {
        return Err(UmuError::VersionResolve(
            "Failed to fetch latest release metadata".to_string(),
        ));
    }

    let release: Value = serde_json::from_slice(&api_output.stdout).map_err(|e| {
        UmuError::VersionResolve(format!("Failed to parse release metadata: {e}"))
    })?;

    let version = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if version.is_empty() {
        return Err(UmuError::VersionResolve(
            "Resolved version tag is empty".to_string(),
        ));
    }

    info!("[dbg] download_and_install_umu: resolved version={version}");
    let tarball_name = format!("umu-launcher-{}-zipapp.tar", version);

    let asset = find_release_asset(&release, &tarball_name).ok_or_else(|| {
        UmuError::VersionResolve(format!(
            "Release asset '{tarball_name}' not found in latest release metadata"
        ))
    })?;

    let download_url = asset
        .get("browser_download_url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| {
            UmuError::VersionResolve(format!(
                "Release asset '{tarball_name}' has no download URL"
            ))
        })?
        .to_string();

    // Fail closed: an asset with no digest cannot be verified, so refuse to
    // install it rather than falling back to TLS-only trust.
    let expected_sha256 = asset_sha256_digest(asset).ok_or_else(|| {
        UmuError::Checksum(format!(
            "No checksum published for '{tarball_name}'; refusing to install an unverified download"
        ))
    })?;

    let tarball_path = format!("{}/{}", dest_dir, tarball_name);

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

    info!("[dbg] download_and_install_umu: download done, verifying checksum");
    let actual_sha256 = hash_file_hex::<Sha256>(Path::new(&tarball_path)).map_err(|e| {
        UmuError::Checksum(format!(
            "Failed to read downloaded tarball for verification: {e}"
        ))
    })?;
    if !actual_sha256.eq_ignore_ascii_case(&expected_sha256) {
        let _ = fs::remove_file(&tarball_path);
        return Err(UmuError::Checksum(format!(
            "Checksum mismatch for '{tarball_name}': expected {expected_sha256}, got {actual_sha256}; download rejected"
        )));
    }
    info!("[dbg] download_and_install_umu: checksum verified, extracting");
    // Extract into a staging directory and rename `umu/` into place at the
    // end: availability is just `Path::exists()` on `umu/umu-run`, so
    // extracting in place would report "ready" (and let a launch use a
    // half-written zipapp) while tar is still running.
    let umu_dir = format!("{}/umu", dest_dir);
    let staging = format!("{}/.extract.{}.{}", dest_dir, std::process::id(), uuid::Uuid::new_v4());
    fs::create_dir_all(&staging)?;
    let status = std::process::Command::new("tar")
        .args(["-xf", &tarball_path, "-C", &staging])
        .status();

    let _ = fs::remove_file(&tarball_path);

    let extracted = match status {
        Ok(s) => s.success(),
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(UmuError::Extraction(e.to_string()));
        }
    };
    if !extracted {
        let _ = fs::remove_dir_all(&staging);
        return Err(UmuError::Extraction("Extraction failed".to_string()));
    }

    // Ensure the binary is executable before it becomes visible.
    let staged_umu = format!("{}/umu", staging);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let umu_run = format!("{}/umu-run", staged_umu);
        if let Ok(meta) = fs::metadata(&umu_run) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&umu_run, perms);
        }
    }
    let _ = fs::remove_dir_all(&umu_dir);
    if let Err(e) = fs::rename(&staged_umu, &umu_dir) {
        let _ = fs::remove_dir_all(&staging);
        return Err(UmuError::Extraction(format!("Failed to move extracted umu into place: {e}")));
    }
    let _ = fs::remove_dir_all(&staging);
    let version_file = format!("{}/version", dest_dir);
    let _ = fs::write(version_file, version);
    Ok(())
}

/// Finds the release asset named `file_name` in a GitHub Releases API
/// response's `assets` array.
fn find_release_asset<'a>(release: &'a Value, file_name: &str) -> Option<&'a Value> {
    release
        .get("assets")?
        .as_array()?
        .iter()
        .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(file_name))
}

/// Extracts and lowercases the SHA-256 hex digest from a release asset's
/// `digest` field (GitHub's format is `sha256:<hex>`).
fn asset_sha256_digest(asset: &Value) -> Option<String> {
    asset
        .get("digest")?
        .as_str()?
        .strip_prefix("sha256:")
        .map(|s| s.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_matching_asset_by_name() {
        let release = serde_json::json!({
            "assets": [
                {"name": "other.deb", "digest": "sha256:aaaa"},
                {"name": "umu-launcher-1.4.1-zipapp.tar", "digest": "sha256:BBBB"},
            ]
        });
        let asset = find_release_asset(&release, "umu-launcher-1.4.1-zipapp.tar").unwrap();
        assert_eq!(asset_sha256_digest(asset), Some("bbbb".to_string()));
    }

    #[test]
    fn missing_asset_returns_none() {
        let release = serde_json::json!({"assets": [{"name": "other.deb", "digest": "sha256:aaaa"}]});
        assert!(find_release_asset(&release, "umu-launcher-1.4.1-zipapp.tar").is_none());
    }

    #[test]
    fn asset_without_digest_returns_none() {
        let asset = serde_json::json!({"name": "umu-launcher-1.4.1-zipapp.tar"});
        assert_eq!(asset_sha256_digest(&asset), None);
    }

    #[test]
    fn malformed_digest_prefix_returns_none() {
        let asset = serde_json::json!({"name": "x", "digest": "md5:aaaa"});
        assert_eq!(asset_sha256_digest(&asset), None);
    }
}

