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
    UMU_DOWNLOADING, WINETRICKS_DOWNLOAD_STARTED, WINETRICKS_DOWNLOADING, claim_download,
    download_winetricks, get_umu_run_path, get_winetricks_path, is_umu_run_available,
    is_winetricks_available,
};

use super::recipes::get_dep_steps;
use super::state::{read_prefix_dep_state_checked, remove_installed_dep, upsert_installed_dep};
use leyen_model::deps::{
    DepProfile, InstalledDependency, find_installed_dependents, get_dep_profile,
    get_deps_cache_dir,
};

const COMMAND_TIMEOUT_SECS: u64 = 600;
/// How long to wait for a killed command's pipes to close before giving up.
const POST_KILL_WAIT: Duration = Duration::from_secs(5);

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
/// `None` if one is already in progress. Keyed by the canonical path so
/// `/x/pfx`, `/x/pfx/` and a symlink to it share one lock.
async fn try_lock_prefix_op(prefix: &str) -> Option<PrefixOpGuard> {
    let raw = prefix.to_string();
    let key = tokio::task::spawn_blocking(move || {
        fs::canonicalize(&raw)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(raw)
    })
    .await
    .unwrap_or_else(|_| prefix.to_string());
    let mut set = busy_prefixes().lock().ok()?;
    if !set.insert(key.clone()) {
        return None;
    }
    Some(PrefixOpGuard(key))
}

/// Files currently being downloaded/verified in the shared dependency cache.
/// `get_deps_cache_dir()` is one global directory across all prefixes, so two
/// installs/uninstalls for *different* prefixes (which `busy_prefixes` does not
/// serialize against each other) can target the same cached file; this stops
/// one from treating a sibling's half-written download as complete, or
/// deleting a file the other still needs mid-verification.
fn busy_cache_files() -> &'static Mutex<HashSet<String>> {
    static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    BUSY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Releases the cache file claim on drop.
struct CacheFileGuard(String);

impl Drop for CacheFileGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = busy_cache_files().lock() {
            set.remove(&self.0);
        }
    }
}

/// Attempts to claim exclusive access to `file_name` in the shared dependency
/// cache, or returns `None` if another task already holds it.
fn try_claim_cache_file(file_name: &str) -> Option<CacheFileGuard> {
    let mut set = busy_cache_files().lock().ok()?;
    if !set.insert(file_name.to_string()) {
        return None;
    }
    Some(CacheFileGuard(file_name.to_string()))
}

