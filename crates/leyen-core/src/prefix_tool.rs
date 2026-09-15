//! Programs run in a prefix outside the library: Wine Configuration, the
//! Registry Editor and a program picked by hand. Each runs in a transient systemd
//! user scope, as games do, and while one runs its prefix is in use: a game
//! cannot launch on it and the daemon starts no dependency job.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use leyen_model::i18n::gettext;
use log::info;
use tokio::process::Command as AsyncCommand;

use crate::launch::{for_each_output_line, in_scope, systemd_user_available, unit_is_active};
use crate::runtime::umu::{get_umu_run_path, is_umu_run_available};

/// Scope unit → prefix, for every program running in a prefix.
fn running() -> MutexGuard<'static, HashMap<String, String>> {
    static RUNNING: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    RUNNING
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Whether a program is running in `prefix`. Paths compare by component, so
/// `/x/pfx/` and `/x/pfx` are one prefix.
pub fn is_running_in(prefix: &str) -> bool {
    let prefix = Path::new(prefix.trim());
    running().values().any(|p| Path::new(p) == prefix)
}

/// Whether a program is running in any prefix. The daemon does not exit while one does.
pub fn any_running() -> bool {
    !running().is_empty()
}

/// Starts `program` in `prefix` with the Proton at `proton_path` (empty: the one
/// umu-launcher picks) and returns once it runs. `program` is `winecfg`,
/// `regedit`, or the absolute path of a file, whose folder it runs in. Its output
/// goes to the log, and the prefix stays in use until every process it started
/// has ended.
pub async fn run_in_prefix(program: &str, prefix: &str, proton_path: &str) -> Result<(), String> {
    let prefix = prefix.trim().to_string();
    if prefix.is_empty() {
        return Err(gettext("Prefix path is required"));
    }
    let (label, game_id, working_dir) = match program {
        "winecfg" | "regedit" => (program.to_string(), format!("leyen-{program}"), None),
        path => {
            let file = PathBuf::from(path);
            let checked = file.clone();
            let is_file = tokio::task::spawn_blocking(move || checked.is_file())
                .await
                .unwrap_or(false);
            if !file.is_absolute() || !is_file {
                return Err(gettext("“{}” is not a file").replacen("{}", path, 1));
            }
            let label = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string());
            (
                label,
                "leyen-prefix-run".to_string(),
                file.parent().map(Path::to_path_buf),
            )
        }
    };
    if !tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or(false)
    {
        return Err(gettext(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
    }
    if !tokio::task::spawn_blocking(systemd_user_available)
        .await
        .unwrap_or(false)
    {
        return Err(gettext(
            "A systemd user session is required to run programs in a prefix.",
        ));
    }

    let mut cmd = AsyncCommand::new(get_umu_run_path());
    cmd.arg(program);
    cmd.env("WINEPREFIX", &prefix);
    if !proton_path.is_empty() {
        cmd.env("PROTONPATH", proton_path);
    }
    cmd.env("GAMEID", game_id);
    cmd.env(
        "WINEDLLOVERRIDES",
        "mscoree=b;mshtml=b;winemenubuilder.exe=d",
    );
    cmd.env("WINEDEBUG", "fixme-all");
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    let unit = format!("leyen-tool-{}.scope", uuid::Uuid::new_v4());
    let mut scoped = in_scope(&cmd, &unit);
    scoped.stdin(Stdio::null());
    scoped.stdout(Stdio::piped());
    scoped.stderr(Stdio::piped());
    let mut child = scoped.spawn().map_err(|e| {
        gettext("Failed to run {}: {}")
            .replacen("{}", &label, 1)
            .replacen("{}", &e.to_string(), 1)
    })?;

    // Also what keeps the daemon from exiting while the program runs.
    running().insert(unit.clone(), prefix.clone());
    info!("Launched '{label}' inside prefix '{prefix}'");

    if let Some(stdout) = child.stdout.take() {
        log_output(stdout, label.clone(), "stdout");
    }
    if let Some(stderr) = child.stderr.take() {
        log_output(stderr, label.clone(), "stderr");
    }

    tokio::spawn(async move {
        let _ = child.wait().await;
        // The program itself has ended, but what it started (wineserver, an
        // installer's own processes) can live on in its scope; the prefix is free
        // once systemd says the scope is gone.
        loop {
            let probe = unit.clone();
            let active = tokio::task::spawn_blocking(move || unit_is_active(&probe))
                .await
                .ok()
                .flatten();
            if active == Some(false) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        running().remove(&unit);
        info!("'{label}' in prefix '{prefix}' has ended");
    });
    Ok(())
}

/// Logs each line of a program's `stream` as it comes.
fn log_output<R>(reader: R, label: String, stream: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(for_each_output_line(reader, move |line| {
        if !line.trim().is_empty() {
            info!("[{label}:{stream}] {line}");
        }
    }));
}
