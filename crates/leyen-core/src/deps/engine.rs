use leyen_model::i18n::gettext;

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
use crate::sandbox::{Confined, SandboxRequest, Share, confine_in_scope, is_available};

use super::recipes::get_dep_steps;
use super::state::{remove_installed_dep, upsert_installed_dep};
use leyen_model::deps::{
    DepProfile, InstalledDependency, find_installed_dependents, get_dep_profile,
    get_deps_cache_dir, read_prefix_dep_state_checked,
};

const COMMAND_TIMEOUT_SECS: u64 = 600;
/// How many lines of a failed command's error output go into its message.
const STDERR_TAIL_LINES: usize = 10;
/// How long a stopped command gets to end after SIGTERM, and again after SIGKILL.
const POST_KILL_WAIT: Duration = Duration::from_secs(5);

/// Prefixes with a dependency install/uninstall in progress. Prevents two
/// concurrent operations on the same prefix (e.g. the deps page opened from both
/// Preferences and a game's edit dialog) from interleaving their prefix
/// snapshots and corrupting the tracked file list.
fn busy_prefixes() -> &'static Mutex<HashSet<String>> {
    static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    BUSY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether a dependency operation runs on any prefix.
pub fn any_in_progress() -> bool {
    busy_prefixes().lock().map_or(true, |set| !set.is_empty())
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
async fn claim_cache_file(
    file_name: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<CacheFileGuard, String> {
    loop {
        if let Some(guard) = try_claim_cache_file(file_name) {
            return Ok(guard);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(gettext("Cancelled."));
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

/// Runs a umu command to completion with a timeout, sandboxed in a transient
/// systemd user scope like a game. Cancel and the timeout stop the whole scope —
/// every process the command started, including those that left its process group
/// (wineserver calls setsid) — with SIGTERM first and SIGKILL after a grace.
pub(super) async fn run_umu_command(
    cmd: AsyncCommand,
    label: String,
    cancel: Arc<AtomicBool>,
) -> Result<std::process::Output, String> {
    if !tokio::task::spawn_blocking(crate::launch::systemd_user_available)
        .await
        .unwrap_or(false)
    {
        return Err(gettext(
            "A systemd user session is required to install or remove dependencies.",
        ));
    }
    // An installer is a Windows program from the internet: no sandbox, no run.
    tokio::task::spawn_blocking(is_available)
        .await
        .unwrap_or_else(|e| Err(e.to_string()))?;

    let settings = crate::config::load_settings().await;
    let unit = format!("leyen-dep-{}.scope", uuid::Uuid::new_v4());
    let shares =
        tokio::task::spawn_blocking(move || dependency_shares(&settings.global_sandbox_folders))
            .await
            .map_err(join_err)?;
    let sandbox = dependency_sandbox(&cmd, shares);
    let Confined {
        command: mut scoped,
        lease,
    } = confine_in_scope(&cmd, &unit, &sandbox).await?;
    scoped.stdin(Stdio::null());
    // Nothing reads stdout; piping it only buffered installer chatter in memory.
    scoped.stdout(Stdio::null());
    scoped.stderr(Stdio::piped());

    // Run on the Tokio runtime (not the GTK/glib executor) so the process and
    // timer drivers advance while the main loop stays responsive for Cancel.
    tokio::spawn(async move {
        let output = match scoped.spawn() {
            Ok(child) => {
                lease.spawned(child.id());
                run_umu_command_inner(child, unit, label, cancel).await
            }
            Err(e) => {
                drop(lease);
                Err(format!("Failed to launch {}: {}", label, e))
            }
        };
        crate::sandbox::release_namespace(&sandbox.prefix_path).await;
        output
    })
    .await
    .map_err(|e| format!("Command task panicked: {e}"))
    .and_then(|r| r)
}

/// Stops what a dependency job left running when the daemon running it died:
/// nobody follows it any more or records what it installed, and it would change
/// its prefix under whatever starts there next. Run once at startup.
pub fn stop_orphaned_jobs() {
    for unit in crate::launch::active_units("leyen-dep-*").unwrap_or_default() {
        warn!("[dep] Stopping '{unit}', left running by an earlier daemon");
        crate::launch::stop_scope_verified(&unit);
    }
}

/// An install whose steps are running, kept on disk until it is recorded or
/// rolled back, so that a daemon that dies meanwhile is followed by one that
/// can take its files back out of the prefix.
#[derive(serde::Serialize, serde::Deserialize)]
struct PendingInstall {
    prefix: String,
    dep_id: String,
    files_before: BTreeMap<String, FileFingerprint>,
}

fn pending_installs_dir() -> PathBuf {
    leyen_model::paths::get_config_dir().join("pending-deps")
}

/// Writes down an install about to run its steps, and returns the file to
/// delete once it is recorded or rolled back. A daemon stopped in between,
/// even cleanly, leaves the file to the next one.
fn record_pending(prefix: &str, dep_id: &str, before: &PrefixSnapshot) -> Result<PathBuf, String> {
    let pending = PendingInstall {
        prefix: prefix.to_string(),
        dep_id: dep_id.to_string(),
        files_before: before.files.clone(),
    };
    let dir = pending_installs_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create '{}': {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    let json = serde_json::to_string(&pending)
        .map_err(|e| format!("Failed to serialize the pending install: {e}"))?;
    leyen_model::paths::atomic_write(&path, &json)
        .map_err(|e| format!("Failed to write '{}': {e}", path.display()))?;
    Ok(path)
}

/// Takes the files of installs an earlier daemon left unfinished back out of
/// their prefixes, as a failed install's are. Run once at startup, after
/// [`stop_orphaned_jobs`] has stopped what was still running.
pub fn roll_back_interrupted_installs() {
    let Ok(entries) = fs::read_dir(pending_installs_dir()) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let pending = fs::read_to_string(&path)
            .ok()
            .and_then(|json| serde_json::from_str::<PendingInstall>(&json).ok());
        if let Some(pending) = pending {
            let before = PrefixSnapshot {
                files: pending.files_before,
                ..PrefixSnapshot::default()
            };
            match snapshot_prefix(&pending.prefix) {
                Ok(after) => {
                    let created = diff_snapshots(&before, &after).created_files;
                    warn!(
                        "[dep:{}] rolling back {} files of an install an earlier daemon left unfinished in '{}'",
                        pending.dep_id,
                        created.len(),
                        pending.prefix
                    );
                    if let Err(e) = remove_created_files(&pending.prefix, &created) {
                        warn!("[dep:{}] rollback failed: {e}", pending.dep_id);
                    }
                }
                Err(e) => warn!("[dep:{}] rollback failed: {e}", pending.dep_id),
            }
        }
        let _ = fs::remove_file(&path);
    }
}

/// The sandbox a dependency install runs in. The prefix and the Proton are on
/// the command as environment variables already, so they are read back from
/// there rather than threaded through every caller.
fn dependency_sandbox(cmd: &AsyncCommand, shares: Vec<Share>) -> SandboxRequest {
    let value = |name: &str| {
        cmd.as_std()
            .get_envs()
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    SandboxRequest {
        prefix_path: value("WINEPREFIX"),
        proton_path: value("PROTONPATH"),
        work_dir: None,
        shares,
        // Installers fetch what they install.
        network: true,
    }
}

/// What a dependency install is given besides its prefix and its Proton.
fn dependency_shares(shared_folders: &[leyen_model::models::SandboxFolder]) -> Vec<Share> {
    let home = std::env::var("HOME").unwrap_or_default();
    // What the preferences share with everything Leyen runs. A refused folder is
    // logged by the launch that owns it; here it is skipped.
    let (mut shares, _) = crate::sandbox::shares_from(shared_folders);
    // winetricks keeps what it downloads in a cache it shares between prefixes,
    // and re-downloading every component per prefix is no gain.
    let winetricks_cache = PathBuf::from(home).join(".cache/winetricks");
    let _ = std::fs::create_dir_all(&winetricks_cache);
    shares.push(Share::writable(winetricks_cache));
    // The installers Leyen downloaded and verified: readable, so a component
    // being installed cannot tamper with the one installed next.
    shares.push(Share::read_only(get_deps_cache_dir()));
    shares
}

async fn run_umu_command_inner(
    child: tokio::process::Child,
    unit: String,
    label: String,
    cancel: Arc<AtomicBool>,
) -> Result<std::process::Output, String> {
    // Until systemd-run has registered the scope, the command is only reachable
    // as this child.
    let pidfd = child
        .id()
        .and_then(|pid| crate::launch::pidfd_open(pid).ok());
    let wait_fut = child.wait_with_output();
    tokio::pin!(wait_fut);

    let start = tokio::time::Instant::now();
    let timeout = Duration::from_secs(COMMAND_TIMEOUT_SECS);

    loop {
        tokio::select! {
            out = &mut wait_fut => {
                return out.map_err(|e| format!("Failed to run {}: {}", label, e));
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                let error = if cancel.load(Ordering::Relaxed) {
                    gettext("Cancelled.")
                } else if start.elapsed() >= timeout {
                    warn!("[dep] '{}' timed out after {} seconds", label, COMMAND_TIMEOUT_SECS);
                    format!("'{}' timed out after {} seconds", label, COMMAND_TIMEOUT_SECS)
                } else {
                    continue;
                };
                // The wait ends once every holder of the stderr pipe is gone,
                // which is every process in the scope.
                signal_scope(&unit, pidfd.as_ref(), libc::SIGTERM).await;
                if tokio::time::timeout(POST_KILL_WAIT, &mut wait_fut).await.is_err() {
                    signal_scope(&unit, pidfd.as_ref(), libc::SIGKILL).await;
                    let _ = tokio::time::timeout(POST_KILL_WAIT, &mut wait_fut).await;
                }
                return Err(error);
            }
        }
    }
}

/// Sends `signal` to every process of the scope `unit`, and to the command
/// itself in case the scope is not there yet.
async fn signal_scope(unit: &str, command: Option<&std::os::fd::OwnedFd>, signal: libc::c_int) {
    if let Some(pidfd) = command {
        crate::launch::pidfd_signal(pidfd, signal);
    }
    let name = if signal == libc::SIGKILL {
        "SIGKILL"
    } else {
        "SIGTERM"
    };
    let unit = unit.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        crate::launch::systemctl(&["kill", &format!("--signal={name}"), &unit])
    })
    .await;
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
    pub winetricks_verbs: Vec<String>,
    pub uninstall_entries: Vec<String>,
}

impl StepChanges {
    fn merge(&mut self, other: StepChanges) {
        self.created_files = merge_unique_strings(&self.created_files, &other.created_files);
        self.touched_existing_files |= other.touched_existing_files;
        self.dll_overrides = merge_unique_strings(&self.dll_overrides, &other.dll_overrides);
        self.registered_dlls = merge_unique_strings(&self.registered_dlls, &other.registered_dlls);
        self.winetricks_verbs =
            merge_unique_strings(&self.winetricks_verbs, &other.winetricks_verbs);
        self.uninstall_entries =
            merge_unique_strings(&self.uninstall_entries, &other.uninstall_entries);
    }

    fn into_dependency_record(self) -> InstalledDependency {
        InstalledDependency {
            created_files: self.created_files,
            touched_existing_files: self.touched_existing_files,
            dll_overrides: self.dll_overrides,
            registered_dlls: self.registered_dlls,
            winetricks_verbs: self.winetricks_verbs,
            uninstall_entries: self.uninstall_entries,
            ..InstalledDependency::default()
        }
    }
}

#[derive(Default)]
struct PrefixSnapshot {
    files: BTreeMap<String, FileFingerprint>,
    /// Files Proton put in the prefix and keeps up to date itself.
    proton_files: HashSet<String>,
    /// What the registry and winetricks keep outside the files above.
    dll_overrides: BTreeSet<String>,
    winetricks_verbs: BTreeSet<String>,
    uninstall_entries: BTreeSet<String>,
}

/// A program registered for removal under `HKLM\...\Uninstall`.
struct UninstallEntry {
    name: String,
    /// The command that removes it without asking, as Windows runs it.
    quiet_command: Option<String>,
    /// Whether Windows Installer installed it, so `msiexec /x` removes it.
    msi: bool,
}

/// File signature: (len, mtime). May miss same-size+same-mtime in-place edits.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FileFingerprint {
    len: u64,
    modified_epoch_seconds: u64,
}

enum CleanupAction {
    RunUninstallers(Vec<String>),
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
        Ok(_) => info!(
            "[dep] Tokio runtime available for step '{}'",
            step.description
        ),
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
                return Err(gettext("Cancelled."));
            }

            info!("[dep] Downloading {} from {}", file_name, url);
            let cache_dir_clone = cache_dir.to_string();
            tokio::task::spawn_blocking(move || fs::create_dir_all(cache_dir_clone))
                .await
                .map_err(join_err)
                .and_then(|r| {
                    r.map_err(|err| format!("Failed to create dependency cache directory: {err}"))
                })?;

            // Download to a unique temp name and rename into place only after
            // the checksum matches, so `dest` is either absent or complete.
            let temp = Path::new(cache_dir).join(format!(
                "{}.tmp.{}.{}",
                file_name,
                std::process::id(),
                uuid::Uuid::new_v4()
            ));

            let mut cmd = AsyncCommand::new("curl");
            cmd.stdout(Stdio::null()).stderr(Stdio::piped());
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
                                return Err(gettext("Cancelled."));
                            }
                        }
                    }
                }
            };

            if !output.status.success() {
                let _ = tokio::fs::remove_file(&temp).await;
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "Download failed for {}: {}",
                    file_name,
                    stderr.trim()
                ));
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
            info!(
                "[dep] Running {} {} (timeout: {}s)",
                file_name, args, COMMAND_TIMEOUT_SECS
            );
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
                return Err(format!(
                    "Installer '{}' failed (code {}): {}",
                    file_name,
                    exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    stderr_tail(&output.stderr)
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
            info!(
                "[dep] Running msiexec /i {} {} (timeout: {}s)",
                file_name, args, COMMAND_TIMEOUT_SECS
            );
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
            info!(
                "[dep] msiexec {} completed (exit code: {:?})",
                file_name, exit_code
            );
            if !output.status.success() && exit_code != Some(3010) {
                return Err(format!(
                    "MSI install '{}' failed (code {:?}): {}",
                    file_name,
                    exit_code,
                    stderr_tail(&output.stderr)
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
            info!(
                "[dep] Running winetricks {} (timeout: {}s)",
                verb, COMMAND_TIMEOUT_SECS
            );
            // umu refuses to run a verb that winetricks.log lists, before
            // winetricks and its `--force` see it, so a reinstall or a verb that
            // came in with another one would fail.
            {
                let prefix = prefix_path.to_string();
                let verb = verb.clone();
                tokio::task::spawn_blocking(move || forget_winetricks_verb(&prefix, &verb))
                    .await
                    .map_err(join_err)
                    .and_then(|r| r)?;
            }
            let output = {
                let mut cmd = AsyncCommand::new(get_umu_run_path());
                configure_umu_command_async(&mut cmd, prefix_path, proton_path);
                // `--force`: winetricks skips a verb whose files it finds.
                cmd.args([
                    get_winetricks_path().as_str(),
                    "-q",
                    "--force",
                    verb.as_str(),
                ]);
                run_umu_command(cmd, format!("winetricks {}", verb), cancel.clone()).await?
            };
            // umu-run exits 0 when it fails before running winetricks; winetricks
            // logs a verb once it has installed it.
            let logged = {
                let prefix = prefix_path.to_string();
                let verb = verb.clone();
                tokio::task::spawn_blocking(move || winetricks_logged(&prefix, &verb))
                    .await
                    .map_err(join_err)?
            };
            if !output.status.success() || !logged {
                return Err(format!(
                    "winetricks '{}' failed: {}",
                    verb,
                    stderr_tail(&output.stderr)
                ));
            }
            info!("[dep] winetricks {} completed", verb);
            Ok(StepChanges::default())
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
    let mut file =
        fs::File::open(path).map_err(|err| format!("Failed to open downloaded file: {err}"))?;
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

/// The last lines of a command's error output, enough to say why it failed
/// without carrying an installer's whole log into the message.
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.trim().lines().collect();
    lines[lines.len().saturating_sub(STDERR_TAIL_LINES)..].join("\n")
}

/// The DLL overrides set in the prefix's registry, by name as Wine stores them.
fn dll_overrides(prefix_path: &str) -> BTreeSet<String> {
    let text = fs::read_to_string(Path::new(prefix_path).join("user.reg")).unwrap_or_default();
    let mut in_section = false;
    let mut names = BTreeSet::new();
    for line in text.lines() {
        if line.starts_with('[') {
            in_section = line.starts_with("[Software\\\\Wine\\\\DllOverrides]");
        } else if in_section
            && let Some((name, _)) = line
                .strip_prefix('"')
                .and_then(|rest| rest.split_once("\"="))
        {
            names.insert(name.to_string());
        }
    }
    names
}

/// The verbs `$WINEPREFIX/winetricks.log` lists.
fn winetricks_verbs(prefix_path: &str) -> BTreeSet<String> {
    fs::read_to_string(Path::new(prefix_path).join("winetricks.log"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Drops `verb` from `$WINEPREFIX/winetricks.log`. umu consults that file
/// before running winetricks and refuses a verb it lists.
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

/// Whether `$WINEPREFIX/winetricks.log` lists `verb`.
fn winetricks_logged(prefix_path: &str, verb: &str) -> bool {
    fs::read_to_string(Path::new(prefix_path).join("winetricks.log"))
        .is_ok_and(|text| text.lines().any(|line| line.trim() == verb))
}

/// Refuses a Proton umu-run cannot use: it exits 0 then, having run nothing.
/// An empty path is umu-launcher's own Proton.
async fn check_proton(proton_path: &str) -> Result<(), String> {
    if proton_path.is_empty() {
        return Ok(());
    }
    let dir = PathBuf::from(proton_path);
    let usable = tokio::task::spawn_blocking(move || {
        dir.join("proton").is_file() && dir.join("toolmanifest.vdf").is_file()
    })
    .await
    .map_err(join_err)?;
    if usable {
        Ok(())
    } else {
        warn!("[dep] Proton '{proton_path}' has no proton script or toolmanifest.vdf");
        Err(gettext("Selected Proton version was not found"))
    }
}

pub(super) fn configure_umu_command_async(
    cmd: &mut AsyncCommand,
    prefix_path: &str,
    proton_path: &str,
) {
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
        return Err(gettext(
            "Another dependency operation is already running for this prefix.",
        ));
    };

    // Preflight: the dependency cache must be writable for downloads.
    let cache_check = cache_dir.clone();
    tokio::task::spawn_blocking(move || fs::create_dir_all(&cache_check))
        .await
        .map_err(join_err)
        .and_then(|r| {
            r.map_err(|e| {
                gettext("Cannot write to the dependency cache directory: {}").replacen(
                    "{}",
                    &e.to_string(),
                    1,
                )
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
            return Err(gettext("Dependency state file is corrupt: {}").replacen("{}", &err, 1));
        }
    };

    let install_plan = match build_install_plan(&dep_id, &state) {
        Ok(plan) if !plan.is_empty() => plan,
        Ok(_) => return Ok(Some(gettext("Dependency is already installed."))),
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

    // Proton creates a missing prefix, and upgrades one another Proton made, on
    // the first command it runs. Done inside a component's snapshot, its whole
    // Wine install would be recorded as that component's files, and removed
    // with it.
    check_proton(&proton_path).await?;
    on_progress(0, total_steps, "Preparing the prefix…".to_string());
    {
        let mut cmd = AsyncCommand::new(get_umu_run_path());
        configure_umu_command_async(&mut cmd, &prefix_path, &proton_path);
        cmd.arg("createprefix");
        // Proton then runs no program and exits 1 even though the prefix is ready,
        // so whether it is ready is read from the prefix.
        let output = run_umu_command(cmd, "createprefix".to_string(), cancel.clone()).await?;
        let registry = Path::new(&prefix_path).join("system.reg");
        if !tokio::task::spawn_blocking(move || registry.is_file())
            .await
            .map_err(join_err)?
        {
            return Err(gettext("Proton could not prepare the prefix: {}").replacen(
                "{}",
                &stderr_tail(&output.stderr),
                1,
            ));
        }
    }

    let mut completed_steps = 0usize;
    for profile in &install_plan {
        let steps = get_dep_steps(profile.id);
        let mut recorded = StepChanges::default();

        // Parallelize independent downloads
        let download_steps: Vec<&DepStep> = steps
            .iter()
            .filter(|s| matches!(s.action, DepStepAction::DownloadFile { .. }))
            .collect();
        let execution_steps: Vec<&DepStep> = steps
            .iter()
            .filter(|s| !matches!(s.action, DepStepAction::DownloadFile { .. }))
            .collect();

        if !download_steps.is_empty() {
            let description = "Downloading files…";
            info!(
                "[dep:{}] {} {}/{}",
                profile.id,
                description,
                completed_steps + 1,
                total_steps
            );
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
            let (prefix_ref, proton_ref, cache_ref) = (
                prefix_path.as_str(),
                proton_path.as_str(),
                cache_dir.as_str(),
            );
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
                        if first_error.is_none()
                            || (error != gettext("Cancelled.")
                                && first_error.as_deref() == Some(gettext("Cancelled.").as_str()))
                        {
                            first_error = Some(error);
                        }
                    }
                }
            }
            if let Some(error) = first_error {
                if cancel.load(Ordering::Relaxed) {
                    return Err(gettext("Cancelled."));
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
        let (pending, snapshot_before) = {
            let (prefix, dep_id) = (prefix_path.clone(), profile.id.to_string());
            tokio::task::spawn_blocking(move || {
                record_pending(&prefix, &dep_id, &snapshot_before)
                    .map(|path| (path, snapshot_before))
            })
            .await
            .map_err(join_err)
            .and_then(|r| r)?
        };

        let mut step_error: Option<String> = None;
        for step in &execution_steps {
            if cancel.load(Ordering::Relaxed) {
                step_error = Some(gettext("Cancelled."));
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
                .and_then(|r| r)
                {
                    warn!(
                        "[dep:{}] cleanup failed during rollback: {}",
                        profile.id, cleanup_err
                    );
                }
            }
            let _ = tokio::fs::remove_file(&pending).await;
            return Err(error);
        }

        {
            let upsert_prefix = prefix_path.clone();
            let upsert_id = profile.id.to_string();
            let upsert_deps: Vec<String> =
                profile.dependencies.iter().map(|d| d.to_string()).collect();
            let upsert_record = recorded.into_dependency_record();
            tokio::task::spawn_blocking(move || {
                let upsert_deps_refs: Vec<&str> = upsert_deps.iter().map(|d| d.as_str()).collect();
                upsert_installed_dep(
                    &upsert_prefix,
                    &upsert_id,
                    &upsert_deps_refs,
                    &upsert_record,
                )
            })
            .await
            .map_err(join_err)
            .and_then(|r| r)?;
        }
        let _ = tokio::fs::remove_file(&pending).await;

        for provided_id in profile.provides {
            let stub_prefix = prefix_path.clone();
            let stub_id = provided_id.to_string();
            let stub = InstalledDependency::default();
            let deps: Vec<&str> = Vec::new();
            if let Err(error) = tokio::task::spawn_blocking(move || {
                upsert_installed_dep(&stub_prefix, &stub_id, &deps, &stub)
            })
            .await
            .map_err(join_err)
            .and_then(|r| r)
            {
                warn!(
                    "[dep:{}] failed to create stub for '{}': {}",
                    profile.id, provided_id, error
                );
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
            Some(gettext("Installed prerequisites: {}.").replacen(
                "{}",
                &prerequisites.join(", "),
                1,
            ))
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
        return Err(gettext(
            "Another dependency operation is already running for this prefix.",
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
            return Err(gettext("Dependency state file is corrupt: {}").replacen("{}", &err, 1));
        }
    };

    let installed = match state.installed.get(&dep_id).cloned() {
        Some(installed) => installed,
        None => return Ok(Some(gettext("Dependency is no longer tracked."))),
    };
    let dependents = find_installed_dependents(&state, &dep_id);
    if !dependents.is_empty() {
        return Err(gettext("Cannot remove '{}': still required by {}.")
            .replacen("{}", &dep_id, 1)
            .replacen("{}", &dependents.join(", "), 1));
    }

    let uninstall_entries = if installed.uninstall_entries.is_empty() {
        let prefix = prefix_path.clone();
        let files = installed.created_files.clone();
        tokio::task::spawn_blocking(move || bundles_among(&prefix, &files))
            .await
            .map_err(join_err)?
    } else {
        installed.uninstall_entries.clone()
    };
    let actions = build_cleanup_actions(&InstalledDependency {
        dll_overrides: overrides_to_remove(&state, &dep_id),
        uninstall_entries,
        ..installed.clone()
    });

    let requires_umu = actions.iter().any(|(_, action)| {
        matches!(
            action,
            CleanupAction::RemoveDllOverrides(_)
                | CleanupAction::UnregisterDlls(_)
                | CleanupAction::RunUninstallers(_)
        )
    });
    if requires_umu {
        ensure_umu_ready(false, &on_progress, &cancel).await?;
        check_proton(&proton_path).await?;
    }

    info!(
        "[dep:{}] starting removal ({} cleanup actions)",
        dep_id,
        actions.len()
    );

    let total_actions = actions.len();
    for (index, (description, action)) in actions.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(gettext("Cancelled."));
        }
        on_progress(index + 1, total_actions, description.clone());

        let prefix_path = prefix_path.clone();
        let proton_path = proton_path.clone();
        let cache_dir = cache_dir.clone();

        let result = match action {
            CleanupAction::RemoveDllOverrides(dlls) => {
                remove_dll_overrides(
                    &prefix_path,
                    &proton_path,
                    &cache_dir,
                    &dlls,
                    cancel.clone(),
                )
                .await
            }
            CleanupAction::UnregisterDlls(dlls) => {
                unregister_dlls(&prefix_path, &proton_path, &dlls, cancel.clone()).await
            }
            CleanupAction::RunUninstallers(entries) => {
                run_uninstallers(&prefix_path, &proton_path, &entries, cancel.clone()).await
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

    {
        // A verb winetricks.log still lists counts as installed for winetricks
        // and for umu, which skip it.
        let forget_prefix = prefix_path.clone();
        let verbs = verbs_to_forget(&state, &dep_id);
        tokio::task::spawn_blocking(move || {
            verbs
                .iter()
                .try_for_each(|verb| forget_winetricks_verb(&forget_prefix, verb))
        })
        .await
        .map_err(join_err)
        .and_then(|r| r)
        .inspect_err(|error| error!("[dep:{}] removal failed: {}", dep_id, error))?;
    }

    {
        let remove_prefix = prefix_path.clone();
        let remove_id = dep_id.clone();
        tokio::task::spawn_blocking(move || remove_installed_dep(&remove_prefix, &remove_id))
            .await
            .map_err(join_err)
            .and_then(|r| r)?;
    }

    if let Some(profile) = get_dep_profile(&dep_id) {
        for provided_id in profile.provides {
            if let Some(entry) = state.installed.get(*provided_id)
                && !entry.has_removable_changes()
                && !entry.touched_existing_files
            {
                let remove_prefix = prefix_path.clone();
                let remove_id = provided_id.to_string();
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    remove_installed_dep(&remove_prefix, &remove_id)
                })
                .await
                .map_err(join_err)
                .and_then(|r| r)
                {
                    warn!(
                        "[dep:{}] failed to remove stub for '{}': {}",
                        dep_id, provided_id, error
                    );
                }
            }
        }
    }

    let note = match (
        installed.has_removable_changes(),
        installed.touched_existing_files,
    ) {
        (true, true) => Some(gettext(
            "Some existing prefix files were changed during installation and were not reverted.",
        )),
        (false, true) => Some(gettext(
            "Removed from tracking. Existing prefix files changed during installation were not reverted.",
        )),
        (false, false) => Some(gettext("Removed from tracking.")),
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
        return Err(gettext("umu-launcher is still downloading, please wait…"));
    }

    if !tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or_else(|e| {
            log::warn!("is_umu_run_available task failed: {}", join_err(e));
            false
        })
    {
        info!("[dep] umu-launcher not available");
        return Err(gettext(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
    }
    info!("[dep] umu-launcher is available");

    if check_winetricks {
        info!("[dep] Checking winetricks availability…");
        if WINETRICKS_DOWNLOADING.load(Ordering::Relaxed) {
            return Err(gettext("winetricks is still downloading, please wait…"));
        }

        if !tokio::task::spawn_blocking(is_winetricks_available)
            .await
            .unwrap_or_else(|e| {
                log::warn!("is_winetricks_available task failed: {}", join_err(e));
                false
            })
        {
            info!("[dep] winetricks not found, triggering download");
            on_progress(0, 0, gettext("Downloading winetricks…"));

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
                    gettext("Failed to download winetricks. Check your internet connection.")
                })?;
            }

            // Wait for the download to complete if another caller started it
            if WINETRICKS_DOWNLOADING.load(Ordering::Relaxed) {
                info!("[dep] Waiting for winetricks download from another caller…");
            }
            while WINETRICKS_DOWNLOADING.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(gettext("Cancelled."));
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
        info!(
            "[dep] Prefix {} does not exist, empty snapshot",
            prefix_path
        );
        return Ok(PrefixSnapshot::default());
    }

    let mut snapshot = PrefixSnapshot {
        proton_files: proton_files(root),
        dll_overrides: dll_overrides(prefix_path),
        winetricks_verbs: winetricks_verbs(prefix_path),
        uninstall_entries: uninstall_entries(prefix_path).into_keys().collect(),
        ..PrefixSnapshot::default()
    };
    collect_snapshot(root, root, &mut snapshot)?;
    info!(
        "[dep] Snapshot: {} files in {}",
        snapshot.files.len(),
        prefix_path
    );
    Ok(snapshot)
}

/// What Proton owns in the prefix: the files its `tracked_files` lists, and its
/// own bookkeeping at the top. Proton rewrites some of them on every run, the
/// DXVK and VKD3D libraries among them, and puts them back on an upgrade.
fn proton_files(root: &Path) -> HashSet<String> {
    let mut files: HashSet<String> = fs::read_to_string(root.join("tracked_files"))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty() && !line.ends_with('/'))
        .map(str::to_string)
        .collect();
    files.extend(
        [
            "tracked_files",
            "version",
            "config_info",
            "pfx.lock",
            "creation_sync_guard",
        ]
        .map(str::to_string),
    );
    files
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

        // The sandbox's own directory in the prefix: caches, nothing a
        // dependency installs.
        if current == root && path.file_name().is_some_and(|n| n == ".leyen") {
            continue;
        }

        // Skip temp/cache directories irrelevant to dependency state
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            let lower = n.to_ascii_lowercase();
            matches!(lower.as_str(), "temp" | "tmp" | "cache" | "prefetch")
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
        let proton = before.proton_files.contains(path) || after.proton_files.contains(path);
        match before.files.get(path) {
            // Removing one of Proton's files would break the prefix.
            None if proton => {}
            None => changes.created_files.push(path.clone()),
            // Proton copies its files again with every run; a component that
            // replaced one changed its size.
            Some(previous) if proton && previous.len == fingerprint.len => {}
            Some(previous) if previous != fingerprint => changes.touched_existing_files = true,
            _ => {}
        }
    }

    if before
        .files
        .keys()
        .any(|path| !after.files.contains_key(path) && !before.proton_files.contains(path))
    {
        changes.touched_existing_files = true;
    }

    changes.created_files.sort();
    changes.created_files.dedup();
    // An override that was there before stays: it is not this component's.
    changes.dll_overrides = after
        .dll_overrides
        .difference(&before.dll_overrides)
        .cloned()
        .collect();
    changes.winetricks_verbs = after
        .winetricks_verbs
        .difference(&before.winetricks_verbs)
        .cloned()
        .collect();
    changes.uninstall_entries = after
        .uninstall_entries
        .difference(&before.uninstall_entries)
        .cloned()
        .collect();
    info!(
        "[dep] Diff: {} files created, touched_existing={}",
        changes.created_files.len(),
        changes.touched_existing_files
    );
    changes
}

/// The DLL overrides `verb` sets, as winetricks writes them: with a leading
/// `*`, and removed only under that name.
fn winetricks_known_dll_overrides(verb: &str) -> Vec<String> {
    let dlls = match verb {
        "d3dx9" => (24..=43).map(|n| format!("d3dx9_{n}")).collect(),
        "d3dcompiler_42" | "d3dcompiler_43" | "d3dcompiler_46" | "d3dcompiler_47" | "d3dx11_42"
        | "d3dx11_43" | "dx8vb" | "amstream" | "devenum" | "dmband" | "dmcompos" | "dmime"
        | "dmloader" | "dmscript" | "dmstyle" | "dmsynth" | "dmusic" | "dmusic32" | "dsound"
        | "dswave" | "qasf" | "qcap" | "qdvd" | "qedit" | "quartz" => vec![verb.to_string()],
        _ => Vec::new(),
    };
    dlls.into_iter().map(|dll| format!("*{dll}")).collect()
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

/// The DLL overrides of `dep_id` that no other installed component set too:
/// removing one another needs would break that one (dotnet40 and dotnet48 both
/// set mscoree).
fn overrides_to_remove(
    state: &leyen_model::deps::PrefixDependencyState,
    dep_id: &str,
) -> Vec<String> {
    let Some(installed) = state.installed.get(dep_id) else {
        return Vec::new();
    };
    set_overrides(dep_id, installed)
        .into_iter()
        .filter(|dll| {
            !state
                .installed
                .iter()
                .any(|(id, other)| id != dep_id && set_overrides(id, other).contains(dll))
        })
        .collect()
}

/// The overrides `dep_id` set. A winetricks component recorded before its
/// verbs were recorded has its overrides named without winetricks' `*`, and
/// with verbs that set others' too, so they come from what its verbs set.
fn set_overrides(dep_id: &str, installed: &InstalledDependency) -> Vec<String> {
    let verbs = recipe_verbs(dep_id);
    if verbs.is_empty() || !installed.winetricks_verbs.is_empty() {
        return installed.dll_overrides.clone();
    }
    verbs
        .iter()
        .flat_map(|verb| winetricks_known_dll_overrides(verb))
        .collect()
}

/// The winetricks verbs the steps of `dep_id` run.
fn recipe_verbs(dep_id: &str) -> Vec<String> {
    get_dep_steps(dep_id)
        .into_iter()
        .filter_map(|step| match step.action {
            DepStepAction::RunWinetricks { verb } => Some(verb),
            _ => None,
        })
        .collect()
}

/// The verbs to drop from winetricks.log with `dep_id`: those its install
/// logged, and its own, unless another installed component has them too.
fn verbs_to_forget(state: &leyen_model::deps::PrefixDependencyState, dep_id: &str) -> Vec<String> {
    let verbs = |id: &str, installed: &InstalledDependency| -> BTreeSet<String> {
        let mut verbs: BTreeSet<String> = installed.winetricks_verbs.iter().cloned().collect();
        verbs.extend(recipe_verbs(id));
        verbs
    };
    let Some(installed) = state.installed.get(dep_id) else {
        return Vec::new();
    };
    verbs(dep_id, installed)
        .into_iter()
        .filter(|verb| {
            !state
                .installed
                .iter()
                .any(|(id, other)| id != dep_id && verbs(id, other).contains(verb))
        })
        .collect()
}

fn build_cleanup_actions(installed: &InstalledDependency) -> Vec<(String, CleanupAction)> {
    let mut actions = Vec::new();

    // First, while the uninstallers it keeps among its files are still there.
    if !installed.uninstall_entries.is_empty() {
        actions.push((
            "Running the uninstallers…".to_string(),
            CleanupAction::RunUninstallers(installed.uninstall_entries.clone()),
        ));
    }

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

/// Removes the programs registered under `entries` the way Windows would: a
/// program with a quiet uninstall command through it, which for an installer
/// bundle also removes the Windows Installer packages it brought, and the rest
/// of Windows Installer's own with `msiexec /x`. Deleting their files alone
/// leaves them registered, and their installer then refuses to put them back.
async fn run_uninstallers(
    prefix_path: &str,
    proton_path: &str,
    entries: &[String],
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let registered = |prefix: String| async move {
        tokio::task::spawn_blocking(move || uninstall_entries(&prefix))
            .await
            .map_err(join_err)
    };
    for quiet in [true, false] {
        let present = registered(prefix_path.to_string()).await?;
        for key in entries {
            let Some(entry) = present.get(key) else {
                continue;
            };
            let mut cmd = AsyncCommand::new(get_umu_run_path());
            configure_umu_command_async(&mut cmd, prefix_path, proton_path);
            match (quiet, &entry.quiet_command) {
                (true, Some(command)) => {
                    let Some((program, args)) = windows_command(prefix_path, command) else {
                        continue;
                    };
                    cmd.arg(program);
                    cmd.args(args);
                }
                (false, _) if entry.msi => {
                    let Some(code) = key.rsplit('\\').next() else {
                        continue;
                    };
                    cmd.args(["msiexec.exe", "/x", code, "/qn"]);
                }
                _ => continue,
            }
            info!("[dep] Uninstalling '{}'", entry.name);
            run_umu_command(cmd, format!("uninstall {}", entry.name), cancel.clone()).await?;
        }
    }
    // Their exit status says little through umu-run; what is still registered
    // says whether they went.
    let present = registered(prefix_path.to_string()).await?;
    let left: Vec<&str> = entries
        .iter()
        .filter_map(|key| present.get(key))
        .map(|entry| entry.name.as_str())
        .collect();
    if left.is_empty() {
        Ok(())
    } else {
        Err(format!("Could not uninstall {}", left.join(", ")))
    }
}

/// The programs registered for removal in the prefix, by their key under
/// `HKEY_LOCAL_MACHINE`, from `system.reg`, which Wine writes once a prefix's
/// wineserver has exited.
fn uninstall_entries(prefix_path: &str) -> BTreeMap<String, UninstallEntry> {
    const SUFFIX: &str = "\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\";
    let text = fs::read(Path::new(prefix_path).join("system.reg"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let mut entries = BTreeMap::new();
    let mut current: Option<(String, UninstallEntry)> = None;
    for line in text.lines() {
        if let Some(header) = line.strip_prefix('[') {
            entries.extend(current.take());
            let Some((key, _)) = header.split_once(']') else {
                continue;
            };
            let key = unescape_reg(key);
            let Some((view, name)) = key.split_once(SUFFIX) else {
                continue;
            };
            if (view == "Software" || view == "Software\\Wow6432Node") && !name.contains('\\') {
                let entry = UninstallEntry {
                    name: name.to_string(),
                    quiet_command: None,
                    msi: false,
                };
                current = Some((key.clone(), entry));
            }
        } else if let Some((_, entry)) = current.as_mut() {
            if let Some(value) = line.strip_prefix("\"DisplayName\"=") {
                entry.name = reg_string(value);
            } else if let Some(value) = line.strip_prefix("\"QuietUninstallString\"=") {
                entry.quiet_command = Some(reg_string(value));
            } else if line == "\"WindowsInstaller\"=dword:00000001" {
                entry.msi = true;
            }
        }
    }
    entries.extend(current);
    entries
}

/// For a component recorded before its uninstall entries were: the programs
/// whose quiet uninstaller is one of its files, an installer bundle in the
/// package cache, which removes the packages it installed with it.
fn bundles_among(prefix_path: &str, created_files: &[String]) -> Vec<String> {
    let root = Path::new(prefix_path);
    let files: HashSet<PathBuf> = created_files.iter().map(|file| root.join(file)).collect();
    uninstall_entries(prefix_path)
        .into_iter()
        .filter(|(_, entry)| {
            entry
                .quiet_command
                .as_deref()
                .and_then(|command| windows_command(prefix_path, command))
                .is_some_and(|(program, _)| files.contains(&program))
        })
        .map(|(key, _)| key)
        .collect()
}

/// A string value as `system.reg` writes it: quoted, escaped, and after a
/// `str(n):` type tag when it is not a plain string.
fn reg_string(value: &str) -> String {
    let value = value
        .strip_prefix("str(")
        .and_then(|rest| rest.split_once("):"))
        .map_or(value, |(_, rest)| rest);
    unescape_reg(value.trim_matches('"'))
}

fn unescape_reg(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A Windows command line on drive C: of the prefix, as the program's path in
/// the prefix and its arguments.
fn windows_command(prefix_path: &str, command: &str) -> Option<(PathBuf, Vec<String>)> {
    let command = command.trim();
    let (program, args) = match command.strip_prefix('"') {
        Some(rest) => rest.split_once('"')?,
        None => command.split_once(' ').unwrap_or((command, "")),
    };
    let relative = program
        .strip_prefix("C:\\")
        .or_else(|| program.strip_prefix("c:\\"))?;
    let path = Path::new(prefix_path)
        .join("drive_c")
        .join(relative.replace('\\', "/"));
    Some((path, args.split_whitespace().map(str::to_string).collect()))
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
        // plain relative components, or a folder on the way that is a link (Wine
        // links the user folders to the home directory), could reach outside it.
        if !Path::new(relative)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
            || passes_through_link(prefix_root, relative)
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

/// Whether a folder between `prefix_root` and the file `relative` is a symlink.
fn passes_through_link(prefix_root: &Path, relative: &str) -> bool {
    let mut folder = prefix_root.to_path_buf();
    let Some(parent) = Path::new(relative).parent() else {
        return false;
    };
    parent.components().any(|component| {
        folder.push(component);
        fs::symlink_metadata(&folder).is_ok_and(|meta| meta.file_type().is_symlink())
    })
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

#[cfg(test)]
mod tests {
    use super::{
        FileFingerprint, PrefixSnapshot, diff_snapshots, dll_overrides, forget_winetricks_verb,
        overrides_to_remove, remove_created_files, stderr_tail, uninstall_entries, verbs_to_forget,
        windows_command, winetricks_known_dll_overrides,
    };
    use leyen_model::deps::{InstalledDependency, PrefixDependencyState};
    use std::collections::BTreeMap;

    #[test]
    fn proton_copying_its_own_files_again_is_not_a_change() {
        let snapshot = |files: &[(&str, u64, u64)]| PrefixSnapshot {
            files: files
                .iter()
                .map(|&(path, len, modified_epoch_seconds)| {
                    let fingerprint = FileFingerprint {
                        len,
                        modified_epoch_seconds,
                    };
                    (path.to_string(), fingerprint)
                })
                .collect(),
            proton_files: ["drive_c/windows/system32/d3d11.dll", "version"]
                .map(str::to_string)
                .into(),
            ..PrefixSnapshot::default()
        };
        let before = snapshot(&[
            ("drive_c/windows/system32/d3d11.dll", 100, 1),
            ("version", 15, 1),
        ]);

        let recopied = snapshot(&[
            ("drive_c/windows/system32/d3d11.dll", 100, 2),
            ("version", 15, 2),
            ("drive_c/windows/Fonts/tahoma.ttf", 50, 2),
        ]);
        let changes = diff_snapshots(&before, &recopied);
        assert_eq!(changes.created_files, ["drive_c/windows/Fonts/tahoma.ttf"]);
        assert!(!changes.touched_existing_files);

        let replaced = snapshot(&[
            ("drive_c/windows/system32/d3d11.dll", 120, 2),
            ("version", 15, 2),
        ]);
        assert!(diff_snapshots(&before, &replaced).touched_existing_files);
    }

    #[test]
    fn overrides_are_read_from_their_own_registry_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("user.reg"),
            "[Software\\\\Wine\\\\Direct3D] 1\n\"renderer\"=\"vulkan\"\n\n\
             [Software\\\\Wine\\\\DllOverrides] 2\n#time=1\n\"*vcruntime140\"=\"native,builtin\"\n\
             \"d3d11\"=\"native\"\n\n[Software\\\\Wine\\\\Fonts] 3\n\"x\"=\"y\"\n",
        )
        .unwrap();
        let names: Vec<String> = dll_overrides(dir.path().to_str().unwrap())
            .into_iter()
            .collect();
        assert_eq!(names, ["*vcruntime140", "d3d11"]);
    }

    #[test]
    fn a_verb_another_component_logged_too_stays_in_the_log() {
        let with = |verbs: &[&str]| InstalledDependency {
            winetricks_verbs: verbs.iter().map(|verb| verb.to_string()).collect(),
            ..InstalledDependency::default()
        };
        let state = PrefixDependencyState {
            installed: BTreeMap::from([
                (
                    "vcredist2019".to_string(),
                    with(&["ucrtbase2019", "vcrun2019"]),
                ),
                (
                    "vcredist2015".to_string(),
                    with(&["ucrtbase2019", "vcrun2015"]),
                ),
            ]),
            ..PrefixDependencyState::default()
        };
        assert_eq!(verbs_to_forget(&state, "vcredist2019"), ["vcrun2019"]);
    }

    #[test]
    fn uninstall_entries_are_read_from_both_registry_views() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().to_str().unwrap();
        std::fs::write(
            dir.path().join("system.reg"),
            concat!(
                "[Software\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Uninstall\\\\{9D36}] 1\n",
                "\"DisplayName\"=\"Runtime x64\"\n",
                "\"UninstallString\"=str(2):\"MsiExec.exe /I{9D36}\"\n",
                "\"WindowsInstaller\"=dword:00000001\n\n",
                "[Software\\\\Wow6432Node\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Uninstall\\\\{a11e}] 1\n",
                "\"DisplayName\"=\"Redistributable x86\"\n",
                "\"QuietUninstallString\"=\"\\\"C:\\\\ProgramData\\\\Package Cache\\\\{a11e}\\\\VC_redist.x86.exe\\\" /uninstall /quiet\"\n\n",
                "[Software\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Uninstall\\\\{9D36}\\\\Sub] 1\n",
                "\"DisplayName\"=\"not an entry\"\n",
            ),
        )
        .unwrap();
        let entries = uninstall_entries(prefix);
        let keys: Vec<&str> = entries.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{9D36}",
                "Software\\Wow6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{a11e}",
            ]
        );
        let msi = &entries[keys[0]];
        assert!(msi.msi && msi.quiet_command.is_none() && msi.name == "Runtime x64");
        let bundle = &entries[keys[1]];
        let (program, args) =
            windows_command(prefix, bundle.quiet_command.as_deref().unwrap()).unwrap();
        assert_eq!(
            program,
            dir.path()
                .join("drive_c/ProgramData/Package Cache/{a11e}/VC_redist.x86.exe")
        );
        assert_eq!(args, ["/uninstall", "/quiet"]);
    }

    #[test]
    fn an_override_another_component_set_too_is_kept() {
        let with = |dlls: &[&str]| InstalledDependency {
            dll_overrides: dlls.iter().map(|dll| dll.to_string()).collect(),
            ..InstalledDependency::default()
        };
        let state = PrefixDependencyState {
            installed: BTreeMap::from([
                ("dotnet40".to_string(), with(&["mscoree"])),
                ("dotnet48".to_string(), with(&["mscoree", "fusion"])),
            ]),
            ..PrefixDependencyState::default()
        };

        assert_eq!(overrides_to_remove(&state, "dotnet48"), vec!["fusion"]);
        assert!(overrides_to_remove(&state, "dotnet40").is_empty());
    }

    #[test]
    fn an_earlier_record_removes_only_what_its_verb_set() {
        let with = |dlls: &[&str]| InstalledDependency {
            dll_overrides: dlls.iter().map(|dll| dll.to_string()).collect(),
            ..InstalledDependency::default()
        };
        let state = PrefixDependencyState {
            installed: BTreeMap::from([
                (
                    "qedit".to_string(),
                    with(&["qasf", "qcap", "qdvd", "qedit"]),
                ),
                ("qcap".to_string(), with(&["*qcap"])),
            ]),
            ..PrefixDependencyState::default()
        };

        assert_eq!(overrides_to_remove(&state, "qedit"), vec!["*qedit"]);
        assert_eq!(overrides_to_remove(&state, "qcap"), vec!["*qcap"]);
    }

    #[test]
    fn winetricks_overrides_are_named_as_winetricks_writes_them() {
        assert_eq!(winetricks_known_dll_overrides("qcap"), vec!["*qcap"]);
        assert_eq!(
            winetricks_known_dll_overrides("d3dx11_43"),
            vec!["*d3dx11_43"]
        );
        assert_eq!(winetricks_known_dll_overrides("d3dx9").len(), 20);
        assert!(winetricks_known_dll_overrides("dsdmo").is_empty());
    }

    #[test]
    fn a_failure_message_keeps_the_last_lines() {
        let output: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let tail = stderr_tail(output.as_bytes());
        assert!(tail.starts_with("line 21\n"), "{tail}");
        assert!(tail.ends_with("line 30"), "{tail}");
        assert_eq!(stderr_tail(b"  only\n"), "only");
    }

    #[test]
    fn a_forgotten_verb_leaves_the_others_in_the_log() {
        let prefix = tempfile::tempdir().unwrap();
        let log = prefix.path().join("winetricks.log");
        std::fs::write(&log, "vcrun2019\nd3dx9\nvcrun2019\ncorefonts\n").unwrap();

        forget_winetricks_verb(prefix.path().to_str().unwrap(), "vcrun2019").unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "d3dx9\ncorefonts\n");
    }

    #[test]
    fn a_tracked_file_behind_a_linked_folder_is_not_removed() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let outside = home.path().join("save.dat");
        std::fs::write(&outside, b"keep").unwrap();
        let prefix = root.path().join("pfx");
        std::fs::create_dir_all(prefix.join("drive_c/users/me")).unwrap();
        std::os::unix::fs::symlink(home.path(), prefix.join("drive_c/users/me/Documents")).unwrap();
        std::fs::write(prefix.join("drive_c/own.dll"), b"x").unwrap();

        let prefix_str = prefix.to_str().unwrap();
        assert!(
            remove_created_files(
                prefix_str,
                &["drive_c/users/me/Documents/save.dat".to_string()]
            )
            .is_err()
        );
        assert!(outside.exists());

        remove_created_files(prefix_str, &["drive_c/own.dll".to_string()]).unwrap();
        assert!(!prefix.join("drive_c/own.dll").exists());
    }
}