/// Claims `file_name`, waiting (honoring `cancel`) while another task holds it.
async fn claim_cache_file(file_name: &str, cancel: &Arc<AtomicBool>) -> Result<CacheFileGuard, String> {
    loop {
        if let Some(guard) = try_claim_cache_file(file_name) {
            return Ok(guard);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(t!("Cancelled."));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
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
pub(super) async fn run_umu_command(
    mut cmd: AsyncCommand,
    label: String,
    cancel: Arc<AtomicBool>,
) -> Result<std::process::Output, String> {
    cmd.stdin(Stdio::null());
    // Nothing reads stdout; piping it only buffered installer chatter in memory.
    cmd.stdout(Stdio::null());
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
                // After the kill, the wait ends only when every holder of the
                // stderr pipe exits; a descendant that left the process group
                // (wineserver calls setsid) could keep it open, so bound it.
                if cancel.load(Ordering::Relaxed) {
                    kill_group();
                    let _ = tokio::time::timeout(POST_KILL_WAIT, &mut wait_fut).await;
                    return Err(t!("Cancelled."));
                }
                if start.elapsed() >= timeout {
                    warn!("[dep] '{}' timed out after {} seconds", label, COMMAND_TIMEOUT_SECS);
                    kill_group();
                    let _ = tokio::time::timeout(POST_KILL_WAIT, &mut wait_fut).await;
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

            // Claimed for the whole exists-check + download + verify sequence
            // below: a sibling operation for another prefix may share this
            // exact cache file.
            let _cache_claim = claim_cache_file(file_name, cancel).await?;

            let dest = Path::new(cache_dir).join(file_name);
            let expected_sha = sha256.to_string();

            // A cached copy is trusted only if its checksum still matches;
            // a partial file from an interrupted download (or a stale pin)
            // is deleted and fetched again instead of failing the install.
            let cached_ok = {
                let d = dest.clone();
                let expected = expected_sha.clone();
                tokio::task::spawn_blocking(move || {
                    if !d.exists() {
                        return Ok(false);
                    }
                    match verify_sha256(&d, &expected) {
                        Ok(true) => Ok(true),
                        Ok(false) => {
                            warn!(
                                "[dep] cached '{}' has a stale checksum; re-downloading",
                                d.display()
                            );
                            fs::remove_file(&d)
                                .map_err(|e| format!("Failed to remove stale cached file: {e}"))?;
                            Ok(false)
                        }
                        Err(e) => Err(e),
                    }
                })
                .await
                .map_err(join_err)
                .and_then(|r| r)?
            };

            if cached_ok {
                info!("[dep] {} already cached, skipping download", file_name);
                return Ok(StepChanges::default());
            }

            // Check cancel before starting download
            if cancel.load(Ordering::Relaxed) {
                return Err(t!("Cancelled."));
            }

            info!("[dep] Downloading {} from {}", file_name, url);
            let cache_dir_clone = cache_dir.to_string();
            tokio::task::spawn_blocking(move || fs::create_dir_all(cache_dir_clone))
                .await
                .map_err(join_err)
                .and_then(|r| r.map_err(|err| format!("Failed to create dependency cache directory: {err}")))?;

            // Download to a unique temp name and rename into place only after
            // the checksum matches, so `dest` is either absent or complete.
            let temp = Path::new(cache_dir).join(format!(
                "{}.tmp.{}.{}",
                file_name,
                std::process::id(),
                uuid::Uuid::new_v4()
            ));

            let mut cmd = AsyncCommand::new("curl");
            cmd.args([
                "--proto",
                "=https",
                "--proto-redir",
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
                temp.to_string_lossy().as_ref(),
                url,
            ]);

            let output = {
                let child = cmd
                    .spawn()
                    .map_err(|err| format!("Failed to spawn curl: {err}"))?;
                let pid = child.id();
                let wait_fut = child.wait_with_output();
                tokio::pin!(wait_fut);

                loop {
                    tokio::select! {
                        out = &mut wait_fut => {
                            break out.map_err(|e| format!("Failed to run curl: {e}"))?;
                        }
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            if cancel.load(Ordering::Relaxed) {
                                if let Some(pid) = pid {
                                    unsafe {
                                        libc::kill(pid as i32, libc::SIGKILL);
                                    }
                                }
                                let _ = (&mut wait_fut).await;
                                let _ = tokio::fs::remove_file(&temp).await;
                                return Err(t!("Cancelled."));
                            }
                        }
                    }
                }
            };

            if !output.status.success() {
                let _ = tokio::fs::remove_file(&temp).await;
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Download failed for {}: {}", file_name, stderr.trim()));
            }
            info!("[dep] Downloaded {}", file_name);

            info!("[dep] Verifying SHA256 for {}", file_name);
            let file_name = *file_name;
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                match verify_sha256(&temp, &expected_sha) {
                    Ok(true) => fs::rename(&temp, &dest).map_err(|e| {
                        let _ = fs::remove_file(&temp);
                        format!("Failed to move downloaded file into the cache: {e}")
                    }),
                    Ok(false) => {
                        let _ = fs::remove_file(&temp);
                        Err(format!(
                            "Checksum mismatch for {}: expected {}; download rejected",
                            file_name, expected_sha
                        ))
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&temp);
                        Err(e)
                    }
                }
            })
            .await
            .map_err(join_err)
            .and_then(|r| r)?;
            info!("[dep] SHA256 verified for {}", file_name);

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
            write_dll_overrides(
                prefix_path,
                proton_path,
                cache_dir,
                &overrides,
                Some("native,builtin"),
                cancel.clone(),
            )
            .await?;
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
                // `--force`: winetricks skips verbs listed in winetricks.log;
                // after an uninstall removed the files that log is stale.
                cmd.args([get_winetricks_path().as_str(), "-q", "--force", verb.as_str()]);
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
                    super::verify::check_registry_key_exists(
                        prefix_path,
                        proton_path,
                        path,
                        cancel.clone(),
                    )
                    .await?
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

/// Streams `path` through SHA-256 and compares with `expected` (hex).
fn verify_sha256(path: &Path, expected: &str) -> Result<bool, String> {
    let mut file = fs::File::open(path)
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
    Ok(hex::encode(hasher.finalize()).eq_ignore_ascii_case(expected))
}

/// Drops `verb` from `$WINEPREFIX/winetricks.log`. winetricks consults that
/// file before installing and skips a verb it has already logged, so a verb
/// whose files Leyen removed must be unlogged or a reinstall becomes a no-op.
fn forget_winetricks_verb(prefix_path: &str, verb: &str) -> Result<(), String> {
    let path = Path::new(prefix_path).join("winetricks.log");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("Failed to read winetricks.log: {err}")),
    };
    let kept: Vec<&str> = text.lines().filter(|line| line.trim() != verb).collect();
    if kept.len() == text.lines().count() {
        return Ok(());
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    leyen_model::paths::atomic_write(&path, &out)
        .map_err(|err| format!("Failed to update winetricks.log: {err}"))
}

