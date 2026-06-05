use leyen_model::t;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use futures::future::join_all;
use log::{error, info, warn};
use sha2::{Digest, Sha256};
use std::io::Read;
use tokio::process::Command as AsyncCommand;

use crate::runtime::umu::{
    UMU_DOWNLOADING, WINETRICKS_DOWNLOAD_STARTED, WINETRICKS_DOWNLOADING, download_winetricks,
    get_umu_run_path, get_winetricks_path, is_umu_run_available, is_winetricks_available,
};

use super::recipes::get_dep_steps;
use super::state::{remove_installed_dep, upsert_installed_dep};
use leyen_model::deps::{
    DepProfile, InstalledDependency, find_installed_dependents, get_dep_profile,
    get_deps_cache_dir, read_prefix_dep_state,
};

const COMMAND_TIMEOUT_SECS: u64 = 600;

/// Prefixes with a dependency install/uninstall in progress. Prevents two
/// concurrent operations on the same prefix (e.g. the deps page opened from both
/// Preferences and a game's edit dialog) from interleaving their prefix
/// snapshots and corrupting the tracked file list.
fn busy_prefixes() -> &'static Mutex<HashSet<String>> {
    static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    BUSY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Releases the prefix lock on drop.
struct PrefixOpGuard(String);

impl Drop for PrefixOpGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = busy_prefixes().lock() {
            set.remove(&self.0);
        }
    }
}

/// Acquires an exclusive lock for dependency operations on `prefix`, or returns
/// `None` if one is already in progress.
fn try_lock_prefix_op(prefix: &str) -> Option<PrefixOpGuard> {
    let mut set = busy_prefixes().lock().ok()?;
    if !set.insert(prefix.to_string()) {
        return None;
    }
    Some(PrefixOpGuard(prefix.to_string()))
}

fn join_err(e: tokio::task::JoinError) -> String {
    if e.is_panic() {
        format!("blocking task panicked: {e}")
    } else {
        format!("blocking task cancelled: {e}")
    }
}

/// Runs a umu command to completion with a timeout, killing the whole process
/// group if the operation is cancelled or times out. The command is put in its
/// own process group via `setpgid` so the entire umu/wine tree is signalled.
async fn run_umu_command(
    mut cmd: AsyncCommand,
    label: String,
    cancel: Arc<AtomicBool>,
) -> Result<std::process::Output, String> {
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }

    // Run on the Tokio runtime (not the GTK/glib executor) so the process and
    // timer drivers advance while the main loop stays responsive for Cancel.
    tokio::spawn(run_umu_command_inner(cmd, label, cancel))
        .await
        .map_err(|e| format!("Command task panicked: {e}"))
        .and_then(|r| r)
}

async fn run_umu_command_inner(
    mut cmd: AsyncCommand,
    label: String,
    cancel: Arc<AtomicBool>,
) -> Result<std::process::Output, String> {
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to launch {}: {}", label, e))?;
    let pid = child.id();
    let wait_fut = child.wait_with_output();
    tokio::pin!(wait_fut);

    let start = tokio::time::Instant::now();
    let timeout = Duration::from_secs(COMMAND_TIMEOUT_SECS);

    let kill_group = || {
        if let Some(pid) = pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    };

    loop {
        tokio::select! {
            out = &mut wait_fut => {
                return out.map_err(|e| format!("Failed to run {}: {}", label, e));
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if cancel.load(Ordering::Relaxed) {
                    kill_group();
                    let _ = (&mut wait_fut).await;
                    return Err(t!("Cancelled."));
                }
                if start.elapsed() >= timeout {
                    warn!("[dep] '{}' timed out after {} seconds", label, COMMAND_TIMEOUT_SECS);
                    kill_group();
                    let _ = (&mut wait_fut).await;
                    return Err(format!(
                        "'{}' timed out after {} seconds",
                        label, COMMAND_TIMEOUT_SECS
                    ));
                }
            }
        }
    }
}

#[derive(Clone)]
pub enum DepStepAction {
    DownloadFile {
        url: &'static str,
        file_name: &'static str,
        sha256: &'static str,
    },
    RunExe {
        file_name: &'static str,
        args: &'static str,
        extra_env: &'static str,
    },
    RunMsi {
        file_name: &'static str,
        args: &'static str,
    },
    OverrideDlls {
        dlls: &'static str,
    },
    RunWinetricks {
        verb: String,
    },
    Verify {
        description: &'static str,
        action: VerifyAction,
    },
}

#[derive(Clone)]
pub enum VerifyAction {
    RegistryKeyExists { path: &'static str },
}

#[derive(Clone)]
pub struct DepStep {
    pub description: &'static str,
    pub action: DepStepAction,
}



#[derive(Default, Debug)]
pub struct StepChanges {
    pub created_files: Vec<String>,
    pub touched_existing_files: bool,
    pub dll_overrides: Vec<String>,
    pub registered_dlls: Vec<String>,
}

impl StepChanges {
    fn merge(&mut self, other: StepChanges) {
        self.created_files = merge_unique_strings(&self.created_files, &other.created_files);
        self.touched_existing_files |= other.touched_existing_files;
        self.dll_overrides = merge_unique_strings(&self.dll_overrides, &other.dll_overrides);
        self.registered_dlls = merge_unique_strings(&self.registered_dlls, &other.registered_dlls);
    }

