use crate::t;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, BufReader as AsyncBufReader};


use gtk4::glib;
use libadwaita as adw;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{
    add_game_playtime, effective_game_id, find_game_and_group, get_config_dir, load_library,
    load_settings_with_auto_install, record_game_launch_result, record_game_launch_start,
};
use crate::models::{Game, GameGroup};
use crate::runtime::proton::resolve_proton_path;
use crate::runtime::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};
use crate::tools::{gamemode_available, join_err, mangohud_available};

#[derive(Debug, Clone)]
pub struct LaunchReport {
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct RunningGamesRegistry {
    sessions: Vec<RunningGameSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct RunningGameSession {
    game_id: String,
    pid: u32,
    known_pids: Vec<u32>,
    started_at_epoch_seconds: u64,
    match_prefix_path: Option<String>,
    match_game_id: Option<String>,
    /// Lowercased basename of the game executable (e.g. `client.exe`). Primary
    /// identity for finding this game's processes by `/proc/PID/cmdline`, since
    /// environ is unreadable for Wine processes in the idmapped container.
    #[serde(default)]
    match_exe: Option<String>,
    /// Lowercased launch arguments, used to tell apart multiple instances of the
    /// same executable launched concurrently (e.g. different `user:` accounts).
    #[serde(default)]
    match_args: Option<String>,
    termination_requested: bool,
}

#[derive(Debug, Clone)]
pub struct RunningGameSnapshot {
    pub game_id: String,
    pub pid: u32,
    pub tracked_pid_count: usize,
    pub elapsed_seconds: u64,
    pub started_at_epoch_seconds: u64,
}

#[derive(Error, Debug)]
pub enum LaunchError {
    #[error("Failed to prepare runtime lock directory: {0}")]
    LockDirectoryError(#[from] std::io::Error),
    #[error("Failed to lock runtime state '{path}': {source}")]
    LockAcquireError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to serialize running games state: {0}")]
    SerializationError(String),
    #[error("Failed to write running games state '{path}': {source}")]
    WriteError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to read running games state '{path}': {source}")]
    ReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Launch failed: {0}")]
    Other(String),
}

enum PrefixLockState {
    Available,
    Busy,
    Unavailable,
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_else(|e| {
            log::warn!("system clock before UNIX_EPOCH: {e}");
            0
        })
}

fn running_registry_path() -> PathBuf {
    get_config_dir().join("running.toml")
}

fn running_registry_lock_path() -> PathBuf {
    get_config_dir().join(".running.lock")
}

/// RAII guard that acquires `LOCK_EX | LOCK_NB` with retry + timeout.
/// Releases the lock on Drop — panic-safe.
struct FlockGuard {
    file: File,
}

impl FlockGuard {
    fn lock(path: &Path, timeout: Duration) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| io::Error::new(e.kind(), format!("create_dir_all '{}': {}", path.display(), e)))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        let fd = file.as_raw_fd();

        let start = std::time::Instant::now();
        loop {
            let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Self { file });
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::WouldBlock {
                return Err(err);
            }
            if start.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Timed out waiting for lock on '{}' after {:?}",
                        path.display(),
                        timeout
                    ),
                ));
            }
            sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn with_running_registry<R>(
    f: impl FnOnce(&mut RunningGamesRegistry) -> (R, bool),
) -> Result<R, LaunchError> {
    let _guard = FlockGuard::lock(&running_registry_lock_path(), Duration::from_secs(5))
        .map_err(|e| LaunchError::LockAcquireError {
            path: running_registry_lock_path(),
            source: e,
        })?;

    let registry_path = running_registry_path();
    let mut registry = match fs::read_to_string(&registry_path) {
        Ok(data) => toml::from_str::<RunningGamesRegistry>(&data)
            .map_err(|e| LaunchError::SerializationError(e.to_string()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RunningGamesRegistry::default(),
        Err(e) => {
            return Err(LaunchError::ReadError {
                path: registry_path,
                source: e,
            });
        }
    };

    let (result, dirty) = f(&mut registry);

    if dirty {
        let data = toml::to_string_pretty(&registry)
            .map_err(|e| LaunchError::SerializationError(e.to_string()))?;
        fs::write(&registry_path, data).map_err(|e| LaunchError::WriteError {
            path: registry_path,
            source: e,
        })?;
    }

    Ok(result)
}

fn split_shell_words(input: &str) -> Vec<String> {
    shlex::split(input).unwrap_or_else(|| input.split_whitespace().map(str::to_string).collect())
}

fn is_valid_env_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn running_sessions_to_snapshots(sessions: &[RunningGameSession]) -> Vec<RunningGameSnapshot> {
    let now = current_epoch_seconds();
    let mut snapshots: Vec<RunningGameSnapshot> = sessions
        .iter()
        .map(|session| RunningGameSnapshot {
            game_id: session.game_id.clone(),
            pid: session.pid,
            tracked_pid_count: session.known_pids.len(),
            elapsed_seconds: now.saturating_sub(session.started_at_epoch_seconds),
            started_at_epoch_seconds: session.started_at_epoch_seconds,
        })
        .collect();

    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.elapsed_seconds));
    snapshots
}

