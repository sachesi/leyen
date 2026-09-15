//! "Run something in this prefix" (winecfg / regedit / pick a program). The
//! daemon runs them in a scope of their own and keeps the prefix in use until
//! everything they started has ended, so no game launches on it and no
//! dependency job starts meanwhile. They are gated on "no game running" and umu
//! availability here first, for the messages. Each returns the message to show,
//! whether it worked or not.

use gtk4::prelude::*;
use leyen_model::i18n::gettext;
use leyen_model::runtime::is_umu_run_available;

use crate::daemon::{self, gio_blocking, running_games_snapshot};
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
    match daemon::run_in_prefix(prefix_path.trim(), proton_path.trim(), name).await {
        Ok(()) => launched_msg,
        Err(err) => gettext("Failed to run {}: {}")
            .replacen("{}", name, 1)
            .replacen("{}", &err, 1),
    }
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

    let program = path.to_string_lossy();
    Some(
        match daemon::run_in_prefix(prefix_path.trim(), proton_path.trim(), &program).await {
            Ok(()) => gettext("Launched in prefix"),
            Err(err) => gettext("Failed to run in prefix: {}").replacen("{}", &err, 1),
        },
    )
}