    fn into_dependency_record(self) -> InstalledDependency {
        InstalledDependency {
            created_files: self.created_files,
            touched_existing_files: self.touched_existing_files,
            dll_overrides: self.dll_overrides,
            registered_dlls: self.registered_dlls,
            ..InstalledDependency::default()
        }
    }
}

#[derive(Default)]
struct PrefixSnapshot {
    files: BTreeMap<String, FileFingerprint>,
}

/// File signature: (len, mtime). May miss same-size+same-mtime in-place edits.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified_epoch_seconds: u64,
}

enum CleanupAction {
    RemoveDllOverrides(Vec<String>),
    UnregisterDlls(Vec<String>),
    RemoveCreatedFiles(Vec<String>),
}



pub async fn execute_dep_step(
    step: &DepStep,
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<StepChanges, String> {
    // Verify tokio runtime context for timeout/process drivers
    match tokio::runtime::Handle::try_current() {
        Ok(_) => info!("[dep] Tokio runtime available for step '{}'", step.description),
        Err(_) => warn!("[dep] No tokio runtime context! Timeout won't fire."),
    }
    match &step.action {
        DepStepAction::DownloadFile {
            url,
            file_name,
            sha256,
        } => {
            if !url.starts_with("https://") {
                return Err(format!(
                    "Refusing to download '{}' from a non-HTTPS source",
                    file_name
                ));
            }

            let dest = Path::new(cache_dir).join(file_name);
            let d = dest.clone();
            let exists = tokio::task::spawn_blocking(move || d.exists())
                .await
                .unwrap_or(true);
            if !exists {
                info!("[dep] Downloading {} from {}", file_name, url);
                let cache_dir_clone = cache_dir.to_string();
                tokio::task::spawn_blocking(move || fs::create_dir_all(cache_dir_clone))
                    .await
                    .map_err(join_err)
                    .and_then(|r| r.map_err(|err| format!("Failed to create dependency cache directory: {err}")))?;

                let output = AsyncCommand::new("curl")
                    .args([
                        "--proto",
                        "=https",
                        "--tlsv1.2",
                        "--silent",
                        "--show-error",
                        "--fail",
                        "--location",
                        "--connect-timeout",
                        "15",
                        "--max-time",
                        "300",
                        "--retry",
                        "3",
                        "--retry-delay",
                        "1",
                        "-o",
                        dest.to_string_lossy().as_ref(),
                        url,
                    ])
                    .output()
                    .await
                    .map_err(|err| format!("curl unavailable: {err}"))?;

                if !output.status.success() {
                    let _ = fs::remove_file(&dest);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("Download failed for {}: {}", file_name, stderr.trim()));
                }
                info!("[dep] Downloaded {}", file_name);
            } else {
                info!("[dep] {} already cached, skipping download", file_name);
            }

            { let expected_sha = *sha256;
                info!("[dep] Verifying SHA256 for {}", file_name);
                let dest_clone = dest.clone();
                let expected_sha = expected_sha.to_string();
                let file_name = *file_name;
                tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let mut file = fs::File::open(&dest_clone)
                        .map_err(|err| format!("Failed to open downloaded file: {err}"))?;
                    let mut hasher = Sha256::new();
                    let mut buffer = [0u8; 8192];
                    loop {
                        let n = file
                            .read(&mut buffer)
                            .map_err(|err| format!("Failed to read downloaded file: {err}"))?;
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buffer[..n]);
                    }
                    let actual_sha = hex::encode(hasher.finalize());
                    if actual_sha != expected_sha {
                        let _ = fs::remove_file(&dest_clone);
                        return Err(format!(
                            "Checksum mismatch for {}: expected {}, got {}",
                            file_name, expected_sha, actual_sha
                        ));
                    }
                    Ok(())
                })
                .await
                .map_err(join_err).and_then(|r| r)?;
                info!("[dep] SHA256 verified for {}", file_name);
            }

            Ok(StepChanges::default())
        }

        DepStepAction::RunExe {
            file_name,
            args,
            extra_env,
        } => {
            let exe_path = Path::new(cache_dir).join(file_name);
            info!("[dep] Running {} {} (timeout: {}s)", file_name, args, COMMAND_TIMEOUT_SECS);
            let output = {
                let mut cmd = AsyncCommand::new(get_umu_run_path());
                configure_umu_command_async(&mut cmd, prefix_path, proton_path);
                for pair in extra_env.split_whitespace() {
                    if let Some(eq) = pair.find('=') {
                        cmd.env(&pair[..eq], &pair[eq + 1..]);
                    }
                }
                cmd.arg(exe_path.to_string_lossy().as_ref());
                for arg in args.split_whitespace() {
                    cmd.arg(arg);
                }
                run_umu_command(cmd, file_name.to_string(), cancel.clone()).await?
            };

            let exit_code = output.status.code();
            info!("[dep] {} completed (exit code: {:?})", file_name, exit_code);
            if !output.status.success() && exit_code != Some(3010) {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "Installer '{}' failed (code {}): {}",
                    file_name,
                    exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    stderr.trim()
                ));
            }

            if exit_code == Some(3010) {
                info!(
                    "[dep] Installer '{}' requested a reboot (3010) — continuing.",
                    file_name
                );
            }

            Ok(StepChanges::default())
        }

        DepStepAction::RunMsi { file_name, args } => {
            let msi_path = Path::new(cache_dir).join(file_name);
            info!("[dep] Running msiexec /i {} {} (timeout: {}s)", file_name, args, COMMAND_TIMEOUT_SECS);
            let output = {
                let mut cmd = AsyncCommand::new(get_umu_run_path());
                configure_umu_command_async(&mut cmd, prefix_path, proton_path);
                cmd.args(["msiexec.exe", "/i"]);
                cmd.arg(msi_path.to_string_lossy().as_ref());
                for arg in args.split_whitespace() {
                    cmd.arg(arg);
                }
                run_umu_command(cmd, file_name.to_string(), cancel.clone()).await?
            };

            let exit_code = output.status.code();
            info!("[dep] msiexec {} completed (exit code: {:?})", file_name, exit_code);
            if !output.status.success() && exit_code != Some(3010) {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "MSI install '{}' failed (code {:?}): {}",
                    file_name, exit_code, stderr.trim()
                ));
            }

            if exit_code == Some(3010) {
                info!(
                    "[dep] MSI installer '{}' requested a reboot (3010) — continuing.",
                    file_name
                );
            }

            Ok(StepChanges::default())
        }

        DepStepAction::OverrideDlls { dlls, .. } => {
            let overrides = split_csv_values(dlls);
            info!("[dep] Applying DLL overrides: {:?}", overrides);
            Ok(StepChanges {
                dll_overrides: overrides,
                ..StepChanges::default()
            })
        }

        DepStepAction::RunWinetricks { verb } => {
            info!("[dep] Running winetricks {} (timeout: {}s)", verb, COMMAND_TIMEOUT_SECS);
            let output = {
                let mut cmd = AsyncCommand::new(get_umu_run_path());
                configure_umu_command_async(&mut cmd, prefix_path, proton_path);
                cmd.args([get_winetricks_path().as_str(), "-q", verb.as_str()]);
                run_umu_command(cmd, format!("winetricks {}", verb), cancel.clone()).await?
            };
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("winetricks '{}' failed: {}", verb, stderr.trim()));
            }
            info!("[dep] winetricks {} completed", verb);

            // Winetricks verbs may set registry DLL overrides invisible to file
            // snapshots; record the known ones so uninstall can revert them.
            let mut changes = StepChanges::default();
            let known = winetricks_known_dll_overrides(verb);
            if !known.is_empty() {
                info!("[dep] Recording {} known DLL overrides for winetricks {}", known.len(), verb);
                changes.dll_overrides = known;
            }
            Ok(changes)
        }

        DepStepAction::Verify {
            description,
            action,
        } => {
            info!("[dep] Verifying: {}", description);
            let verified = match action {
                VerifyAction::RegistryKeyExists { path } => {
                    let prefix_path = prefix_path.to_string();
                    let proton_path = proton_path.to_string();
                    let path = path.to_string();
                    tokio::task::spawn_blocking(move || {
                        super::verify::check_registry_key_exists(&prefix_path, &proton_path, &path)
                    })
                    .await
                    .map_err(join_err).and_then(|r| r)?
                }

            };

            if !verified {
                warn!("[dep] Verification failed: {}", description);
                return Err(format!("Verification failed: {}", description));
            }
            info!("[dep] Verified: {}", description);
            Ok(StepChanges::default())
        }
    }
}