fn running_sessions_version(sessions: &[RunningGameSession]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    if sessions.is_empty() {
        return 0;
    }

    let mut ordered = sessions.to_vec();
    ordered.sort_by(|left, right| left.game_id.cmp(&right.game_id));

    // Hash only fields that identify the running *set* and its discrete state
    // transitions. Deliberately exclude `known_pids.len()`: Wine/Proton spawn and
    // reap helper processes constantly, so the tracked-pid count flutters on every
    // 1s monitor tick. Hashing it bumped the version each second while a game ran,
    // forcing a full library teardown+rebuild on the GTK main thread every tick.
    let mut hasher = DefaultHasher::new();
    for session in &ordered {
        session.game_id.hash(&mut hasher);
        session.pid.hash(&mut hasher);
        session.started_at_epoch_seconds.hash(&mut hasher);
        session.termination_requested.hash(&mut hasher);
    }

    hasher.finish().max(1)
}

async fn finalize_finished_session(session: &RunningGameSession) {
    let elapsed_seconds = current_epoch_seconds().saturating_sub(session.started_at_epoch_seconds);
    let status = if session.termination_requested {
        "Last run: stopped"
    } else {
        "Last run: completed"
    };

    let total_playtime = add_game_playtime(&session.game_id, elapsed_seconds).await;
    if !record_game_launch_result(&session.game_id, elapsed_seconds, status).await {
        warn!(target: &format!("game:{}", session.game_id), "Failed to record launch result");
    }

    info!(
        target: &format!("game:{}", session.game_id),
        "Managed session finished after {}s ({})",
        elapsed_seconds, status
    );

    if let Some(total) = total_playtime {
        info!(
            target: &format!("game:{}", session.game_id),
            "Total recorded playtime is now {}s", total
        );
    }
}

async fn synchronize_running_sessions() -> Result<Vec<RunningGameSession>, LaunchError> {
    let (active_sessions, finished_sessions) = tokio::task::spawn_blocking(|| {
        let scan = get_proc_scan();
        let children_map = &scan.children;
        let cmdlines = &scan.cmdlines;
        with_running_registry(|registry| {
            let original_sessions = registry.sessions.clone();
            let mut active_sessions = Vec::new();
            let mut finished_sessions = Vec::new();

            for mut session in registry.sessions.drain(..) {
                if refresh_known_pids(&mut session, children_map, cmdlines).is_empty() {
                    finished_sessions.push(session);
                } else {
                    active_sessions.push(session);
                }
            }

            let dirty = active_sessions != original_sessions;
            registry.sessions = active_sessions.clone();
            ((active_sessions, finished_sessions), dirty)
        })
    })
    .await
    .map_err(|e| LaunchError::Other(join_err(e)))
    .and_then(|r| r)?;

    for session in &finished_sessions {
        finalize_finished_session(session).await;
    }

    Ok(active_sessions)
}

/// Re-scans running sessions and republishes them so the UI reflects the change
/// at once. Used on the launch/stop/own-exit fast paths to avoid waiting for the
/// next background monitor tick. Errors are swallowed — the periodic monitor is
/// the safety net.
async fn republish_running_sessions() {
    match synchronize_running_sessions().await {
        Ok(sessions) => publish_sessions(&sessions),
        Err(e) => warn!("Immediate session republish failed: {e}"),
    }
}

async fn try_register_running_session(session: RunningGameSession) -> Result<bool, LaunchError> {
    let _ = synchronize_running_sessions().await;
    tokio::task::spawn_blocking(move || {
        with_running_registry(|registry| {
            if registry
                .sessions
                .iter()
                .any(|existing| existing.game_id == session.game_id)
            {
                return (false, false);
            }

            registry.sessions.push(session);
            (true, true)
        })
    })
    .await
    .map_err(|e| LaunchError::Other(join_err(e)))
    .and_then(|r| r)
}

fn mark_running_session_termination_requested(game_id: &str) -> Result<bool, LaunchError> {
    with_running_registry(|registry| {
        let mut changed = false;
        if let Some(session) = registry
            .sessions
            .iter_mut()
            .find(|session| session.game_id == game_id)
            && !session.termination_requested
        {
            session.termination_requested = true;
            changed = true;
        }

        (changed, changed)
    })
}

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

static RUNNING_SESSIONS_CACHE: OnceLock<RwLock<Vec<RunningGameSnapshot>>> = OnceLock::new();
static RUNNING_SESSIONS_VERSION_CACHE: AtomicU64 = AtomicU64::new(0);
/// Lock-free mirror of "is any game running", kept in sync with the snapshot
/// cache by the monitor. Lets the GTK main thread (per-second timer, window
/// close handler) answer without taking the `RwLock`.
static ANY_GAME_RUNNING: AtomicBool = AtomicBool::new(false);

/// Wakes the GTK refresh loop the instant running state changes, instead of
/// waiting for the next 1s poll tick. Bounded + drop-on-full: the receiver only
/// needs to know "something changed", so coalescing a burst into one wake is
/// correct. Sender is cloned out; the held receiver keeps the channel open.
static SESSION_EVENT_CHANNEL: OnceLock<(
    async_channel::Sender<()>,
    async_channel::Receiver<()>,
)> = OnceLock::new();

fn session_event_channel() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    SESSION_EVENT_CHANNEL.get_or_init(|| async_channel::bounded(8))
}

/// Returns a receiver that fires whenever running-session state is republished.
pub fn subscribe_session_events() -> async_channel::Receiver<()> {
    session_event_channel().1.clone()
}

