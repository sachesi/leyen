use log::{info, warn};
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
    #[error("Download failed: {0}")]
    Download(String),
    #[error("Extraction failed: {0}")]
    Extraction(String),
    #[error("Checksum verification failed: {0}")]
    Checksum(String),
}

/// Streams `path` through `D` a chunk at a time and returns the lowercase hex
/// digest, so verifying a multi-hundred-MB tarball never loads it into memory
/// whole.
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

/// umu-launcher release fetched when it is not installed. Bump by hand, with its
/// digest: the `digest` GitHub publishes for the `-zipapp.tar` asset of the
/// release, checked against a download of your own.
const UMU_VERSION: &str = "1.4.4";
const UMU_TARBALL_SHA256: &str = "eb590691841f7fad3fc3ad8fd5db4ccb87849fe7948e62b28ece7a4ee48cc851";

/// Winetricks release fetched when it is not installed, and the SHA-256 of its
/// `src/winetricks` at that tag. Bump both by hand after checking the script
/// against upstream.
const WINETRICKS_PINNED_TAG: &str = "20260125";
const WINETRICKS_SHA256: &str = "431f82fc74000e6c864409f1d8fb495d696c03928808e3e8acffc45179312a7b";

/// `curl` options every download uses: HTTPS only, redirects included.
const CURL_HTTPS_ONLY: [&str; 13] = [
    "--proto",
    "=https",
    "--proto-redir",
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
];

/// Downloads `url` to `dest` with retries, then keeps it only if its SHA-256 is
/// `sha256`; a failed or mismatched download leaves nothing behind.
fn download_verified(url: &str, dest: &str, sha256: &str) -> Result<(), UmuError> {
    let status = std::process::Command::new("curl")
        .args(CURL_HTTPS_ONLY)
        .args(["--retry", "3", "--retry-delay", "1", "-o", dest, url])
        .status()
        .map_err(|e| UmuError::Download(e.to_string()))?;
    if !status.success() {
        let _ = fs::remove_file(dest);
        return Err(UmuError::Download(format!("Failed to download {url}")));
    }
    keep_if_sha256(Path::new(dest), sha256)
}

/// Removes `path` unless its SHA-256 is `sha256`.
fn keep_if_sha256(path: &Path, sha256: &str) -> Result<(), UmuError> {
    let result = match hash_file_hex::<Sha256>(path) {
        Ok(actual) if actual.eq_ignore_ascii_case(sha256) => return Ok(()),
        Ok(actual) => Err(UmuError::Checksum(format!(
            "{}: expected {sha256}, got {actual}; download rejected",
            path.display()
        ))),
        Err(e) => Err(UmuError::Checksum(format!(
            "Failed to read {} for verification: {e}",
            path.display()
        ))),
    };
    let _ = fs::remove_file(path);
    result
}

/// Downloads the pinned winetricks script from GitHub into the local data directory.
pub fn download_winetricks() -> Result<(), UmuError> {
    let dest_dir = get_winetricks_dir();
    fs::create_dir_all(&dest_dir)?;
    let dest_path = format!("{}/winetricks", dest_dir);
    // Download to a uniquely-named temp file first and rename into place only
    // once it is verified — a process dying mid-download must not leave a
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
    download_verified(&winetricks_url, &temp_path, WINETRICKS_SHA256)?;

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
/// downloads the pinned zipapp release from the umu-launcher GitHub repository
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
/// spawns a background thread that downloads the pinned winetricks script
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

/// Downloads the pinned umu-launcher zipapp tarball and extracts it into
/// `dest_dir`.
fn download_and_install_umu(dest_dir: &str) -> Result<(), UmuError> {
    fs::create_dir_all(dest_dir)?;
    let version = UMU_VERSION;
    let tarball_name = format!("umu-launcher-{version}-zipapp.tar");
    let download_url = format!(
        "https://github.com/Open-Wine-Components/umu-launcher/releases/download/{version}/{tarball_name}"
    );
    let tarball_path = format!("{}/{}", dest_dir, tarball_name);

    info!("[dbg] download_and_install_umu: downloading {download_url}");
    download_verified(&download_url, &tarball_path, UMU_TARBALL_SHA256)?;
    info!("[dbg] download_and_install_umu: checksum verified, extracting");
    // Extract into a staging directory and rename `umu/` into place at the
    // end: availability is just `Path::exists()` on `umu/umu-run`, so
    // extracting in place would report "ready" (and let a launch use a
    // half-written zipapp) while tar is still running.
    let umu_dir = format!("{}/umu", dest_dir);
    let staging = format!(
        "{}/.extract.{}.{}",
        dest_dir,
        std::process::id(),
        uuid::Uuid::new_v4()
    );
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
        return Err(UmuError::Extraction(format!(
            "Failed to move extracted umu into place: {e}"
        )));
    }
    let _ = fs::remove_dir_all(&staging);
    let version_file = format!("{}/version", dest_dir);
    let _ = fs::write(version_file, version);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::keep_if_sha256;

    #[test]
    fn a_download_is_kept_only_with_the_pinned_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("winetricks");
        // SHA-256 of "abc".
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

        std::fs::write(&path, b"abc").unwrap();
        keep_if_sha256(&path, abc).unwrap();
        assert!(path.exists());

        std::fs::write(&path, b"abd").unwrap();
        assert!(keep_if_sha256(&path, abc).is_err());
        assert!(!path.exists(), "a mismatched download is removed");
    }
}