fn configure_umu_command_async(cmd: &mut AsyncCommand, prefix_path: &str, proton_path: &str) {
    cmd.env("WINEPREFIX", prefix_path);
    if !proton_path.is_empty() {
        cmd.env("PROTONPATH", proton_path);
    }
    cmd.env("GAMEID", "leyen-dep-install");
    cmd.env(
        "WINEDLLOVERRIDES",
        "mscoree=b;mshtml=b;winemenubuilder.exe=d",
    );
    cmd.env("WINEDEBUG", "fixme-all");
}

/// Installs a dependency (and its prerequisites) into a prefix. Runtime-agnostic:
/// progress is reported via `on_progress`; the terminal outcome is the `Result`
/// (`Ok(Some(note))`/`Ok(None)` on success, `Err(message)` on failure). The
/// daemon spawns this and maps it onto `DepProgress`/`DepFinished` signals.
pub async fn install_dep(
    dep_id: &str,
    prefix_path: &str,
    proton_path: &str,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(usize, usize, String) + Send + 'static,
) -> Result<Option<String>, String> {
    let profile = get_dep_profile(dep_id);
    let needs_winetricks = profile
        .map(|p| {
            get_dep_steps(p.id)
                .iter()
                .any(|s| matches!(s.action, DepStepAction::RunWinetricks { .. }))
        })
        .unwrap_or(false);

    let dep_id = dep_id.to_string();
    let prefix_path = prefix_path.to_string();
    let proton_path = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();

    let Some(_prefix_guard) = try_lock_prefix_op(&prefix_path) else {
        return Err(t!(
            "Another dependency operation is already running for this prefix."
        ));
    };

    // Preflight: the dependency cache must be writable for downloads.
    let cache_check = cache_dir.clone();
    tokio::task::spawn_blocking(move || fs::create_dir_all(&cache_check))
        .await
        .map_err(join_err)
        .and_then(|r| {
            r.map_err(|e| {
                t!("Cannot write to the dependency cache directory: {}")
                    .replacen("{}", &e.to_string(), 1)
            })
        })?;

    ensure_umu_ready(needs_winetricks, &on_progress).await?;

    let prefix_path_for_state = prefix_path.clone();
    let state = tokio::task::spawn_blocking(move || read_prefix_dep_state(&prefix_path_for_state))
        .await
        .unwrap_or_default();

    let install_plan = match build_install_plan(&dep_id, &state) {
        Ok(plan) if !plan.is_empty() => plan,
        Ok(_) => return Ok(Some(t!("Dependency is already installed."))),
        Err(message) => return Err(message),
    };

    let total_steps = install_plan
        .iter()
        .map(|profile| get_dep_steps(profile.id).len())
        .sum::<usize>();
    if total_steps == 0 {
        return Err(format!("No install steps defined for '{}'", dep_id));
    }

    info!(
        "[dep:{}] starting install plan ({} profiles, {} steps)",
        dep_id,
        install_plan.len(),
        total_steps
    );

    let mut completed_steps = 0usize;
    for profile in &install_plan {
        let steps = get_dep_steps(profile.id);
        let mut recorded = StepChanges::default();

        // Parallelize independent downloads
        let download_steps: Vec<&DepStep> = steps.iter().filter(|s| matches!(s.action, DepStepAction::DownloadFile { .. })).collect();
        let execution_steps: Vec<&DepStep> = steps.iter().filter(|s| !matches!(s.action, DepStepAction::DownloadFile { .. })).collect();

        if !download_steps.is_empty() {
            let description = "Downloading files…";
            info!("[dep:{}] {} {}/{}", profile.id, description, completed_steps + 1, total_steps);
            on_progress(completed_steps + 1, total_steps, description.to_string());

            let futures: Vec<_> = download_steps.iter().map(|step| {
                execute_dep_step(step, &prefix_path, &proton_path, &cache_dir, &cancel)
            }).collect();

            let results = join_all(futures).await;
            for result in results {
                match result {
                    Ok(changes) => recorded.merge(changes),
                    Err(error) => {
                        error!("[dep:{}] download failed: {}", profile.id, error);
                        return Err(error);
                    }
                }
            }
            completed_steps += download_steps.len();
        }

        // Snapshot the prefix once before the file-creating steps; all new
        // or changed files are attributed to this profile after they run.
        // This is cheaper than per-step scans and more robust.
        let snapshot_before = {
            let p = prefix_path.clone();
            tokio::task::spawn_blocking(move || snapshot_prefix(&p))
                .await
                .map_err(join_err)
                .and_then(|r| r)?
        };

        let mut step_error: Option<String> = None;
        for step in &execution_steps {
            if cancel.load(Ordering::Relaxed) {
                step_error = Some(t!("Cancelled."));
                break;
            }
            completed_steps += 1;
            let description = if install_plan.len() > 1 {
                format!("{}: {}", profile.name, step.description)
            } else {
                step.description.to_string()
            };

            info!(
                "[dep:{}] step {}/{}: {}",
                profile.id, completed_steps, total_steps, description
            );
            on_progress(completed_steps, total_steps, description);

            match execute_dep_step(step, &prefix_path, &proton_path, &cache_dir, &cancel).await {
                Ok(changes) => recorded.merge(changes),
                Err(error) => {
                    error!("[dep:{}] install failed: {}", profile.id, error);
                    step_error = Some(error);
                    break;
                }
            }
        }

        // Diff once after the steps so we know what this attempt created.
        {
            let p = prefix_path.clone();
            if let Ok(after) = tokio::task::spawn_blocking(move || snapshot_prefix(&p))
                .await
                .map_err(join_err)
                .and_then(|r| r)
            {
                recorded.merge(diff_snapshots(&snapshot_before, &after));
            }
        }

        // On cancel or a failed step, do NOT mark the dependency installed.
        // Best-effort remove the files this attempt created so the prefix is
        // left clean and the entry stays in the available list.
        if let Some(error) = step_error {
            let cleanup_files = recorded.created_files.clone();
            if !cleanup_files.is_empty() {
                let cleanup_prefix = prefix_path.clone();
                info!(
                    "[dep:{}] rolling back {} files from the cancelled/failed attempt",
                    profile.id,
                    cleanup_files.len()
                );
                if let Err(cleanup_err) = tokio::task::spawn_blocking(move || {
                    remove_created_files(&cleanup_prefix, &cleanup_files)
                })
                .await
                .map_err(join_err)
                .and_then(|r| r) {
                    warn!("[dep:{}] cleanup failed during rollback: {}", profile.id, cleanup_err);
                }
            }
            return Err(error);
        }

        {
            let upsert_prefix = prefix_path.clone();
            let upsert_id = profile.id.to_string();
            let upsert_deps: Vec<String> = profile.dependencies.iter().map(|d| d.to_string()).collect();
            let upsert_record = recorded.into_dependency_record();
            tokio::task::spawn_blocking(move || {
                let upsert_deps_refs: Vec<&str> = upsert_deps.iter().map(|d| d.as_str()).collect();
                upsert_installed_dep(&upsert_prefix, &upsert_id, &upsert_deps_refs, &upsert_record)
            }).await.map_err(join_err).and_then(|r| r)?;
        }

        for provided_id in profile.provides {
            let stub_prefix = prefix_path.clone();
            let stub_id = provided_id.to_string();
            let stub = InstalledDependency::default();
            let deps: Vec<&str> = Vec::new();
            if let Err(error) = tokio::task::spawn_blocking(move || {
                upsert_installed_dep(&stub_prefix, &stub_id, &deps, &stub)
            }).await.map_err(join_err).and_then(|r| r) {
                warn!("[dep:{}] failed to create stub for '{}': {}", profile.id, provided_id, error);
            }
        }
    }

    let note = if install_plan.len() > 1 {
        let prerequisites = install_plan
            .iter()
            .map(|profile| profile.id)
            .filter(|profile_id| *profile_id != dep_id)
            .collect::<Vec<_>>();
        if prerequisites.is_empty() {
            None
        } else {
            Some(
                t!("Installed prerequisites: {}.")
                    .replacen("{}", &prerequisites.join(", "), 1),
            )
        }
    } else {
        None
    };

    info!("[dep:{}] install complete", dep_id);
    Ok(note)
}