/// Single path that makes a new session set visible to the UI: refreshes the
/// snapshot cache, the `any running` mirror and the version atomic, then nudges
/// the event channel so the GTK loop refreshes immediately. Called by the
/// background monitor and by the launch/stop/exit fast paths.
fn publish_sessions(sessions: &[RunningGameSession]) {
    let snapshots = running_sessions_to_snapshots(sessions);
    let version = running_sessions_version(sessions);
    let any_running = !snapshots.is_empty();

    if let Ok(mut cache) = get_running_sessions_cache().write() {
        *cache = snapshots;
    }
    ANY_GAME_RUNNING.store(any_running, Ordering::Relaxed);
    // Release pairs with the Acquire load in running_games_version so a reader
    // seeing the new version also sees the new snapshot.
    RUNNING_SESSIONS_VERSION_CACHE.store(version, Ordering::Release);
    // Non-blocking: a full channel already has a pending wake, which is enough.
    let _ = session_event_channel().0.try_send(());
}

fn get_running_sessions_cache() -> &'static RwLock<Vec<RunningGameSnapshot>> {
    RUNNING_SESSIONS_CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn start_running_sessions_monitor() {
    tokio::spawn(async move {
        let mut consecutive_errors: u32 = 0;
        loop {
            match synchronize_running_sessions().await {
                Ok(sessions) => {
                    publish_sessions(&sessions);
                    consecutive_errors = 0;
                }
                Err(e) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    warn!("Session monitor sync failed (attempt {consecutive_errors}): {e}");
                    // After repeated failures the cached snapshot is stale and may
                    // show phantom "running" games forever. Clear it so the UI
                    // reflects unknown-but-empty rather than a frozen state.
                    if consecutive_errors == 3 {
                        publish_sessions(&[]);
                    }
                }
            }
            let base_delay = 2;
            let max_backoff = 30;
            let delay = base_delay * (1u64 << consecutive_errors.min(4));
            let delay = delay.min(max_backoff);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
    });
}

async fn find_running_session(game_id: &str) -> Result<Option<RunningGameSession>, LaunchError> {
    Ok(synchronize_running_sessions()
        .await?
        .into_iter()
        .find(|session| session.game_id == game_id))
}

pub fn is_game_running(game_id: &str) -> bool {
    get_running_sessions_cache()
        .read()
        .map(|c| c.iter().any(|s| s.game_id == game_id))
        .unwrap_or(false)
}

pub fn is_any_game_running() -> bool {
    ANY_GAME_RUNNING.load(Ordering::Relaxed)
}

pub async fn running_games_version() -> u64 {
    RUNNING_SESSIONS_VERSION_CACHE.load(Ordering::Acquire)
}

pub async fn read_running_games_snapshot() -> Result<Vec<RunningGameSnapshot>, LaunchError> {
    let sessions = tokio::task::spawn_blocking(|| {
        with_running_registry(|registry| (registry.sessions.clone(), false))
    })
    .await
    .map_err(|e| LaunchError::Other(join_err(e)))
    .and_then(|r| r)?;

    Ok(running_sessions_to_snapshots(&sessions))
}

