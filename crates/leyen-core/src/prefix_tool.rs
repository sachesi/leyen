//! Programs run in a prefix outside the library: Wine Configuration, the
//! Registry Editor and a program picked by hand. Each runs sandboxed in a
//! transient systemd user scope, as games do, and while one runs its prefix is in
//! use: a game cannot launch on it and the daemon starts no dependency job.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use leyen_model::i18n::gettext;
use leyen_model::models::SandboxFolder;
use log::{info, warn};
use tokio::process::Command as AsyncCommand;

use crate::launch::{
    PrefixProgram, claim_prefix, for_each_output_line, forget_prefix_program, prefix_folder,
    prefix_programs, read_cgroup_populated, record_prefix_program, stop_scope_verified,
    systemctl_show_property, systemd_user_available, unit_is_active,
};
use crate::runtime::umu::{UmuReservation, get_umu_run_path, is_umu_run_available};
use crate::sandbox::{Confined, SandboxRequest, confine_in_scope, is_available};

/// Every program running in a prefix, by the scope unit it runs in.
fn running() -> MutexGuard<'static, HashMap<String, PrefixProgram>> {
    static RUNNING: OnceLock<Mutex<HashMap<String, PrefixProgram>>> = OnceLock::new();
    RUNNING
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Notified with the programs running in prefixes whenever one starts or ends.
/// The daemon installs an emitter here to drive `PrefixProgramsChanged`.
static PROGRAMS_LISTENER: OnceLock<Box<dyn Fn(Vec<PrefixProgram>) + Send + Sync>> = OnceLock::new();

/// Installs the listener. No-op if called more than once.
pub fn set_programs_listener(listener: impl Fn(Vec<PrefixProgram>) + Send + Sync + 'static) {
    let _ = PROGRAMS_LISTENER.set(Box::new(listener));
}

/// The programs running in prefixes, by label.
pub fn programs() -> Vec<PrefixProgram> {
    let mut programs: Vec<PrefixProgram> = running().values().cloned().collect();
    programs.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.unit.cmp(&b.unit)));
    programs
}

fn publish() {
    if let Some(listener) = PROGRAMS_LISTENER.get() {
        listener(programs());
    }
}

/// Whether a program is running in `prefix`, however either is spelled: through
/// a symlink they are one prefix with one wineserver.
pub fn is_running_in(prefix: &str) -> bool {
    let prefix = prefix_folder(prefix);
    running()
        .values()
        .any(|program| prefix_folder(&program.prefix) == prefix)
}

/// Stops the program running in the scope `unit`, and everything it started:
/// SIGTERM, then SIGKILL for what is left after a few seconds. Returns whether
/// it was running. Its prefix is freed once the scope has emptied.
pub async fn stop_program(unit: &str) -> bool {
    if !running().contains_key(unit) {
        return false;
    }
    let unit = unit.to_string();
    info!("Stopping '{unit}'");
    let _ = tokio::task::spawn_blocking(move || stop_scope_verified(&unit)).await;
    true
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
    let umu = UmuReservation::reserve();
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
    // Wine runs here too: sandboxed like a game, refused the same way.
    tokio::task::spawn_blocking(is_available)
        .await
        .unwrap_or_else(|e| Err(e.to_string()))?;

    let is_own_program = !matches!(program, "winecfg" | "regedit");
    let program_folder = working_dir.clone();
    // A program picked by hand sees the folder it sits in, and the preferences'
    // folders; a game's own belong to that game.
    let settings = crate::config::load_settings().await;
    let own_folder = program_folder.clone();
    let shares = tokio::task::spawn_blocking(move || {
        let mut shares = Vec::new();
        if let Some(folder) = own_folder {
            shares.push(crate::sandbox::check_folder(&SandboxFolder {
                path: folder.to_string_lossy().into_owned(),
                writable: false,
            })?);
        }
        let (shared, refused) = crate::sandbox::shares_from(&settings.global_sandbox_folders);
        for reason in refused {
            log::warn!("{reason}");
        }
        shares.extend(shared);
        Ok::<_, String>(shares)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))?;

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
    let prefix_claim = claim_prefix(&prefix).await;
    let sandbox = SandboxRequest {
        prefix_path: prefix.clone(),
        proton_path: proton_path.to_string(),
        work_dir: program_folder,
        shares,
        // An installer run by hand fetches what it installs; winecfg and regedit
        // have no business on the network.
        network: is_own_program,
    };
    let Confined {
        command: mut scoped,
        lease,
    } = confine_in_scope(&cmd, &unit, &sandbox).await?;
    scoped.stdin(Stdio::null());
    scoped.stdout(Stdio::piped());
    scoped.stderr(Stdio::piped());
    let mut child = match scoped.spawn() {
        Ok(child) => child,
        Err(e) => {
            // Nothing enters the prefix's holder now; one started for this program
            // would otherwise idle until the next game on the prefix ends.
            drop(lease);
            crate::sandbox::release_namespace(&prefix).await;
            return Err(gettext("Failed to run {}: {}")
                .replacen("{}", &label, 1)
                .replacen("{}", &e.to_string(), 1));
        }
    };
    lease.spawned(child.id());

    // Also what keeps the daemon from exiting while the program runs. Recorded,
    // so a daemon started after this one dies keeps the prefix in use too.
    let program = PrefixProgram {
        unit: unit.clone(),
        prefix: prefix.clone(),
        label: label.clone(),
    };
    running().insert(unit.clone(), program.clone());
    drop(prefix_claim);
    drop(umu);
    publish();
    if let Err(e) = tokio::task::spawn_blocking(move || record_prefix_program(program))
        .await
        .map_err(|e| e.to_string())
        .and_then(|recorded| recorded.map_err(|e| e.to_string()))
    {
        warn!("Could not record '{label}' as running in '{prefix}': {e}");
    }
    info!("Launched '{label}' inside prefix '{prefix}'");

    if let Some(stdout) = child.stdout.take() {
        log_output(stdout, label.clone(), "stdout");
    }
    if let Some(stderr) = child.stderr.take() {
        log_output(stderr, label.clone(), "stderr");
    }

    tokio::spawn(async move {
        let _ = child.wait().await;
        watch(unit, prefix, label).await;
    });
    Ok(())
}