/// Removes a tracked dependency from a prefix. Runtime-agnostic: progress via
/// `on_progress`, terminal outcome via the `Result`.
pub async fn uninstall_dep(
    dep_id: &str,
    prefix_path: &str,
    proton_path: &str,
    on_progress: impl Fn(usize, usize, String) + Send + 'static,
) -> Result<Option<String>, String> {
    let dep_id = dep_id.to_string();
    let prefix_path = prefix_path.to_string();
    let proton_path = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();

    let Some(_prefix_guard) = try_lock_prefix_op(&prefix_path) else {
        return Err(t!(
            "Another dependency operation is already running for this prefix."
        ));
    };
    let prefix_path_for_state = prefix_path.clone();
    let state = tokio::task::spawn_blocking(move || read_prefix_dep_state(&prefix_path_for_state))
        .await
        .unwrap_or_default();

    let installed = match state.installed.get(&dep_id).cloned() {
        Some(installed) => installed,
        None => return Ok(Some(t!("Dependency is no longer tracked."))),
    };
    let dependents = find_installed_dependents(&state, &dep_id);
    if !dependents.is_empty() {
        return Err(t!("Cannot remove '{}': still required by {}.")
            .replacen("{}", &dep_id, 1)
            .replacen("{}", &dependents.join(", "), 1));
    }

    let actions = build_cleanup_actions(&installed);

    // Detect winetricks-based deps and uninstall their registry markers first
    // so reinstall doesn't fail with "already installed"
    let winetricks_verbs: Vec<String> = get_dep_steps(&dep_id).iter()
        .filter_map(|step| match &step.action {
            DepStepAction::RunWinetricks { verb } => Some(verb.clone()),
            _ => None,
        })
        .collect();

    let requires_umu = actions.iter().any(|(_, action)| {
        matches!(
            action,
            CleanupAction::RemoveDllOverrides(_) | CleanupAction::UnregisterDlls(_)
        )
    });
    let needs_umu = requires_umu || !winetricks_verbs.is_empty();
    if needs_umu {
        ensure_umu_ready(false, &on_progress).await?;
    }

    // Async winetricks uninstall before sync cleanup loop
    for verb in &winetricks_verbs {
        on_progress(0, 0, format!("Uninstalling winetricks '{}'…", verb));
        let prefix_path = prefix_path.clone();
        let proton_path = proton_path.clone();
        let verb = verb.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let result: Result<(), String> = {
            let mut cmd = AsyncCommand::new(get_umu_run_path());
            configure_umu_command_async(&mut cmd, &prefix_path, &proton_path);
            cmd.args([get_winetricks_path().as_str(), "--uninstall", &verb]);
            run_umu_command(cmd, format!("winetricks --uninstall {}", verb), cancel.clone()).await.map(|_| ())
        };
        if let Err(e) = result {
            warn!("[dep:{}] winetricks uninstall warning: {}", dep_id, e);
        }
    }

    info!(
        "[dep:{}] starting removal ({} cleanup actions)",
        dep_id,
        actions.len()
    );

    let total_actions = actions.len();
    for (index, (description, action)) in actions.into_iter().enumerate() {
        on_progress(index + 1, total_actions, description.clone());

        let prefix_path = prefix_path.clone();
        let proton_path = proton_path.clone();
        let cache_dir = cache_dir.clone();

        tokio::task::spawn_blocking(move || match action {
            CleanupAction::RemoveDllOverrides(dlls) => {
                remove_dll_overrides(&prefix_path, &proton_path, &cache_dir, &dlls)
            }
            CleanupAction::UnregisterDlls(dlls) => {
                unregister_dlls(&prefix_path, &proton_path, &dlls)
            }
            CleanupAction::RemoveCreatedFiles(files) => {
                remove_created_files(&prefix_path, &files)
            }
        })
        .await
        .map_err(join_err)
        .and_then(|r| r)
        .inspect_err(|error| error!("[dep:{}] removal failed: {}", dep_id, error))?;
    }

    if let Some(profile) = get_dep_profile(&dep_id) {
        for provided_id in profile.provides {
            if let Some(entry) = state.installed.get(*provided_id)
                && !entry.has_removable_changes() && !entry.touched_existing_files {
                    let remove_prefix = prefix_path.clone();
                    let remove_id = provided_id.to_string();
                    if let Err(error) = tokio::task::spawn_blocking(move || {
                        remove_installed_dep(&remove_prefix, &remove_id)
                    }).await.map_err(join_err).and_then(|r| r) {
                        warn!("[dep:{}] failed to remove stub for '{}': {}", dep_id, provided_id, error);
                    }
                }
        }
    }

    {
        let remove_prefix = prefix_path.clone();
        let remove_id = dep_id.clone();
        tokio::task::spawn_blocking(move || {
            remove_installed_dep(&remove_prefix, &remove_id)
        }).await.map_err(join_err).and_then(|r| r)?;
    }

    let note = match (installed.has_removable_changes(), installed.touched_existing_files) {
        (true, true) => Some(t!(
            "Some existing prefix files were changed during installation and were not reverted."
        )),
        (false, true) => Some(t!(
            "Removed from tracking. Existing prefix files changed during installation were not reverted."
        )),
        (false, false) => Some(t!("Removed from tracking.")),
        (true, false) => None,
    };

    info!("[dep:{}] removal complete", dep_id);
    Ok(note)
}