pub async fn running_games_snapshot() -> Vec<RunningGameSnapshot> {
    get_running_sessions_cache()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

pub async fn monitor_running_game(game_id: &str) -> Result<(), LaunchError> {
    loop {
        let active = synchronize_running_sessions().await?;
        if !active.iter().any(|session| session.game_id == game_id) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

pub async fn stop_game(game_id: &str) -> Result<bool, LaunchError> {
    let Some(session) = find_running_session(game_id).await? else {
        return Ok(false);
    };

    let game_id_clone = game_id.to_string();

    let result = tokio::task::spawn_blocking(move || {
        let mut session = session;
        let root = session.pid;

        // Fresh scan (bypass the 1s cache) so we catch children spawned right
        // before the stop request.
        let scan = scan_all_procs();
        let targets = refresh_known_pids(&mut session, &scan.children, &scan.cmdlines);

        // Nothing of *this game's own* processes is alive — already stopped.
        if targets.is_empty() {
            return Ok(true);
        }

        // When another game still shares this container, the process group spans
        // shared infra (wineserver / pressure-vessel) that the co-tenant needs.
        // Suppress the group-kill and signal only this game's own PIDs so the
        // co-tenant keeps running. Sole occupant → full group-kill teardown.
        let allow_group_kill = !prefix_has_other_live_session(&session, &scan);
        if !allow_group_kill {
            info!(
                target: &format!("game:{}", game_id_clone),
                "Container shared with another game — stopping only this game's processes"
            );
        }

        let _ = mark_running_session_termination_requested(&game_id_clone);

        // Graceful first: SIGTERM lets Wine/Proton flush and the game save state.
        let signaled = signal_targets(root, &targets, libc::SIGTERM, allow_group_kill);
        info!(
            target: &format!("game:{}", game_id_clone),
            "Sent SIGTERM to {} target(s) of pid {}", targets.len(), root
        );

        // Wait up to ~3s for a clean exit before escalating, re-discovering this
        // game's processes each poll (children / the container launcher may move
        // or linger).
        let mut survivors = wait_for_session_exit(&mut session, 15);

        if !survivors.is_empty() {
            let forced = signal_targets(root, &survivors, libc::SIGKILL, allow_group_kill);
            info!(
                target: &format!("game:{}", game_id_clone),
                "Escalated to SIGKILL for pid {} ({} survivors)", root, survivors.len()
            );
            // Wait up to ~5s and confirm the processes are actually gone — the
            // shared container's main process can take a moment to wind down.
            survivors = wait_for_session_exit(&mut session, 25);

            if !signaled && !forced {
                return Err(LaunchError::Other(format!(
                    "Failed to signal any process for pid {}: {}",
                    root,
                    std::io::Error::last_os_error()
                )));
            }
        }

        if !survivors.is_empty() {
            warn!(
                target: &format!("game:{}", game_id_clone),
                "{} process(es) of pid {} still alive after stop (likely uninterruptible); \
                 will clear once the kernel reaps them",
                survivors.len(), root
            );
        }

        // Returns "we acted on a live session"; the actual post-stop liveness is
        // reflected truthfully by the republish below, not this flag.
        Ok(true)
    })
    .await
    .map_err(|e| LaunchError::Other(join_err(e)))
    .and_then(|r| r);

    // Reflect the stopped state immediately instead of waiting for the monitor.
    republish_running_sessions().await;

    result
}

fn resolve_launch_prefix(game: &Game, group: Option<&GameGroup>, default_prefix: &str) -> String {
    if !game.prefix_path.trim().is_empty() {
        return game.prefix_path.clone();
    }

    if let Some(group) = group
        && !group.defaults.prefix_path.trim().is_empty()
    {
        return group.defaults.prefix_path.clone();
    }

    default_prefix.to_string()
}

fn resolve_launch_proton(
    game: &Game,
    group: Option<&GameGroup>,
    default_proton: &str,
) -> Option<String> {
    let group_proton = group
        .map(|group| group.defaults.proton.trim())
        .filter(|value| !value.is_empty() && *value != "Default");

    let selected = if game.proton.trim().is_empty() || game.proton == "Default" {
        group_proton.unwrap_or(default_proton)
    } else {
        &game.proton
    };

    resolve_proton_path(selected)
}

fn working_directory_for(exe_path: &str) -> Option<PathBuf> {
    let exe = Path::new(exe_path);
    exe.parent()
        .filter(|parent| parent.exists() && parent.is_dir())
        .map(Path::to_path_buf)
}

async fn try_lock_prefix(prefix_path: &str) -> PrefixLockState {
    if prefix_path.trim().is_empty() {
        return PrefixLockState::Unavailable;
    }

    let path_clone = prefix_path.to_string();
    let create_result = tokio::task::spawn_blocking(move || fs::create_dir_all(&path_clone))
        .await;
    match create_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            warn!("Failed to create prefix directory '{}': {}", prefix_path, e);
            return PrefixLockState::Unavailable;
        }
        Err(e) => {
            warn!("spawn_blocking task failed while creating prefix directory '{}': {e}", prefix_path);
            return PrefixLockState::Unavailable;
        }
    }

    match synchronize_running_sessions().await {
        Ok(sessions) => {
            if sessions
                .iter()
                .any(|session| session.match_prefix_path.as_deref() == Some(prefix_path))
            {
                PrefixLockState::Busy
            } else {
                PrefixLockState::Available
            }
        }
        Err(e) => {
            warn!(
                "Failed to inspect runtime prefix usage '{}': {}",
                prefix_path, e
            );
            PrefixLockState::Unavailable
        }
    }
}

fn read_parent_pid(pid: u32) -> Option<u32> {
    let mut file = File::open(format!("/proc/{pid}/stat")).ok()?;
    let mut buf = [0u8; 1024];
    let n = file.read(&mut buf).ok()?;
    let stat = String::from_utf8_lossy(&buf[..n]);
    let after_name = stat.rsplit_once(") ")?.1;
    let mut fields = after_name.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

fn read_process_comm(pid: u32) -> Option<String> {
    let mut file = File::open(format!("/proc/{pid}/comm")).ok()?;
    let mut buf = [0u8; 64];
    let n = file.read(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).trim().to_string())
}

/// Reads `/proc/PID/cmdline` (NUL-separated argv) as a single space-joined,
/// lowercased string. Unlike `/proc/PID/environ`, cmdline is world-readable —
/// even for Wine processes inside an idmapped pressure-vessel container — so it
/// is the only reliable way to identify which game a process belongs to here.
fn read_process_cmdline(pid: u32) -> Option<String> {
    let mut file = File::open(format!("/proc/{pid}/cmdline")).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    if buf.is_empty() {
        return None;
    }
    let joined: Vec<u8> = buf
        .into_iter()
        .map(|b| if b == 0 { b' ' } else { b })
        .collect();
    let text = String::from_utf8_lossy(&joined).to_lowercase();
    // Collapse whitespace so substring matching against a launch-args signature
    // is insensitive to argv spacing.
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Normalized, lowercased signature of a game's launch arguments, used to tell
/// apart concurrent instances of the same executable. Drops the `%command%`
/// wrapper prefix (only the trailing real args appear in the game's cmdline).
fn cmdline_arg_signature(launch_args: &str) -> Option<String> {
    let tail = match launch_args.split_once("%command%") {
        Some((_, after)) => after,
        None => launch_args,
    };
    let normalized = tail.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

/// Combined result of a single /proc scan: parent→children map + per-PID cmdline.
struct ProcScan {
    children: HashMap<u32, Vec<u32>>,
    cmdlines: HashMap<u32, String>,
}

type ProcScanCache = RwLock<(ProcScan, u64)>;

static PROC_SCAN_CACHE: OnceLock<ProcScanCache> = OnceLock::new();

fn get_proc_scan_cache() -> &'static ProcScanCache {
    PROC_SCAN_CACHE.get_or_init(|| RwLock::new((
        ProcScan { children: HashMap::new(), cmdlines: HashMap::new() },
        0,
    )))
}

/// Returns combined children + cmdlines from a single /proc scan, cached 1s TTL.
fn get_proc_scan() -> ProcScan {
    let now = current_epoch_seconds();
    if let Ok(cache) = get_proc_scan_cache().read()
        && now.saturating_sub(cache.1) < 1 {
            return ProcScan {
                children: cache.0.children.clone(),
                cmdlines: cache.0.cmdlines.clone(),
            };
        }
    let scan = scan_all_procs();
    if let Ok(mut cache) = get_proc_scan_cache().write() {
        *cache = (
            ProcScan {
                children: scan.children.clone(),
                cmdlines: scan.cmdlines.clone(),
            },
            now,
        );
    }
    scan
}

/// Single pass over /proc — collects the parent→children index and every
/// process's cmdline for game-identity matching.
fn scan_all_procs() -> ProcScan {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut cmdlines: HashMap<u32, String> = HashMap::new();

    let Ok(entries) = fs::read_dir("/proc") else {
        return ProcScan { children, cmdlines };
    };

    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };

        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };

        // Build children map from parent PID (all PIDs)
        if let Some(ppid) = read_parent_pid(pid) {
            children.entry(ppid).or_default().push(pid);
        }

        if let Some(cmdline) = read_process_cmdline(pid) {
            cmdlines.insert(pid, cmdline);
        }
    }

    ProcScan { children, cmdlines }
}

