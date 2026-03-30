use std::fs;

pub static UMU_DOWNLOAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// `true` while the background download thread is actively running.
/// The UI polls this to show/hide the download status banner.
pub static UMU_DOWNLOADING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Directory where the umu-launcher zipapp is extracted.
pub fn get_umu_core_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/leyen/core/umu-launcher", home)
}

/// Directory where umu-run stores the Steam Linux Runtime (steamrt3).
/// Deleting this directory forces umu-run to re-download a clean runtime on
/// the next launch — useful when pressure-vessel-wrap fails due to a
/// corrupted or incomplete sniper_platform installation.
pub fn get_umu_runtime_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/umu/steamrt3", home)
}

/// Full path to the `umu-run` binary inside the extracted zipapp (`umu/umu-run`).
pub fn get_local_umu_run_path() -> String {
    format!("{}/umu/umu-run", get_umu_core_dir())
}

/// Returns the command / path to use when invoking `umu-run`.
/// Prefers the system-wide binary; falls back to the locally downloaded copy.
pub fn get_umu_run_path() -> String {
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
pub fn is_umu_run_available() -> bool {
    if std::process::Command::new("which")
        .arg("umu-run")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }
    std::path::Path::new(&get_local_umu_run_path()).exists()
}

/// Checks whether `umu-run` is available.  If it is not found in the system
/// PATH or in the local leyen data directory, spawns a background thread that
/// downloads the latest zipapp release from the umu-launcher GitHub repository
/// and extracts it to `~/.local/share/leyen/core/umu-launcher/`.
pub fn check_or_install_umu() {
    if is_umu_run_available() {
        return;
    }

    if UMU_DOWNLOAD_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    UMU_DOWNLOADING.store(true, std::sync::atomic::Ordering::Relaxed);

    let umu_core_dir = get_umu_core_dir();

    std::thread::spawn(move || {
        let result = download_and_install_umu(&umu_core_dir);
        if !result {
            // Reset so the next application start can retry.
            UMU_DOWNLOAD_STARTED.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        UMU_DOWNLOADING.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Downloads the latest umu-launcher zipapp tarball and extracts it into
/// `dest_dir`.  Returns `true` on success.
fn download_and_install_umu(dest_dir: &str) -> bool {
    let _ = fs::create_dir_all(dest_dir);

    // Resolve the latest release tag via the GitHub redirect.
    let tag_output = std::process::Command::new("curl")
        .args([
            "-sI",
            "-L",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "https://github.com/Open-Wine-Components/umu-launcher/releases/latest",
        ])
        .output();

    let version = match tag_output {
        Ok(o) if o.status.success() => {
            let url = String::from_utf8_lossy(&o.stdout);
            url.trim()
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string()
        }
        _ => return false,
    };

    if version.is_empty() {
        return false;
    }

    let tarball_name = format!("umu-launcher-{}-zipapp.tar", version);
    let tarball_path = format!("{}/{}", dest_dir, tarball_name);
    let download_url = format!(
        "https://github.com/Open-Wine-Components/umu-launcher/releases/download/{}/{}",
        version, tarball_name
    );

    let ok = std::process::Command::new("curl")
        .args(["-sL", "--fail", "-o", &tarball_path, &download_url])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        let _ = fs::remove_file(&tarball_path);
        return false;
    }

    // Extract: the tarball contains an `umu/` directory with `umu-run` inside.
    let extracted = std::process::Command::new("tar")
        .args(["-xf", &tarball_path, "-C", dest_dir])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let _ = fs::remove_file(&tarball_path);

    if extracted {
        // Ensure the binary is executable.
        let umu_run = format!("{}/umu/umu-run", dest_dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&umu_run) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&umu_run, perms);
            }
        }
    }

    extracted
}