async fn ensure_umu_ready<F: Fn(usize, usize, String)>(
    check_winetricks: bool,
    on_progress: &F,
) -> Result<(), String> {
    info!("[dep] Checking umu-launcher availability…");
    if UMU_DOWNLOADING.load(Ordering::Relaxed) {
        return Err(t!("umu-launcher is still downloading, please wait…"));
    }

    if !tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or_else(|e| {
            log::warn!("is_umu_run_available task failed: {}", join_err(e));
            false
        })
    {
        info!("[dep] umu-launcher not available");
        return Err(t!(
            "umu-launcher is not installed. Please check your internet connection and restart."
        ));
    }
    info!("[dep] umu-launcher is available");

    if check_winetricks {
        info!("[dep] Checking winetricks availability…");
        if WINETRICKS_DOWNLOADING.load(Ordering::Relaxed) {
            return Err(t!("winetricks is still downloading, please wait…"));
        }

        if !tokio::task::spawn_blocking(is_winetricks_available)
            .await
            .unwrap_or_else(|e| {
                log::warn!("is_winetricks_available task failed: {}", join_err(e));
                false
            })
        {
            info!("[dep] winetricks not found, triggering download");
            on_progress(0, 0, t!("Downloading winetricks…"));

            if !WINETRICKS_DOWNLOAD_STARTED.swap(true, Ordering::Relaxed) {
                info!("[dep] Starting winetricks download…");
                WINETRICKS_DOWNLOADING.store(true, Ordering::Relaxed);
                let result = tokio::task::spawn_blocking(download_winetricks)
                    .await
                    .map_err(join_err)
                    .and_then(|r| r.map_err(|e| e.to_string()));
                if result.is_err() {
                    warn!("[dep] winetricks download failed");
                    WINETRICKS_DOWNLOAD_STARTED.store(false, Ordering::Relaxed);
                } else {
                    info!("[dep] winetricks download completed");
                }
                WINETRICKS_DOWNLOADING.store(false, Ordering::Relaxed);
                result.map_err(|_| {
                    t!("Failed to download winetricks. Check your internet connection.")
                })?;
            }

            // Wait for the download to complete if another caller started it
            if WINETRICKS_DOWNLOADING.load(Ordering::Relaxed) {
                info!("[dep] Waiting for winetricks download from another caller…");
            }
            while WINETRICKS_DOWNLOADING.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }

            if !tokio::task::spawn_blocking(is_winetricks_available)
                .await
                .unwrap_or_else(|e| {
                    log::warn!("is_winetricks_available task failed: {}", join_err(e));
                    false
                })
            {
                return Err("winetricks not available after download".to_string());
            }
            info!("[dep] winetricks is available");
        } else {
            info!("[dep] winetricks already available");
        }
    }

    Ok(())
}