pub(super) fn configure_umu_command_async(cmd: &mut AsyncCommand, prefix_path: &str, proton_path: &str) {
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

    let Some(_prefix_guard) = try_lock_prefix_op(&prefix_path).await else {
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

    ensure_umu_ready(needs_winetricks, &on_progress, &cancel).await?;

    let prefix_path_for_state = prefix_path.clone();
    let state = match tokio::task::spawn_blocking(move || {
        read_prefix_dep_state_checked(&prefix_path_for_state)
    })
    .await
    .map_err(join_err)
    .and_then(|r| r)
    {
        Ok(state) => state,
        Err(err) => {
            return Err(t!("Dependency state file is corrupt: {}").replacen("{}", &err, 1));
        }
    };

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

            // Downloads run in parallel under a shared stop flag: the user's
            // cancel is mirrored into it, and the first failure sets it, so a
            // checksum mismatch on one file does not wait out the others'
            // transfers. Each download cleans its own temp file on stop.
            let stop = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            {
                let stop = stop.clone();
                let done = done.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    while !done.load(Ordering::Relaxed) {
                        if cancel.load(Ordering::Relaxed) {
                            stop.store(true, Ordering::Relaxed);
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                });
            }
            let (prefix_ref, proton_ref, cache_ref) =
                (prefix_path.as_str(), proton_path.as_str(), cache_dir.as_str());
            let futures: Vec<_> = download_steps
                .iter()
                .map(|step| {
                    let stop = stop.clone();
                    async move {
                        let result =
                            execute_dep_step(step, prefix_ref, proton_ref, cache_ref, &stop).await;
                        if result.is_err() {
                            stop.store(true, Ordering::Relaxed);
                        }
                        result
                    }
                })
                .collect();

            let results = join_all(futures).await;
            done.store(true, Ordering::Relaxed);
            let mut first_error: Option<String> = None;
            for result in results {
                match result {
                    Ok(changes) => recorded.merge(changes),
                    // Siblings aborted by the flag report "Cancelled."; keep
                    // the error that actually caused the abort.
                    Err(error) => {
                        error!("[dep:{}] download failed: {}", profile.id, error);
                        if first_error.is_none() || (error != t!("Cancelled.") && first_error.as_deref() == Some(t!("Cancelled.").as_str())) {
                            first_error = Some(error);
                        }
                    }
                }
            }
            if let Some(error) = first_error {
                if cancel.load(Ordering::Relaxed) {
                    return Err(t!("Cancelled."));
                }
                return Err(error);
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

        // Diff once after the steps so we know what this attempt created. A
        // failed snapshot would record the dependency with no file list, so
        // nothing could ever be rolled back or uninstalled: treat it as a
        // failed step instead.
        {
            let p = prefix_path.clone();
            match tokio::task::spawn_blocking(move || snapshot_prefix(&p))
                .await
                .map_err(join_err)
                .and_then(|r| r)
            {
                Ok(after) => recorded.merge(diff_snapshots(&snapshot_before, &after)),
                Err(err) => {
                    error!("[dep:{}] post-install snapshot failed: {}", profile.id, err);
                    step_error.get_or_insert(err);
                }
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
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(usize, usize, String) + Send + 'static,
) -> Result<Option<String>, String> {
    let dep_id = dep_id.to_string();
    let prefix_path = prefix_path.to_string();
    let proton_path = proton_path.to_string();
    let cache_dir = get_deps_cache_dir();

    let Some(_prefix_guard) = try_lock_prefix_op(&prefix_path).await else {
        return Err(t!(
            "Another dependency operation is already running for this prefix."
        ));
    };
    let prefix_path_for_state = prefix_path.clone();
    let state = match tokio::task::spawn_blocking(move || {
        read_prefix_dep_state_checked(&prefix_path_for_state)
    })
    .await
    .map_err(join_err)
    .and_then(|r| r)
    {
        Ok(state) => state,
        Err(err) => {
            return Err(t!("Dependency state file is corrupt: {}").replacen("{}", &err, 1));
        }
    };

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
    if requires_umu {
        ensure_umu_ready(false, &on_progress, &cancel).await?;
    }

    // winetricks has no uninstall command; forgetting the verb in its log is
    // what lets a later install run again instead of "already installed".
    for verb in &winetricks_verbs {
        on_progress(0, 0, format!("Uninstalling winetricks '{}'…", verb));
        let prefix_path = prefix_path.clone();
        let verb = verb.clone();
        let result = tokio::task::spawn_blocking(move || forget_winetricks_verb(&prefix_path, &verb))
            .await
            .map_err(join_err)
            .and_then(|r| r);
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
        if cancel.load(Ordering::Relaxed) {
            return Err(t!("Cancelled."));
        }
        on_progress(index + 1, total_actions, description.clone());

        let prefix_path = prefix_path.clone();
        let proton_path = proton_path.clone();
        let cache_dir = cache_dir.clone();

        let result = match action {
            CleanupAction::RemoveDllOverrides(dlls) => {
                remove_dll_overrides(&prefix_path, &proton_path, &cache_dir, &dlls, cancel.clone())
                    .await
            }
            CleanupAction::UnregisterDlls(dlls) => {
                unregister_dlls(&prefix_path, &proton_path, &dlls, cancel.clone()).await
            }
            CleanupAction::RemoveCreatedFiles(files) => {
                tokio::task::spawn_blocking(move || remove_created_files(&prefix_path, &files))
                    .await
                    .map_err(join_err)
                    .and_then(|r| r)
            }
        };
        result.inspect_err(|error| error!("[dep:{}] removal failed: {}", dep_id, error))?;
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
    cancel: &AtomicBool,
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

            if claim_download(&WINETRICKS_DOWNLOAD_STARTED, &WINETRICKS_DOWNLOADING) {
                info!("[dep] Starting winetricks download…");
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
                if cancel.load(Ordering::Relaxed) {
                    return Err(t!("Cancelled."));
                }
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

        // Wine may still be tearing down temp files while we scan: an entry
        // that vanished between `read_dir` and `stat` is simply not there.
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(format!(
                    "Failed to read metadata for '{}': {}",
                    path.display(),
                    err
                ));
            }
        };

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
        "d3dx11" | "d3dx11_42" | "d3dx11_43" => (42..=43).map(|n| format!("d3dx11_{n}")).collect(),
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

async fn remove_dll_overrides(
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
    dlls: &[String],
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    write_dll_overrides(prefix_path, proton_path, cache_dir, dlls, None, cancel).await
}

/// Sets (`Some(value)`) or deletes (`None`) `HKCU\Software\Wine\DllOverrides`
/// entries for `dlls` through `regedit /S`.
async fn write_dll_overrides(
    prefix_path: &str,
    proton_path: &str,
    cache_dir: &str,
    dlls: &[String],
    value: Option<&str>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    if dlls.is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|err| format!("Failed to create dependency cache directory: {err}"))?;

    let reg_lines = dlls
        .iter()
        .map(|dll| match value {
            Some(value) => format!("\"{}\"=\"{}\"", dll, value),
            None => format!("\"{}\"=-", dll),
        })
        .collect::<Vec<_>>();
    let reg_content = format!(
        "Windows Registry Editor Version 5.00\r\n\r\n\
         [HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\
         {}\r\n",
        reg_lines.join("\r\n")
    );

    // Unique per invocation: cache_dir is one global directory shared by every
    // prefix, so a fixed name here would let concurrent uninstalls for
    // different prefixes overwrite each other's file before regedit reads it.
    let reg_path = Path::new(cache_dir).join(format!(
        "dependency_overrides.{}.{}.reg",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    tokio::fs::write(&reg_path, reg_content)
        .await
        .map_err(|err| format!("Failed to write DLL override file: {err}"))?;

    let mut cmd = AsyncCommand::new(get_umu_run_path());
    configure_umu_command_async(&mut cmd, prefix_path, proton_path);
    cmd.args(["regedit.exe", "/S"]);
    cmd.arg(reg_path.as_os_str());

    let what = if value.is_some() { "apply" } else { "remove" };
    let result = run_umu_command(cmd, "regedit /S".to_string(), cancel).await;
    let _ = tokio::fs::remove_file(&reg_path).await;
    let output = result.map_err(|e| format!("Failed to {what} DLL overrides: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Failed to {what} DLL overrides: regedit exited with status {}",
            output.status
        ));
    }
    Ok(())
}

async fn unregister_dlls(
    prefix_path: &str,
    proton_path: &str,
    dlls: &[String],
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    for dll in dlls {
        let mut cmd = AsyncCommand::new(get_umu_run_path());
        configure_umu_command_async(&mut cmd, prefix_path, proton_path);
        cmd.args(["regsvr32.exe", "/u", "/s", dll]);

        let output = run_umu_command(cmd, format!("regsvr32 /u {}", dll), cancel.clone())
            .await
            .map_err(|e| format!("Failed to unregister '{}': {}", dll, e))?;
        if !output.status.success() {
            return Err(format!(
                "Failed to unregister '{}': regsvr32 exited with status {}",
                dll, output.status
            ));
        }
    }

    Ok(())
}

fn remove_created_files(prefix_path: &str, files: &[String]) -> Result<(), String> {
    let prefix_root = Path::new(prefix_path);
    let mut files = files.to_vec();
    files.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));

    for relative in &files {
        // Tracked paths come from a state file inside the prefix; anything but
        // plain relative components could reach outside it.
        if !Path::new(relative)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "Refusing to remove tracked file '{}': path escapes the prefix",
                relative
            ));
        }
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
