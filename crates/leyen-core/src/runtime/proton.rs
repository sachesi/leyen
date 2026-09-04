use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::Sha512;

use crate::runtime::umu::hash_file_hex;
use leyen_model::paths::get_data_dir;

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

    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            let _ = fs::create_dir_all(&proton_dir);

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
                _ => {
                    PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                    return;
                }
            };

            if tag.is_empty() || !tag.starts_with("GE-Proton") {
                PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                return;
            }

            if !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
                PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
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
                // Fail closed: verify against the release's published
                // `.sha512sum` before extracting anything from the tarball.
                let sums_url = format!(
                    "https://github.com/GloriousEggroll/proton-ge-custom/releases/download/{}/{}.sha512sum",
                    tag, tag
                );
                let sums_output = std::process::Command::new("curl")
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
                        "60",
                        &sums_url,
                    ])
                    .output();

                let expected_sha512 = match sums_output {
                    Ok(o) if o.status.success() => {
                        parse_sha512sum_line(&String::from_utf8_lossy(&o.stdout), &tarball)
                    }
                    _ => None,
                };

                let Some(expected_sha512) = expected_sha512 else {
                    log::error!(
                        "No checksum available for ProtonGE tarball '{}'; refusing to install an unverified download",
                        tarball
                    );
                    let _ = fs::remove_file(&tarball_path);
                    PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                    return;
                };

                let actual_sha512 = match hash_file_hex::<Sha512>(&tarball_path) {
                    Ok(hash) => hash,
                    Err(e) => {
                        log::error!(
                            "Failed to read downloaded ProtonGE tarball for verification: {}",
                            e
                        );
                        let _ = fs::remove_file(&tarball_path);
                        PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                        return;
                    }
                };

                if !actual_sha512.eq_ignore_ascii_case(&expected_sha512) {
                    log::error!(
                        "Checksum mismatch for ProtonGE tarball '{}': expected {}, got {}; download rejected",
                        tarball, expected_sha512, actual_sha512
                    );
                    let _ = fs::remove_file(&tarball_path);
                    PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                    return;
                }
                log::info!("ProtonGE checksum verified for {}", tarball);

                // Extract into a staging directory and rename the version
                // directory into place: `detect_proton_versions` lists any
                // directory with `proton` + `version` files, so extracting in
                // place would offer a half-written Proton while tar runs.
                let staging = proton_dir.join(format!(
                    ".extract.{}.{}",
                    std::process::id(),
                    uuid::Uuid::new_v4()
                ));
                let _ = fs::create_dir_all(&staging);
                let status = std::process::Command::new("tar")
                    .args([
                        "-xzf",
                        &tarball_path.to_string_lossy(),
                        "-C",
                        &staging.to_string_lossy(),
                    ])
                    .status();

                // Only the staging directory may be removed on failure — never
                // the shared parent, which holds other installed Proton versions.
                let extracted_dir = proton_dir.join(&tag);
                match status {
                    Ok(s) if s.success() => {
                        match fs::rename(staging.join(&tag), &extracted_dir) {
                            Ok(()) => log::info!("Successfully extracted ProtonGE"),
                            Err(e) => {
                                log::error!("Failed to move extracted ProtonGE into place: {}", e);
                                PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                    Ok(s) => {
                        log::error!("Failed to extract ProtonGE: tar exited with status {}", s);
                        PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                    }
                    Err(e) => {
                        log::error!("Failed to extract ProtonGE: failed to spawn tar: {}", e);
                        PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                    }
                }
                let _ = fs::remove_dir_all(&staging);
                if let Err(e) = fs::remove_file(&tarball_path) {
                    log::warn!("failed to remove tarball {}: {e}", tarball_path.display());
                }
            } else {
                if let Err(e) = fs::remove_file(&tarball_path) {
                    log::warn!("failed to remove tarball {}: {e}", tarball_path.display());
                }
                PROTONGE_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
            }
        })
        .await;
    });
}

/// Parses a `sha512sum`-style sums file (`<hex-digest>  <filename>` per line,
/// optionally binary-mode `*`-prefixed) and returns the lowercased digest for
/// `file_name`, if present.
fn parse_sha512sum_line(sums_text: &str, file_name: &str) -> Option<String> {
    sums_text.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        if name.trim().trim_start_matches('*') == file_name {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_matching_line() {
        let sums = "d5792f4a  GE-Proton11-1.tar.gz\n";
        assert_eq!(
            parse_sha512sum_line(sums, "GE-Proton11-1.tar.gz"),
            Some("d5792f4a".to_string())
        );
    }

    #[test]
    fn missing_entry_returns_none() {
        let sums = "aaaa  other-file.tar.gz\n";
        assert_eq!(parse_sha512sum_line(sums, "GE-Proton11-1.tar.gz"), None);
    }

    #[test]
    fn picks_correct_line_among_several() {
        let sums = "aaaa  a.tar.gz\nbbbb  b.tar.gz\n";
        assert_eq!(parse_sha512sum_line(sums, "b.tar.gz"), Some("bbbb".to_string()));
    }

    #[test]
    fn handles_binary_mode_prefix_and_mixed_case() {
        let sums = "ABCD *GE-Proton11-1.tar.gz\n";
        assert_eq!(
            parse_sha512sum_line(sums, "GE-Proton11-1.tar.gz"),
            Some("abcd".to_string())
        );
    }
}