fn configure_umu_command(cmd: &mut std::process::Command, prefix_path: &str, proton_path: &str) {
    cmd.env("WINEPREFIX", prefix_path);
    if !proton_path.is_empty() {
        cmd.env("PROTONPATH", proton_path);
    }
    cmd.env("GAMEID", "leyen-dep-install");
    cmd.env(
        "WINEDLLOVERRIDES",
        "mscoree=b;mshtml=b;winemenubuilder.exe=d",
    );
    cmd.env("WINEDEBUG", "fixme-all");
}

fn build_install_plan(
    dep_id: &str,
    state: &leyen_model::deps::PrefixDependencyState,
) -> Result<Vec<&'static DepProfile>, String> {
    let mut visiting = BTreeSet::new();
    let mut planned_ids = BTreeSet::new();
    let mut planned = Vec::new();
    append_install_plan(
        dep_id,
        state,
        &mut visiting,
        &mut planned_ids,
        &mut planned,
        true,
    )?;
    Ok(planned)
}

fn append_install_plan(
    dep_id: &str,
    state: &leyen_model::deps::PrefixDependencyState,
    visiting: &mut BTreeSet<String>,
    planned_ids: &mut BTreeSet<String>,
    planned: &mut Vec<&'static DepProfile>,
    include_even_if_installed: bool,
) -> Result<(), String> {
    if planned_ids.contains(dep_id) {
        return Ok(());
    }

    if !visiting.insert(dep_id.to_string()) {
        return Err(format!("Dependency cycle detected for '{}'", dep_id));
    }

    let profile = get_dep_profile(dep_id)
        .ok_or_else(|| format!("No dependency profile found for '{}'", dep_id))?;

    for dependency in profile.dependencies {
        append_install_plan(dependency, state, visiting, planned_ids, planned, false)?;
    }

    let should_add = include_even_if_installed || !state.installed.contains_key(dep_id);
    if should_add && planned_ids.insert(dep_id.to_string()) {
        planned.push(profile);
    }

    visiting.remove(dep_id);
    Ok(())
}