/// Takes over the programs a daemon before this one left running in a prefix, so
/// their prefixes stay in use until they end. Run once at startup.
pub async fn adopt_running() {
    let programs = match tokio::task::spawn_blocking(prefix_programs).await {
        Ok(Ok(programs)) => programs,
        Ok(Err(e)) => return warn!("Could not read the programs running in prefixes: {e}"),
        Err(e) => return warn!("Could not read the programs running in prefixes: {e}"),
    };
    for mut program in programs {
        if program.label.is_empty() {
            program.label = program.unit.clone();
        }
        running().insert(program.unit.clone(), program.clone());
        tokio::spawn(watch(program.unit, program.prefix, program.label));
    }
    publish();
}

/// Frees `prefix` once the scope `unit` is gone. The program itself may have
/// ended already, but what it started (wineserver, an installer's own
/// processes) can live on in its scope.
async fn watch(unit: String, prefix: String, label: String) {
    // Read from the scope's cgroup, found once; the user manager is asked only
    // while the cgroup is not there.
    let mut cgroup: Option<PathBuf> = None;
    loop {
        let probe = unit.clone();
        let (found, active) = tokio::task::spawn_blocking(move || {
            let found = cgroup.or_else(|| {
                systemctl_show_property(&probe, "ControlGroup")
                    .map(|group| PathBuf::from(format!("/sys/fs/cgroup{group}")))
            });
            let active = found
                .as_deref()
                .and_then(read_cgroup_populated)
                .or_else(|| unit_is_active(&probe));
            (found, active)
        })
        .await
        .unwrap_or_default();
        if active == Some(false) {
            break;
        }
        cgroup = found;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    running().remove(&unit);
    publish();
    let ended = unit.clone();
    if let Ok(Err(e)) = tokio::task::spawn_blocking(move || forget_prefix_program(&ended)).await {
        warn!("Could not record that '{label}' has ended: {e}");
    }
    crate::sandbox::release_namespace(&prefix).await;
    info!("'{label}' in prefix '{prefix}' has ended");
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

#[cfg(test)]
mod tests {
    use super::{PrefixProgram, is_running_in, running};

    #[test]
    fn a_prefix_is_in_use_however_its_path_is_spelled() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("pfx");
        std::fs::create_dir(&prefix).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&prefix, &link).unwrap();
        let unit = "leyen-tool-spelling.scope";
        running().insert(
            unit.to_string(),
            PrefixProgram {
                unit: unit.to_string(),
                prefix: link.to_string_lossy().into_owned(),
                label: "winecfg".to_string(),
            },
        );

        let in_use = is_running_in(&prefix.to_string_lossy());
        let with_slash = is_running_in(&format!("{}/", prefix.display()));
        let other = is_running_in(&dir.path().to_string_lossy());
        running().remove(unit);

        assert!(in_use);
        assert!(with_slash);
        assert!(!other);
    }
}
