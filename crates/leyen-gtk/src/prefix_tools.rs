//! Untracked "run something in this prefix" helpers (winecfg / regedit / pick a
//! program). These are fire-and-forget umu-run invocations from the client — not
//! managed games — so they don't go through the daemon's scope tracking. They are
//! gated on "no game running" (via the daemon) and umu availability. Each returns
//! the message to show, whether it worked or not.

use std::path::Path;
use std::process::{Command, Stdio};

use gtk4::prelude::*;
use leyen_model::i18n::gettext;
use leyen_model::runtime::{get_umu_run_path, is_umu_run_available};
use log::info;

use crate::daemon::{gio_blocking, running_games_snapshot};
use crate::dialogs::windows_programs_filter;

/// Why a tool cannot run now, if it cannot.
async fn preflight(blocked_msg: String, prefix_path: &str) -> Option<String> {
    if !running_games_snapshot().await.is_empty() {
        return Some(blocked_msg);
    }
    if !gio_blocking(is_umu_run_available).await.unwrap_or(false) {
        return Some(gettext(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
    }
    if prefix_path.trim().is_empty() {
        return Some(gettext("Prefix path is required"));
    }
    None
}

pub async fn run_winecfg(prefix_path: &str, proton_path: &str) -> String {
    run_wine_tool(
        "winecfg",
        gettext("Blocked: Cannot run winecfg while games are running."),
        gettext("Wine Configuration launched"),
        prefix_path,
        proton_path,
    )
    .await
}

pub async fn run_regedit(prefix_path: &str, proton_path: &str) -> String {
    run_wine_tool(
        "regedit",
        gettext("Blocked: Cannot run regedit while games are running."),
        gettext("Registry Editor launched"),
        prefix_path,
        proton_path,
    )
    .await
}

async fn run_wine_tool(
    name: &'static str,
    blocked_msg: String,
    launched_msg: String,
    prefix_path: &str,
    proton_path: &str,
) -> String {
    if let Some(reason) = preflight(blocked_msg, prefix_path).await {
        return reason;
    }
    let prefix = prefix_path.trim().to_string();
    let proton = proton_path.trim().to_string();
    let result = gio_blocking(move || launch_wine_command(name, &prefix, &proton))
        .await
        .unwrap_or_else(|| Err(gettext("Internal error: background task failed")));
    match result {
        Ok(()) => launched_msg,
        Err(err) => gettext("Failed to run {}: {}")
            .replacen("{}", name, 1)
            .replacen("{}", &err, 1),
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
    cmd.env(
        "WINEDLLOVERRIDES",
        "mscoree=b;mshtml=b;winemenubuilder.exe=d",
    );
    cmd.env("WINEDEBUG", "fixme-all");
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|err| err.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    info!("Launched '{}' inside prefix '{}'", name, prefix_path);
    Ok(())
}

/// Asks for a program and runs it in the prefix. `None` when the choice was cancelled.
pub async fn pick_and_run(
    parent: Option<&gtk4::Window>,
    prefix_path: &str,
    proton_path: &str,
) -> Option<String> {
    if let Some(reason) = preflight(
        gettext("Blocked: Cannot run programs in prefix while games are running."),
        prefix_path,
    )
    .await
    {
        return Some(reason);
    }

    let file = gtk4::FileDialog::builder()
        .title(gettext("Select Program"))
        .default_filter(&windows_programs_filter())
        .build()
        .open_future(parent)
        .await
        .ok()?;
    let Some(path) = file.path() else {
        return Some(gettext("Selected file has no local path"));
    };

    let prefix = prefix_path.trim().to_string();
    let proton = proton_path.trim().to_string();
    let result = gio_blocking(move || launch_path_in_prefix(&path, &prefix, &proton))
        .await
        .unwrap_or_else(|| Err(gettext("Internal error: background task failed")));
    Some(match result {
        Ok(()) => gettext("Launched in prefix"),
        Err(err) => gettext("Failed to run in prefix: {}").replacen("{}", &err, 1),
    })
}

fn launch_path_in_prefix(path: &Path, prefix_path: &str, proton_path: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(gettext("“{}” is not a file").replacen("{}", &path.display().to_string(), 1));
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

    let mut child = cmd.spawn().map_err(|err| err.to_string())?;
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