fn split_csv_values(values: &str) -> Vec<String> {
    values
        .split(',')
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .collect()
}

fn merge_unique_strings(existing: &[String], additional: &[String]) -> Vec<String> {
    let mut merged: BTreeSet<String> = existing.iter().cloned().collect();
    merged.extend(additional.iter().cloned());
    merged.into_iter().collect()
}

fn snapshot_prefix(prefix_path: &str) -> Result<PrefixSnapshot, String> {
    let root = Path::new(prefix_path);
    if !root.exists() {
        info!("[dep] Prefix {} does not exist, empty snapshot", prefix_path);
        return Ok(PrefixSnapshot::default());
    }

    let mut snapshot = PrefixSnapshot::default();
    collect_snapshot(root, root, &mut snapshot)?;
    info!("[dep] Snapshot: {} files in {}", snapshot.files.len(), prefix_path);
    Ok(snapshot)
}

fn collect_snapshot(
    root: &Path,
    current: &Path,
    snapshot: &mut PrefixSnapshot,
) -> Result<(), String> {
    let entries = fs::read_dir(current).map_err(|err| {
        format!(
            "Failed to read prefix directory '{}': {}",
            current.display(),
            err
        )
    })?;

    for entry in entries {
        let entry =
            entry.map_err(|err| format!("Failed to read prefix directory entry: {}", err))?;
        let path = entry.path();

        // Skip dosdevices to avoid redundant scanning and infinite loops (symlink cycles)
        if path.file_name().is_some_and(|n| n == "dosdevices") {
            continue;
        }

        // Skip temp/cache directories irrelevant to dependency state
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            let lower = n.to_ascii_lowercase();
            matches!(
                lower.as_str(),
                "temp" | "tmp" | "cache" | "installer" | "prefetch"
            ) || lower.starts_with("gac")
        }) {
            continue;
        }

        let metadata = entry
            .metadata()
            .map_err(|err| format!("Failed to read metadata for '{}': {}", path.display(), err))?;

        if metadata.is_dir() {
            collect_snapshot(root, &path, snapshot)?;
            continue;
        }

        if !metadata.is_file() {
            continue;
        }

        // Skip volatile bookkeeping that changes on every Wine run and is not
        // meaningful dependency state (registry hives are tracked via overrides).
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".log")
                || lower.ends_with(".tmp")
                || matches!(
                    lower.as_str(),
                    "system.reg" | "user.reg" | "userdef.reg" | ".update-timestamp"
                )
            {
                continue;
            }
        }

        if let Some(relative) = path_to_prefix_relative(root, &path) {
            snapshot.files.insert(
                relative,
                FileFingerprint {
                    len: metadata.len(),
                    modified_epoch_seconds: metadata
                        .modified()
                        .ok()
                        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                        .map(|value| value.as_secs())
                        .unwrap_or(0),
                },
            );
        }
    }

    Ok(())
}

fn diff_snapshots(before: &PrefixSnapshot, after: &PrefixSnapshot) -> StepChanges {
    let mut changes = StepChanges::default();

    for (path, fingerprint) in &after.files {
        match before.files.get(path) {
            None => changes.created_files.push(path.clone()),
            Some(previous) if previous != fingerprint => changes.touched_existing_files = true,
            _ => {}
        }
    }

    if before
        .files
        .keys()
        .any(|path| !after.files.contains_key(path))
    {
        changes.touched_existing_files = true;
    }

    changes.created_files.sort();
    changes.created_files.dedup();
    info!(
        "[dep] Diff: {} files created, touched_existing={}",
        changes.created_files.len(),
        changes.touched_existing_files
    );
    changes
}