fn is_pid_alive(pid: u32) -> bool {
    let mut file = if let Ok(f) = File::open(format!("/proc/{pid}/stat")) {
        f
    } else {
        return false;
    };
    let mut buf = [0u8; 1024];
    let n = file.read(&mut buf).unwrap_or(0);
    let stat = String::from_utf8_lossy(&buf[..n]);
    let after_name = match stat.rsplit_once(") ") {
        Some((_, after)) => after,
        None => return false,
    };
    let mut fields = after_name.split_whitespace();
    let state = match fields.next() {
        Some(s) => s,
        None => return false,
    };

    // Process is alive if it exists and its state is not 'Z' (Zombie)
    state != "Z"
}

/// Reads the process group id (field 5 of /proc/PID/stat). Used to confirm a PID
/// still leads the group we created before issuing a `kill(-pgid)` — guards
/// against signalling an unrelated process that reused the PID.
fn read_proc_pgrp(pid: u32) -> Option<i32> {
    let mut file = File::open(format!("/proc/{pid}/stat")).ok()?;
    let mut buf = [0u8; 1024];
    let n = file.read(&mut buf).ok()?;
    let stat = String::from_utf8_lossy(&buf[..n]);
    let after_name = stat.rsplit_once(") ")?.1;
    let mut fields = after_name.split_whitespace();
    let _state = fields.next()?;
    let _ppid = fields.next()?;
    fields.next()?.parse::<i32>().ok()
}

/// Sends `signal` to every individually tracked PID, and — when `allow_group_kill`
/// is set and `root` still leads its own group — to the whole process group.
/// Returns true if at least one `kill` syscall was accepted.
///
/// `allow_group_kill` is suppressed when another game shares the container: the
/// group spans shared infra (wineserver / pressure-vessel) that co-tenants need,
/// so the stop falls back to precise per-PID kills of this game's own processes.
fn signal_targets(
    root: u32,
    targets: &HashSet<u32>,
    signal: libc::c_int,
    allow_group_kill: bool,
) -> bool {
    let mut signaled = false;

    // Only group-kill when the root PID is still the leader of its own group.
    // After PID reuse the reused process leads a different group (or none), so
    // this avoids killing an unrelated process tree.
    if allow_group_kill
        && read_proc_pgrp(root) == Some(root as i32)
        && unsafe { libc::kill(-(root as i32), signal) } == 0
    {
        signaled = true;
    }

    for &pid in targets {
        if unsafe { libc::kill(pid as i32, signal) } == 0 {
            signaled = true;
        }
    }

    signaled
}

/// Re-discovers a session's live processes by cmdline each poll and returns the
/// set still alive after up to `attempts * 200ms`. Returns early as soon as the
/// game is fully gone. Re-scanning (rather than polling a fixed PID set) confirms
/// the actual game — including the container launcher — has really terminated.
fn wait_for_session_exit(session: &mut RunningGameSession, attempts: u32) -> HashSet<u32> {
    let mut remaining = attempts;
    loop {
        let scan = scan_all_procs();
        let alive = refresh_known_pids(session, &scan.children, &scan.cmdlines);
        if alive.is_empty() || remaining == 0 {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(200));
        remaining -= 1;
    }
}

fn collect_descendant_pids(
    roots: &HashSet<u32>,
    children_map: &HashMap<u32, Vec<u32>>,
) -> HashSet<u32> {
    let mut visited = roots.clone();
    let mut queue: VecDeque<u32> = roots.iter().copied().collect();

    while let Some(pid) = queue.pop_front() {
        if let Some(children) = children_map.get(&pid) {
            for child_pid in children {
                if visited.insert(*child_pid) {
                    queue.push_back(*child_pid);
                }
            }
        }
    }

    visited
        .into_iter()
        .filter(|pid| is_pid_alive(*pid))
        .collect()
}

