//! Untracked "run something in this prefix" helpers (winecfg / regedit / pick a
//! program). These are fire-and-forget umu-run invocations from the client — not
//! managed games — so they don't go through the daemon's scope tracking. They are
//! gated on "no game running" (via the daemon) and umu availability.

use leyen_model::t;
use std::path::Path;
use std::process::{Command, Stdio};

use libadwaita as adw;
use log::info;

use adw::prelude::*;
use gtk4::gio;

use crate::daemon::{gio_blocking, running_games_snapshot};
use leyen_model::runtime::{get_umu_run_path, is_umu_run_available};

async fn preflight(overlay: &adw::ToastOverlay, blocked_msg: &str) -> bool {
    if !running_games_snapshot().await.is_empty() {
        overlay.add_toast(adw::Toast::new(blocked_msg));
        return false;
    }
    if !gio_blocking(is_umu_run_available).await {
        overlay.add_toast(adw::Toast::new(&t!(
            "umu-launcher is not installed. Please check your internet connection and restart."
        )));
        return false;
    }
    true
}

pub async fn run_winecfg_in_prefix(
    overlay: &adw::ToastOverlay,
    prefix_path: &str,
    proton_path: &str,
) {
    if !preflight(
        overlay,
        &t!("Blocked: Cannot run winecfg while games are running."),
    )
    .await
    {
        return;
    }

    let proton = proton_path.trim().to_string();
    let prefix = prefix_path.trim().to_string();
    if prefix.is_empty() {
        overlay.add_toast(adw::Toast::new(&t!("Prefix path is required")));
        return;
    }

    let result = gio_blocking(move || launch_wine_command("winecfg", &prefix, &proton)).await;
    match result {
        Ok(()) => overlay.add_toast(adw::Toast::new(&t!("Wine Configuration launched"))),
        Err(err) => overlay.add_toast(adw::Toast::new(&format!("Failed to run winecfg: {err}"))),
    }
}

pub async fn run_regedit_in_prefix(
    overlay: &adw::ToastOverlay,
    prefix_path: &str,
    proton_path: &str,
) {
    if !preflight(
        overlay,
        &t!("Blocked: Cannot run regedit while games are running."),
    )
    .await
    {
        return;
    }

    let proton = proton_path.trim().to_string();
    let prefix = prefix_path.trim().to_string();
    if prefix.is_empty() {
        overlay.add_toast(adw::Toast::new(&t!("Prefix path is required")));
        return;
    }

    let result = gio_blocking(move || launch_wine_command("regedit", &prefix, &proton)).await;
    match result {
        Ok(()) => overlay.add_toast(adw::Toast::new(&t!("Registry Editor launched"))),
        Err(err) => overlay.add_toast(adw::Toast::new(&format!("Failed to run regedit: {err}"))),
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
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to launch {}: {}", name, err))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    info!("Launched '{}' inside prefix '{}'", name, prefix_path);
    Ok(())
}

pub async fn pick_and_run_in_prefix(
    parent: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    prefix_path: &str,
    proton_path: &str,
) {
    if !preflight(
        overlay,
        &t!("Blocked: Cannot run programs in prefix while games are running."),
    )
    .await
    {
        return;
    }

    let prefix_path = prefix_path.trim().to_string();
    let proton_path = proton_path.trim().to_string();
    if prefix_path.is_empty() {
        overlay.add_toast(adw::Toast::new(&t!("Prefix path is required first")));
        return;
    }

    let filter = gtk4::FileFilter::new();
    filter.set_name(Some(&t!("Windows programs")));
    for suffix in ["exe", "msi", "bat", "cmd", "com"] {
        filter.add_suffix(suffix);
    }

    let file_dialog = gtk4::FileDialog::builder()
        .title(t!("Select Program"))
        .default_filter(&filter)
        .build();

    let overlay = overlay.clone();
    file_dialog.open(Some(parent), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else {
            return;
        };
        let Some(path) = file.path() else {
            overlay.add_toast(adw::Toast::new(&t!("Selected file has no local path")));
            return;
        };

        let prefix_path = prefix_path.clone();
        let proton_path = proton_path.clone();
        gtk4::glib::spawn_future_local(async move {
            let result =
                gio_blocking(move || launch_path_in_prefix(&path, &prefix_path, &proton_path))
                    .await;
            match result {
                Ok(()) => overlay.add_toast(adw::Toast::new(&t!("Launched in prefix"))),
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
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to launch '{}': {}", path.display(), err))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    info!(
        "Launched '{}' inside prefix '{}'",
        path.display(),
        prefix_path
    );
    Ok(())
}