fn winetricks_known_dll_overrides(verb: &str) -> Vec<String> {
    match verb {
        "d3dcompiler_42" | "d3dcompiler_43" | "d3dcompiler_46" | "d3dcompiler_47" => {
            vec![verb.to_string()]
        }
        "d3dx9" => (24..=43).map(|n| format!("d3dx9_{n}")).collect(),
        "d3dx11" => (42..=43).map(|n| format!("d3dx11_{n}")).collect(),
        "dx8vb" => vec!["dx8vb".to_string()],
        "amstream" => vec!["amstream".to_string()],
        "devenum" => vec!["devenum".to_string()],
        "dmband" | "dmcompos" | "dmime" | "dmloader" | "dmscript" | "dmstyle" | "dmsynth"
        | "dmusic" | "dmusic32" | "dsound" | "dswave" | "dsdmo" => vec![verb.to_string()],
        "qasf" | "qcap" | "qdvd" | "qedit" => {
            vec!["qasf".into(), "qcap".into(), "qdvd".into(), "qedit".into()]
        }
        "quartz" => vec!["quartz".to_string()],
        _ => vec![],
    }
}

fn path_to_prefix_relative(prefix_root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(prefix_root)
        .ok()
        .map(|relative| {
            relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .filter(|relative| !relative.is_empty())
}

fn build_cleanup_actions(installed: &InstalledDependency) -> Vec<(String, CleanupAction)> {
    let mut actions = Vec::new();

    if !installed.dll_overrides.is_empty() {
        actions.push((
            "Removing tracked DLL overrides…".to_string(),
            CleanupAction::RemoveDllOverrides(installed.dll_overrides.clone()),
        ));
    }

    if !installed.registered_dlls.is_empty() {
        actions.push((
            "Unregistering tracked DLLs…".to_string(),
            CleanupAction::UnregisterDlls(installed.registered_dlls.clone()),
        ));
    }

    if !installed.created_files.is_empty() {
        actions.push((
            format!("Removing {} tracked files…", installed.created_files.len()),
            CleanupAction::RemoveCreatedFiles(installed.created_files.clone()),
        ));
    }

    actions
}

fn remove_dll_overrides(
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
    dlls: &[String],
) -> Result<(), String> {
    fs::create_dir_all(cache_dir)
        .map_err(|err| format!("Failed to create dependency cache directory: {err}"))?;

    let reg_lines = dlls
        .iter()
        .map(|dll| format!("\"{}\"=-", dll))
        .collect::<Vec<_>>();
    let reg_content = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n\
         [HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\
         {}\r\n",
        reg_lines.join("\r\n")
    );

    let reg_path = Path::new(cache_dir).join("remove_dependency_overrides.reg");
    fs::write(&reg_path, reg_content)
        .map_err(|err| format!("Failed to write override removal file: {err}"))?;

    let mut cmd = std::process::Command::new(get_umu_run_path());
    configure_umu_command(&mut cmd, prefix_path, proton_path);
    cmd.args(["regedit.exe", "/S"]);
    cmd.arg(reg_path.as_os_str());

    let output = cmd
        .output()
        .map_err(|err| format!("Failed to run regedit: {err}"))?;
    let _ = fs::remove_file(&reg_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to remove DLL overrides: {}", stderr.trim()));
    }

    Ok(())
}

fn unregister_dlls(prefix_path: &str, proton_path: &str, dlls: &[String]) -> Result<(), String> {
    for dll in dlls {
        let mut cmd = std::process::Command::new(get_umu_run_path());
        configure_umu_command(&mut cmd, prefix_path, proton_path);
        cmd.args(["regsvr32.exe", "/u", "/s", dll]);

        let output = cmd
            .output()
            .map_err(|err| format!("Failed to run regsvr32 for '{}': {}", dll, err))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to unregister '{}': {}", dll, stderr.trim()));
        }
    }

    Ok(())
}

fn remove_created_files(prefix_path: &str, files: &[String]) -> Result<(), String> {
    let prefix_root = Path::new(prefix_path);
    let mut files = files.to_vec();
    files.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));

    for relative in &files {
        let path = prefix_root.join(relative);
        match fs::remove_file(&path) {
            Ok(()) => prune_empty_parent_dirs(prefix_root, &path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "Failed to remove tracked file '{}': {}",
                    path.display(),
                    err
                ));
            }
        }
    }

    Ok(())
}

fn prune_empty_parent_dirs(prefix_root: &Path, file_path: &Path) {
    let mut current = file_path.parent().map(PathBuf::from);
    while let Some(path) = current {
        if path == prefix_root {
            break;
        }

        match fs::remove_dir(&path) {
            Ok(()) => current = path.parent().map(PathBuf::from),
            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                current = path.parent().map(PathBuf::from)
            }
            Err(_) => break,
        }
    }
}