/// Decides whether a process `cmdline` belongs to a specific game launch.
///
/// The game's executable basename (e.g. `client.exe`) must appear in the
/// cmdline. When the launch had distinguishing arguments (e.g. `user:des094`),
/// they must appear too — this separates several concurrent instances of the
/// same executable sharing one container. cmdline is the only readable identity
/// signal: environ is permission-denied for Wine processes here.
fn process_matches_cmdline(
    cmdline: &str,
    match_exe: Option<&str>,
    match_args: Option<&str>,
) -> bool {
    let Some(exe) = match_exe.filter(|e| !e.is_empty()) else {
        return false;
    };
    if !cmdline.contains(exe) {
        return false;
    }
    match match_args.filter(|a| !a.is_empty()) {
        Some(args) => cmdline.contains(args),
        None => true,
    }
}

fn collect_runtime_matched_pids(
    match_exe: Option<&str>,
    match_args: Option<&str>,
    cmdlines: &HashMap<u32, String>,
) -> HashSet<u32> {
    let mut matched = HashSet::new();

    for (pid, cmdline) in cmdlines {
        if process_matches_cmdline(cmdline, match_exe, match_args) {
            matched.insert(*pid);
        }
    }

    matched
}

/// Process shared by every game in a Wine prefix / pressure-vessel container
/// (one wineserver per prefix, one container supervisor). Killing one of these
/// tears down *all* co-tenants, so they are excluded from a session's "own"
/// process set: they never decide a single game's liveness and are never killed
/// while another game still shares the container.
fn is_shared_runtime_infra(comm: &str) -> bool {
    let lower = comm.to_ascii_lowercase();
    lower.contains("wineserver")
        || lower.contains("pressure-vessel")
        || lower.contains("bwrap")
        || lower.contains("umu-run")
        || lower == "umu"
        || lower.contains("steam-runtime")
}

fn is_shared_infra_pid(pid: u32) -> bool {
    read_process_comm(pid).is_some_and(|comm| is_shared_runtime_infra(&comm))
}

/// Token-authoritative discovery of a session's *own* live processes.
///
/// A game is "running" iff at least one process still carries its
/// `LEYEN_INSTANCE` token (or, for pre-token sessions, its GAMEID) — independent
/// of how Wine/pressure-vessel reparents it inside a shared container. The
/// process tree under the recorded roots is unioned in to catch payload helpers
/// that did not inherit the env, then shared container infra (wineserver,
/// pressure-vessel, …) is filtered out so a co-tenant keeping the container
/// alive never makes this session look running after its own game exited.
fn refresh_known_pids(
    session: &mut RunningGameSession,
    children_map: &HashMap<u32, Vec<u32>>,
    cmdlines: &HashMap<u32, String>,
) -> HashSet<u32> {
    let mut roots: HashSet<u32> = session.known_pids.iter().copied().collect();
    roots.insert(session.pid);

    let alive_roots: HashSet<u32> = roots.into_iter().filter(|pid| is_pid_alive(*pid)).collect();
    let mut discovered = collect_descendant_pids(&alive_roots, children_map);

    // Authoritative signal: processes whose cmdline matches this launch. They
    // survive the launcher exiting and reparenting into a shared container,
    // which the process tree cannot follow.
    let matched = collect_runtime_matched_pids(
        session.match_exe.as_deref(),
        session.match_args.as_deref(),
        cmdlines,
    );
    if !matched.is_empty() {
        discovered.extend(collect_descendant_pids(&matched, children_map));
    }

    // Drop shared container infra: it belongs to no single game.
    discovered.retain(|pid| !is_shared_infra_pid(*pid));

    let mut known_pids: Vec<u32> = discovered.iter().copied().collect();
    known_pids.sort_unstable();
    session.known_pids = known_pids;
    discovered
}

/// True when another registered session shares `target`'s Wine prefix and still
/// has live processes — i.e. stopping `target` must spare the shared container.
fn prefix_has_other_live_session(target: &RunningGameSession, scan: &ProcScan) -> bool {
    let Some(prefix) = target.match_prefix_path.as_deref() else {
        return false;
    };

    let others = with_running_registry(|registry| {
        let collected: Vec<RunningGameSession> = registry
            .sessions
            .iter()
            .filter(|session| {
                session.game_id != target.game_id
                    && session.match_prefix_path.as_deref() == Some(prefix)
            })
            .cloned()
            .collect();
        (collected, false)
    });

    let Ok(others) = others else {
        return false;
    };

    others
        .into_iter()
        .any(|mut session| !refresh_known_pids(&mut session, &scan.children, &scan.cmdlines).is_empty())
}

fn pipe_process_output<R>(reader: R, game_id: String, game_title: String, stream_name: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let reader = AsyncBufReader::new(reader);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                info!(
                    target: &format!("game:{}", game_id),
                    "[{}:{}] {}", game_title, stream_name, line
                );
            }
        }
    });
}

pub fn launch_game(game: &Game, overlay: &adw::ToastOverlay) {
    let game = game.clone();
    let overlay = overlay.clone();
    glib::spawn_future_local(async move {
        match launch_game_managed(&game, true, true, false).await {
            Ok(report) => {
                for notice in report.notices {
                    overlay.add_toast(adw::Toast::new(&notice));
                }
            }
            Err(err) => overlay.add_toast(adw::Toast::new(&err.to_string())),
        }
    });
}

