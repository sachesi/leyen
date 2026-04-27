use std::fs;
use crate::config::get_data_dir;

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
