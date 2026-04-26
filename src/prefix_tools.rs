use std::path::Path;
use std::process::{Command, Stdio};

use libadwaita as adw;

use gtk4::gio;
use gtk4::prelude::*;

use crate::logging::{LOG_OPERATIONS, leyen_log};
use crate::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};

pub fn pick_and_run_in_prefix(
    parent: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    prefix_path: &str,
    proton_path: &str,
) {
    let prefix_path = prefix_path.trim().to_string();
    let proton_path = proton_path.trim().to_string();

    if prefix_path.is_empty() {
        overlay.add_toast(adw::Toast::new("Prefix path is required first"));
        return;
    }

    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }

    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return;
    }

    let filter = gtk4::FileFilter::new();
    filter.set_name(Some("Windows programs"));
    for suffix in ["exe", "msi", "bat", "cmd", "com"] {
        filter.add_suffix(suffix);
    }

    let file_dialog = gtk4::FileDialog::builder()
        .title("Select Program")
        .default_filter(&filter)
        .build();

    let overlay = overlay.clone();
    file_dialog.open(Some(parent), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else {
            return;
        };
        let Some(path) = file.path() else {
            overlay.add_toast(adw::Toast::new("Selected file has no local path"));
            return;
        };

        match launch_path_in_prefix(&path, &prefix_path, &proton_path) {
            Ok(()) => overlay.add_toast(adw::Toast::new("Launched in prefix")),
            Err(err) => {
                overlay.add_toast(adw::Toast::new(&format!("Failed to run in prefix: {err}")))
            }
        }
    });
}

fn launch_path_in_prefix(path: &Path, prefix_path: &str, proton_path: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("'{}' is not a file", path.display()));
    }

    let mut cmd = Command::new(get_umu_run_path());
    cmd.arg(path.as_os_str());
    cmd.env("WINEPREFIX", prefix_path);
    if !proton_path.is_empty() {
        cmd.env("PROTONPATH", proton_path);
    }
    cmd.env("GAMEID", "leyen-prefix-run");
    cmd.env(
        "WINEDLLOVERRIDES",
        "mscoree=b;mshtml=b;winemenubuilder.exe=d",
    );
    cmd.env("WINEDEBUG", "fixme-all");
    if let Some(parent) = path.parent()
        && parent.is_dir()
    {
        cmd.current_dir(parent);
    }
    if !LOG_OPERATIONS.load(std::sync::atomic::Ordering::Relaxed) {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    cmd.spawn()
        .map_err(|err| format!("Failed to launch '{}': {}", path.display(), err))?;

    leyen_log(
        "INFO ",
        &format!(
            "Launched '{}' inside prefix '{}'",
            path.display(),
            prefix_path
        ),
    );
    Ok(())
}
