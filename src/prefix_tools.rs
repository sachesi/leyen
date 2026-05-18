use std::path::Path;
use std::process::{Command, Stdio};

use libadwaita as adw;
use log::info;

use gtk4::gio;
use gtk4::prelude::*;

use std::sync::atomic::Ordering;

use crate::logging::LOG_OPERATIONS;
use crate::runtime::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};

pub async fn run_winecfg_in_prefix(
    overlay: &adw::ToastOverlay,
    prefix_path: &str,
    proton_path: &str,
) {
    let snapshots = crate::launch::running_games_snapshot().await;
    if !snapshots.is_empty() {
        overlay.add_toast(adw::Toast::new(
            "Blocked: Cannot run winecfg while games are running.",
        ));
        return;
    }
    if UMU_DOWNLOADING.load(Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }
    if !tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or(false)
    {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return;
    }

    let proton = proton_path.trim().to_string();
    let prefix = prefix_path.trim().to_string();
    if prefix.is_empty() {
        overlay.add_toast(adw::Toast::new("Prefix path is required"));
        return;
    }

    let overlay_clone = overlay.clone();
    let result = tokio::task::spawn_blocking(move || launch_wine_command("winecfg", &prefix, &proton))
        .await
        .unwrap_or_else(|e| Err(format!("blocking task failed: {e}")));
    match result {
        Ok(()) => overlay_clone.add_toast(adw::Toast::new("Wine Configuration launched")),
        Err(err) => overlay_clone.add_toast(adw::Toast::new(&format!("Failed to run winecfg: {err}"))),
    }
}

pub async fn run_regedit_in_prefix(
    overlay: &adw::ToastOverlay,
    prefix_path: &str,
    proton_path: &str,
) {
    let snapshots = crate::launch::running_games_snapshot().await;
    if !snapshots.is_empty() {
        overlay.add_toast(adw::Toast::new(
            "Blocked: Cannot run regedit while games are running.",
        ));
        return;
    }
    if UMU_DOWNLOADING.load(Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }
    if !tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or(false)
    {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return;
    }

    let proton = proton_path.trim().to_string();
    let prefix = prefix_path.trim().to_string();
    if prefix.is_empty() {
        overlay.add_toast(adw::Toast::new("Prefix path is required"));
        return;
    }

    let overlay_clone = overlay.clone();
    let result = tokio::task::spawn_blocking(move || launch_wine_command("regedit", &prefix, &proton))
        .await
        .unwrap_or_else(|e| Err(format!("blocking task failed: {e}")));
    match result {
        Ok(()) => overlay_clone.add_toast(adw::Toast::new("Registry Editor launched")),
        Err(err) => overlay_clone.add_toast(adw::Toast::new(&format!("Failed to run regedit: {err}"))),
    }
}

fn launch_wine_command(name: &str, prefix_path: &str, proton_path: &str) -> Result<(), String> {
    let mut cmd = Command::new(get_umu_run_path());
    cmd.arg(name);
    cmd.env("WINEPREFIX", prefix_path);
    if !proton_path.is_empty() {
        cmd.env("PROTONPATH", proton_path);
    }
    cmd.env("GAMEID", format!("leyen-{name}"));
    cmd.env("WINEDLLOVERRIDES", "mscoree=b;mshtml=b;winemenubuilder.exe=d");
    cmd.env("WINEDEBUG", "fixme-all");
    if !LOG_OPERATIONS.load(Ordering::Relaxed) {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    cmd.spawn()
        .map_err(|err| format!("Failed to launch {}: {}", name, err))?;
    info!("Launched '{}' inside prefix '{}'", name, prefix_path);
    Ok(())
}

pub async fn pick_and_run_in_prefix(
    parent: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    prefix_path: &str,
    proton_path: &str,
) {
    let snapshots = crate::launch::running_games_snapshot().await;
    if !snapshots.is_empty() {
        overlay.add_toast(adw::Toast::new(
            "Blocked: Cannot run programs in prefix while games are running.",
        ));
        return;
    }

    let prefix_path = prefix_path.trim().to_string();
    let proton_path = proton_path.trim().to_string();

    if prefix_path.is_empty() {
        overlay.add_toast(adw::Toast::new("Prefix path is required first"));
        return;
    }

    if UMU_DOWNLOADING.load(Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }

    if !tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or_default()
    {
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

        let prefix_path = prefix_path.clone();
        let proton_path = proton_path.clone();
        gtk4::glib::spawn_future_local(async move {
            let result = tokio::task::spawn_blocking(move || {
                launch_path_in_prefix(&path, &prefix_path, &proton_path)
            })
            .await
            .unwrap_or_else(|e| Err(format!("blocking task failed: {e}")));
            match result {
                Ok(()) => overlay.add_toast(adw::Toast::new("Launched in prefix")),
                Err(err) => {
                    overlay.add_toast(adw::Toast::new(&format!("Failed to run in prefix: {err}")))
                }
            }
        });
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
    if !LOG_OPERATIONS.load(Ordering::Relaxed) {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    cmd.spawn()
        .map_err(|err| format!("Failed to launch '{}': {}", path.display(), err))?;

    info!(
        "Launched '{}' inside prefix '{}'",
        path.display(),
        prefix_path
    );
    Ok(())
}