pub async fn launch_game_headless(game: &Game) -> Result<LaunchReport, LaunchError> {
    launch_game_managed(game, true, true, false).await
}

fn spawn_detached_monitor(game_id: &str) {
    let Ok(current_exe) = std::env::current_exe() else {
        warn!(
            target: &format!("game:{game_id}"),
            "Failed to resolve the current executable for the runtime monitor"
        );
        return;
    };

    match StdCommand::new(&current_exe)
        .arg("internal-monitor")
        .arg(game_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            let pid = child.id();
            // Detach immediately — the monitor handles its own lifecycle.
            info!(
                target: &format!("game:{game_id}"),
                "Spawned detached runtime monitor (pid {pid})"
            );
        }
        Err(e) => {
            warn!(
                target: &format!("game:{game_id}"),
                "Failed to start the runtime monitor: {}", e
            );
        }
    }
}

async fn launch_game_managed(
    game: &Game,
    capture_output: bool,
    reap_child_locally: bool,
    spawn_background_monitor: bool,
) -> Result<LaunchReport, LaunchError> {
    let mut notices = Vec::new();

    // Block launch while umu-launcher is being downloaded.
    if UMU_DOWNLOADING.load(Ordering::Relaxed) {
        return Err(LaunchError::Other(
            t!("umu-launcher is still downloading, please wait…"),
        ));
    }

    // Block launch if umu-run is simply not available.
    if !tokio::task::spawn_blocking(is_umu_run_available)
        .await
        .unwrap_or_else(|e| {
            warn!("is_umu_run_available task failed: {e}");
            false
        })
    {
        return Err(LaunchError::Other(
            t!("umu-launcher is not installed. Please check your internet connection and restart."),
        ));
    }

    let settings = load_settings_with_auto_install(false).await;
    let library = load_library().await.map_err(LaunchError::Other)?;
    let parent_group = find_game_and_group(&library, &game.id).and_then(|(_, group)| group);
    let prefix_path = resolve_launch_prefix(game, parent_group, &settings.default_prefix_path);
    let launch_game_id = effective_game_id(game);

    if is_game_running(&game.id) {
        return Err(LaunchError::Other(
            t!("This game is already running"),
        ));
    }

    let mut env_vars: Vec<(String, String)> = Vec::new();
    if !prefix_path.is_empty() {
        env_vars.push(("WINEPREFIX".to_string(), prefix_path.clone()));
    }

    if !launch_game_id.is_empty() {
        env_vars.push(("GAMEID".to_string(), launch_game_id.clone()));
    }

    let proton_path = match resolve_launch_proton(game, parent_group, &settings.default_proton) {
        Some(path) => {
            if path.starts_with('/') {
                let p = path.clone();
                if !tokio::task::spawn_blocking(move || std::path::Path::new(&p).exists())
                    .await
                    .unwrap_or(true)
                {
                    error!(
                        target: &format!("game:{}", game.id),
                        "Proton path for '{}' does not exist: {}", game.title, path
                    );
                    return Err(LaunchError::Other(
                        t!("Selected Proton version was not found"),
                    ));
                }
            }
            env_vars.push(("PROTONPATH".to_string(), path.clone()));
            path
        }
        None => settings.default_proton.clone(),
    };

    if mangohud_available() && game.mangohud {
        env_vars.push(("MANGOHUD".to_string(), "1".to_string()));
    }

    env_vars.push((
        "PROTON_ENABLE_WAYLAND".to_string(),
        if game.wayland {
            "1".to_string()
        } else {
            "0".to_string()
        },
    ));

    env_vars.push((
        "PROTON_USE_WOW64".to_string(),
        if game.wow64 {
            "1".to_string()
        } else {
            "0".to_string()
        },
    ));

    let ntsync_val = if game.ntsync { "1" } else { "0" };
    env_vars.push(("PROTON_USE_NTSYNC".to_string(), ntsync_val.to_string()));
    env_vars.push(("WINENTSYNC".to_string(), ntsync_val.to_string()));

    if game.hdr {
        env_vars.push(("PROTON_ENABLE_HDR".to_string(), "1".to_string()));
        env_vars.push(("DXVK_HDR".to_string(), "1".to_string()));
    }

    if game.proton_log {
        env_vars.push(("PROTON_LOG".to_string(), "1".to_string()));
    }

    let umu = get_umu_run_path();
    let mut cmd_args: Vec<String> = Vec::new();

    if game.launch_args.contains("%command%") {
        let parts: Vec<&str> = game.launch_args.splitn(2, "%command%").collect();
        let postfix = split_shell_words(parts.get(1).unwrap_or(&""));

        let mut cmd_wrappers: Vec<String> = Vec::new();
        for token in split_shell_words(parts[0]) {
            if let Some((key, value)) = token.split_once('=')
                && is_valid_env_key(key)
            {
                env_vars.push((key.to_string(), value.to_string()));
                continue;
            }
            cmd_wrappers.push(token.to_string());
        }

        if gamemode_available() && game.gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.extend(cmd_wrappers);
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        cmd_args.extend(postfix);
    } else {
        if gamemode_available() && game.gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        if !game.launch_args.is_empty() {
            cmd_args.extend(split_shell_words(&game.launch_args));
        }
    }

    let exe_path_clone = game.exe_path.clone();
    let working_dir = tokio::task::spawn_blocking(move || working_directory_for(&exe_path_clone))
        .await
        .unwrap_or_default();
    match try_lock_prefix(&prefix_path).await {
        PrefixLockState::Available => {}
        PrefixLockState::Busy => {
            env_vars.push(("UMU_CONTAINER_NSENTER".to_string(), "1".to_string()));
            notices.push(
                t!("Prefix is already in use. Launching with shared-container fallback."),
            );
        }
        PrefixLockState::Unavailable => {}
    }

    let launch_summary = format!(
        "Launching '{}' | exe: {} | cwd: {} | prefix: {} | proton: {}",
        game.title,
        game.exe_path,
        working_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
        if prefix_path.is_empty() {
            "<none>".to_string()
        } else {
            prefix_path.clone()
        },
        if proton_path.is_empty() {
            "<default>".to_string()
        } else {
            proton_path.clone()
        },
    );
    info!(target: &format!("game:{}", game.id), "{}", launch_summary);
    let full_cmd = format!(
        "{} {}",
        env_vars
            .iter()
            .map(|(k, v)| format!(
                "{}={}",
                k,
                shlex::try_quote(v).unwrap_or(std::borrow::Cow::Borrowed(v))
            ))
            .collect::<Vec<_>>()
            .join(" "),
        cmd_args
            .iter()
            .map(|a| {
                shlex::try_quote(a)
                    .unwrap_or(std::borrow::Cow::Borrowed(a))
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    info!(target: &format!("game:{}", game.id), "Command: {}", full_cmd);

    // Spawn process in blocking thread — fork() blocks, don't stall GTK main loop
    let (mut child, child_pid, child_stdout, child_stderr) =
        tokio::task::spawn_blocking(move || {
            let mut cmd = tokio::process::Command::new(&cmd_args[0]);
            cmd.args(&cmd_args[1..]);
            cmd.stdin(Stdio::null());
            cmd.stdout(if capture_output {
                Stdio::piped()
            } else {
                Stdio::null()
            });
            cmd.stderr(if capture_output {
                Stdio::piped()
            } else {
                Stdio::null()
            });
            cmd.envs(env_vars.iter().map(|(k, v)| (k, v)));
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setpgid(0, 0) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
            if let Some(ref cwd) = working_dir {
                cmd.current_dir(cwd);
            }
            let mut child = cmd
                .spawn()
                .map_err(|e| LaunchError::Other(format!("Failed to launch: {}", e)))?;
            let pid = child
                .id()
                .ok_or_else(|| LaunchError::Other(t!("Failed to get child PID")))?;
            let child_stdout = child.stdout.take();
            let child_stderr = child.stderr.take();
            Ok::<_, LaunchError>((child, pid, child_stdout, child_stderr))
        })
        .await
        .map_err(|e| LaunchError::Other(join_err(e)))??;
    let started_at_epoch_seconds = current_epoch_seconds();
    // Identity for finding this game's processes via /proc/PID/cmdline (environ
    // is unreadable for Wine processes in the idmapped container): the executable
    // basename, plus the distinguishing launch arguments to separate concurrent
    // instances of the same executable.
    let match_exe = Path::new(&game.exe_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .filter(|name| !name.is_empty());
    let match_args = cmdline_arg_signature(&game.launch_args);
    let session = RunningGameSession {
        game_id: game.id.clone(),
        pid: child_pid,
        known_pids: vec![child_pid],
        started_at_epoch_seconds,
        match_prefix_path: (!prefix_path.is_empty()).then_some(prefix_path.clone()),
        match_game_id: (!launch_game_id.is_empty()).then_some(launch_game_id.clone()),
        match_exe,
        match_args,
        termination_requested: false,
    };

    if !try_register_running_session(session).await? {
        let _ = unsafe { libc::kill(-(child_pid as i32), libc::SIGKILL) };
        let _ = child.wait().await;
        return Err(LaunchError::Other(
            t!("This game is already running"),
        ));
    }

    // Reflect the new "running" state immediately instead of waiting for the
    // next background monitor tick.
    republish_running_sessions().await;

    if !record_game_launch_start(&game.id, started_at_epoch_seconds).await {
        warn!(target: &format!("game:{}", game.id), "Failed to record game launch start");
    }

    if let Some(stdout) = child_stdout {
        pipe_process_output(stdout, game.id.clone(), game.title.clone(), "stdout");
    }
    if let Some(stderr) = child_stderr {
        pipe_process_output(stderr, game.id.clone(), game.title.clone(), "stderr");
    }

    if spawn_background_monitor {
        spawn_detached_monitor(&game.id);
    }

    if reap_child_locally {
        let game_id_log = game.id.clone();
        let game_title_log = game.title.clone();
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".to_string());
                    info!(target: &format!("game:{}", game_id_log), "'{}' exited with status {}", game_title_log, code);
                }
                Err(e) => {
                    warn!(target: &format!("game:{}", game_id_log), "'{}' wait error: {}", game_title_log, e);
                }
            }
            // Game quit on its own — refresh state now so the card clears promptly
            // rather than on the next background monitor tick.
            republish_running_sessions().await;
        });
    }

    info!(
        target: &format!("game:{}", game.id),
        "Spawned '{}' with pid {}", game.title, child_pid
    );
    notices.push(t!("Launching {}...").replacen("{}", &game.title, 1));
    Ok(LaunchReport { notices })
}