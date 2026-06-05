use leyen_model::t;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, BufReader as AsyncBufReader};


use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{
    add_game_playtime, load_library, load_settings_with_auto_install, record_game_launch_result,
    record_game_launch_start,
};
use crate::runtime::proton::resolve_proton_path;
use crate::runtime::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};
use crate::tools::{gamemode_available, join_err, mangohud_available};
use leyen_model::library::{effective_game_id, find_game_and_group};
use leyen_model::models::{Game, GameGroup};
use leyen_model::paths::get_config_dir;

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
    /// PID of the `systemd-run --scope` leader. Informational (display only); the
    /// authoritative process set is the scope's cgroup, never this single PID.
    pid: u32,
    /// Transient systemd user scope that holds the entire game process tree
    /// (e.g. `leyen-ly-1234-1700000000.scope`). This is the identity and the kill
    /// handle: cgroup membership is inherited across `setsid`, PID namespaces and
    /// pressure-vessel reparenting, so it cannot be escaped like a process group.
    unit: String,
    /// Absolute path to the scope's cgroup directory, resolved once from the
    /// unit's `ControlGroup` property and cached. `cgroup.events`/`cgroup.procs`/
    /// `cgroup.kill` are read from here. Stable for the unit's lifetime (unit
    /// names are unique per launch), so persisting it avoids re-querying systemd
    /// on every monitor tick.
    #[serde(default)]
    cgroup_dir: Option<String>,
    /// Live PID count from the last cgroup read. Display only; excluded from the
    /// state version hash so per-tick flutter does not rebuild the library view.
    #[serde(default)]
    tracked_pid_count: usize,
    started_at_epoch_seconds: u64,
    match_prefix_path: Option<String>,
    /// Lowercased basename of the game executable (e.g. `client.exe`). With
    /// `match_args`, identifies this game's real process by `/proc/PID/cmdline`
    /// when it runs inside a *shared* pressure-vessel container (siblings launched
    /// with `UMU_CONTAINER_NSENTER`): the real process then lives in the first
    /// game's scope cgroup, not this session's own scope, so the scope alone
    /// cannot tell siblings apart.
    #[serde(default)]
    match_exe: Option<String>,
    /// Lowercased launch-argument signature (e.g. `user:ply094 role:plymouth`)
    /// that distinguishes concurrent instances of the same executable sharing a
    /// container.
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

/// Grace period after launch during which a session with an empty/unqueryable
/// scope is still treated as running. Covers the brief window between spawning
/// `systemd-run` and the transient scope becoming visible to the user manager.
const LAUNCH_GRACE_SECONDS: u64 = 8;

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
            tracked_pid_count: session.tracked_pid_count,
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
    // transitions. Deliberately exclude `tracked_pid_count`: Wine/Proton spawn and
    // reap helper processes constantly, so the scope's pid count flutters on every
    // monitor tick. Hashing it bumped the version each second while a game ran,
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
        let now = current_epoch_seconds();

        // Phase 1: snapshot the sessions under the lock (fast), then release it.
        let mut sessions = with_running_registry(|registry| (registry.sessions.clone(), false))?;

        // Phase 2: scan WITHOUT holding the lock. The cgroup/cmdline/systemctl reads
        // can be slow (a fresh pressure-vessel container has ~100 starting PIDs);
        // doing them while holding the registry flock serialized every launch/stop
        // behind one slow sync and cascaded into UI stalls.
        let universe = leyen_pid_cmdlines(&mut sessions);
        let mut finished_sessions = Vec::new();
        let mut finished_keys: std::collections::HashSet<(String, u64)> = std::collections::HashSet::new();
        let mut updates: HashMap<(String, u64), (Option<String>, usize)> = HashMap::new();
        for mut session in sessions {
            let alive = session_is_live(&mut session, &universe);
            let key = (session.game_id.clone(), session.started_at_epoch_seconds);
            if alive || now.saturating_sub(session.started_at_epoch_seconds) < LAUNCH_GRACE_SECONDS {
                updates.insert(key, (session.cgroup_dir.clone(), session.tracked_pid_count));
            } else {
                finished_keys.insert(key);
                finished_sessions.push(session);
            }
        }

        // Phase 3: apply the result under the lock (fast). Match by (game_id,
        // started_at) so a session relaunched during the unlocked scan — a new
        // instance with a fresh start time — is never wrongly removed or updated.
        let active_sessions = with_running_registry(|registry| {
            let before = registry.sessions.clone();
            registry
                .sessions
                .retain(|s| !finished_keys.contains(&(s.game_id.clone(), s.started_at_epoch_seconds)));
            for s in registry.sessions.iter_mut() {
                if let Some((dir, count)) = updates.get(&(s.game_id.clone(), s.started_at_epoch_seconds)) {
                    if s.cgroup_dir.is_none() {
                        s.cgroup_dir = dir.clone();
                    }
                    s.tracked_pid_count = *count;
                }
            }
            let dirty = registry.sessions != before;
            (registry.sessions.clone(), dirty)
        })?;

        Ok::<_, LaunchError>((active_sessions, finished_sessions))
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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};

static RUNNING_SESSIONS_CACHE: OnceLock<RwLock<Vec<RunningGameSnapshot>>> = OnceLock::new();
static RUNNING_SESSIONS_VERSION_CACHE: AtomicU64 = AtomicU64::new(0);
/// Lock-free mirror of "is any game running", kept in sync with the snapshot
/// cache by the monitor. Lets the GTK main thread (per-second timer, window
/// close handler) answer without taking the `RwLock`.
static ANY_GAME_RUNNING: AtomicBool = AtomicBool::new(false);

/// Notified (on the tokio runtime) whenever running-session state is
/// republished. The daemon installs an emitter here to drive `SessionsChanged`;
/// the listener receives the fresh snapshot set so it can be sent over D-Bus
/// without re-reading. Replaces the in-process async-channel wake of the old
/// single-binary GUI.
static SESSIONS_LISTENER: OnceLock<Box<dyn Fn(Vec<RunningGameSnapshot>) + Send + Sync>> =
    OnceLock::new();

/// Installs the publish listener. No-op if called more than once.
pub fn set_sessions_listener(listener: impl Fn(Vec<RunningGameSnapshot>) + Send + Sync + 'static) {
    let _ = SESSIONS_LISTENER.set(Box::new(listener));
}

/// Single path that makes a new session set visible: refreshes the snapshot
/// cache, the `any running` mirror and the version atomic, then notifies the
/// listener. Called by the monitor and by the launch/stop/exit fast paths.
fn publish_sessions(sessions: &[RunningGameSession]) {
    let snapshots = running_sessions_to_snapshots(sessions);
    let version = running_sessions_version(sessions);
    let any_running = !snapshots.is_empty();

    if let Ok(mut cache) = get_running_sessions_cache().write() {
        *cache = snapshots.clone();
    }
    ANY_GAME_RUNNING.store(any_running, Ordering::Relaxed);
    // Release pairs with the Acquire load in running_games_version so a reader
    // seeing the new version also sees the new snapshot.
    RUNNING_SESSIONS_VERSION_CACHE.store(version, Ordering::Release);

    if let Some(listener) = SESSIONS_LISTENER.get() {
        listener(snapshots);
    }
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

/// Reconciles the persisted registry against live systemd scopes at startup.
///
/// A scope outlives the Leyen process that created it, so a game can still be
/// running after a crash — those sessions are kept and re-adopted by the monitor.
/// Sessions whose scope is gone are leftovers from a crashed run; their true
/// playtime is unknowable (the scope and its timestamps are collected), so they
/// are dropped WITHOUT recording playtime rather than finalized with an inflated
/// `now - started_at` elapsed. Run once before the monitor starts.
pub async fn reconcile_stale_sessions_on_startup() {
    let dropped = tokio::task::spawn_blocking(|| {
        with_running_registry(|registry| {
            let before = registry.sessions.len();
            let mut kept = Vec::new();
            let mut dropped: Vec<String> = Vec::new();
            let mut sessions = std::mem::take(&mut registry.sessions);
            let universe = leyen_pid_cmdlines(&mut sessions);
            for mut session in sessions {
                if session_is_live(&mut session, &universe) {
                    kept.push(session);
                } else {
                    dropped.push(session.game_id.clone());
                }
            }
            let dirty = kept.len() != before;
            registry.sessions = kept;
            (dropped, dirty)
        })
    })
    .await;

    match dropped {
        Ok(Ok(dropped)) => {
            for game_id in dropped {
                warn!(
                    target: &format!("game:{game_id}"),
                    "Dropping stale running session with no live scope (playtime not recorded)"
                );
            }
        }
        Ok(Err(e)) => warn!("Startup session reconciliation failed: {e}"),
        Err(e) => warn!("Startup session reconciliation task failed: {e}"),
    }
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

pub async fn stop_game(game_id: &str) -> Result<bool, LaunchError> {
    let Some(session) = find_running_session(game_id).await? else {
        return Ok(false);
    };

    let game_id_clone = game_id.to_string();

    let result = tokio::task::spawn_blocking(move || {
        let target = session;
        let unit = target.unit.clone();

        // All registered sessions, to build the PID universe (the target's real
        // process may live in another game's shared-container scope) and to detect
        // co-tenants of the same wineprefix.
        let mut all = with_running_registry(|registry| (registry.sessions.clone(), false))?;
        let universe = leyen_pid_cmdlines(&mut all);
        let mut matched = session_matched_pids(&target, &universe);

        // Already gone: no real process for this game and its launcher scope is
        // empty too.
        let mut probe = target.clone();
        if matched.is_empty() && !scope_alive(&mut probe) {
            return Ok(true);
        }

        // When another game shares this wineprefix and is still live, the
        // container (wineserver / pressure-vessel) must survive — signal only this
        // game's own PIDs. Sole occupant → also tear the scope down for a clean
        // container shutdown.
        let shared = prefix_shared_with_other_live(&target, &all, &universe);

        let _ = mark_running_session_termination_requested(&game_id_clone);

        // Graceful first: SIGTERM the game's real processes (wherever they run) and
        // its own launcher scope so Wine/Proton can flush and save state.
        let signaled = signal_pids(&matched, libc::SIGTERM)
            | systemctl(&["kill", "--signal=SIGTERM", &unit]);
        info!(
            target: &format!("game:{}", game_id_clone),
            "Sent SIGTERM to {} process(es) of {}", matched.len(), unit
        );

        // Wait up to ~3s for a clean exit before escalating, re-matching the game's
        // processes each poll against a fresh cgroup read.
        matched = wait_for_session_pids(&target, &mut all, 15);

        if !matched.is_empty() {
            let forced = signal_pids(&matched, libc::SIGKILL);
            // Sole occupant: tear the scope down atomically for a clean container
            // shutdown. Shared: leave it — a co-tenant needs the container.
            if !shared {
                kill_scope_forcibly(&target);
            }
            info!(
                target: &format!("game:{}", game_id_clone),
                "Escalated to SIGKILL for {} process(es) of {}", matched.len(), unit
            );
            matched = wait_for_session_pids(&target, &mut all, 25);

            if !signaled && !forced {
                return Err(LaunchError::Other(format!(
                    "Failed to signal any process of {}", unit
                )));
            }
        } else if !shared {
            // Game already exited but its launcher scope / container infra may
            // linger; tear it down so the container winds down promptly.
            kill_scope_forcibly(&target);
        }

        if !matched.is_empty() {
            warn!(
                target: &format!("game:{}", game_id_clone),
                "{} process(es) of {} still alive after stop (likely uninterruptible); \
                 will clear once the kernel reaps them",
                matched.len(), unit
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

/// Hard cap on every `systemctl --user` invocation. The user manager can stall
/// for seconds while pressure-vessel containers churn; without a cap a stalled
/// query blocks its `spawn_blocking` thread indefinitely and, repeated, exhausts
/// the blocking pool so every game action hangs. A timed-out call is killed and
/// reported as failure — the periodic monitor retries.
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(3);

/// Waits for `child` up to `timeout`, killing it (and reaping) on expiry.
/// Returns the exit status, or `None` if it had to be killed or wait failed.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Runs `systemctl --user <args>` with a timeout and reports success. Output is
/// discarded; callers only need the status.
fn systemctl(args: &[&str]) -> bool {
    let Ok(mut child) = StdCommand::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    matches!(wait_with_timeout(&mut child, SYSTEMCTL_TIMEOUT), Some(s) if s.success())
}

/// Returns the value of a single systemd property for `unit`, or `None` if the
/// unit is gone / the query fails / it timed out. Used to resolve a scope's
/// `ControlGroup`. Output is tiny so the pipe never deadlocks the poll loop.
fn systemctl_show_property(unit: &str, property: &str) -> Option<String> {
    let mut child = StdCommand::new("systemctl")
        .arg("--user")
        .arg("show")
        .arg(unit)
        .arg(format!("--property={property}"))
        .arg("--value")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = wait_with_timeout(&mut child, SYSTEMCTL_TIMEOUT)?;
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let value = out.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Cached result of the systemd user-manager probe: 0 = unknown, 1 = available,
/// 2 = unavailable. Probing on every launch is itself a `systemctl` call that can
/// stall, so it is resolved once.
static SYSTEMD_AVAILABLE: AtomicU8 = AtomicU8::new(0);

/// True when a usable systemd user manager is reachable — required for the
/// transient-scope launch backend. Cached after the first successful probe.
fn systemd_user_available() -> bool {
    match SYSTEMD_AVAILABLE.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let available = systemctl(&["show", "--property=Version", "--value"]);
    SYSTEMD_AVAILABLE.store(if available { 1 } else { 2 }, Ordering::Relaxed);
    available
}

/// Sanitizes a string into a valid systemd unit-name component: systemd only
/// accepts `[A-Za-z0-9:_.-]`, so every other byte becomes `-`.
fn sanitize_unit_component(input: &str) -> String {
    let mapped: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if mapped.is_empty() {
        "x".to_string()
    } else {
        mapped
    }
}

/// Builds the transient scope unit name for a launch. The epoch + a monotonic
/// counter make it unique even if the same game is relaunched the same second.
fn scope_unit_name(game_id: &str, epoch: u64) -> String {
    static SCOPE_NONCE: AtomicU64 = AtomicU64::new(0);
    let nonce = SCOPE_NONCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "leyen-{}-{}-{}.scope",
        sanitize_unit_component(game_id),
        epoch,
        nonce
    )
}

/// Resolves the absolute path to a scope's cgroup directory, caching it on the
/// session. The `ControlGroup` property is the cgroup path relative to the v2
/// mount; it is stable for the unit's lifetime.
fn resolve_cgroup_dir(session: &mut RunningGameSession) -> Option<String> {
    if let Some(dir) = &session.cgroup_dir {
        return Some(dir.clone());
    }
    let control_group = systemctl_show_property(&session.unit, "ControlGroup")?;
    let dir = format!("/sys/fs/cgroup{control_group}");
    session.cgroup_dir = Some(dir.clone());
    Some(dir)
}

/// Parses a cgroup `cgroup.procs` file (one PID per line) into a PID list.
fn read_cgroup_pids(path: &Path) -> Vec<u32> {
    match fs::read_to_string(path) {
        Ok(data) => data.lines().filter_map(|line| line.trim().parse().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Reads the `populated` flag from a cgroup's `cgroup.events`. Unlike
/// `cgroup.procs` (direct members only), this reflects the **whole subtree**, so
/// it stays correct even if the game tree creates nested cgroups under the scope.
/// `None` means the file is gone (scope collected) or unreadable.
fn read_cgroup_populated(dir: &Path) -> Option<bool> {
    let data = fs::read_to_string(dir.join("cgroup.events")).ok()?;
    data.lines()
        .find_map(|line| line.strip_prefix("populated "))
        .map(|value| value.trim() == "1")
}

/// Authoritative liveness for a session: whether its scope's cgroup subtree still
/// holds any process. Immune to reparenting, `setsid` and PID namespaces —
/// membership is a kernel property the tree cannot escape. Falls back to an
/// `is-active` probe only when the cgroup is unresolvable / already collected.
fn scope_alive(session: &mut RunningGameSession) -> bool {
    match resolve_cgroup_dir(session) {
        Some(dir) => match read_cgroup_populated(Path::new(&dir)) {
            Some(populated) => populated,
            None => systemctl(&["is-active", "--quiet", &session.unit]),
        },
        None => systemctl(&["is-active", "--quiet", &session.unit]),
    }
}

/// Live PID set from the scope's `cgroup.procs` (direct members). Used only for
/// the cosmetic `tracked_pid_count`; liveness decisions use [`scope_alive`].
fn scope_pids(session: &mut RunningGameSession) -> Vec<u32> {
    match resolve_cgroup_dir(session) {
        Some(dir) => read_cgroup_pids(&Path::new(&dir).join("cgroup.procs")),
        None => Vec::new(),
    }
}


/// Polls until the target session's real (cmdline-matched) processes are gone or
/// `attempts * 200ms` elapse, re-reading the cgroup universe each poll. Returns
/// the survivors still present at the end.
fn wait_for_session_pids(
    target: &RunningGameSession,
    all: &mut [RunningGameSession],
    attempts: u32,
) -> Vec<u32> {
    let mut remaining = attempts;
    loop {
        let universe = leyen_pid_cmdlines(all);
        let alive = session_matched_pids(target, &universe);
        if alive.is_empty() || remaining == 0 {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(200));
        remaining -= 1;
    }
}

/// Force-kills every process in the scope atomically. Prefers writing `1` to the
/// cgroup's `cgroup.kill` (kernel ≥5.14, immune to re-forking children); falls
/// back to `systemctl kill --signal=SIGKILL`. Returns true if either succeeded.
fn kill_scope_forcibly(session: &RunningGameSession) -> bool {
    if let Some(dir) = &session.cgroup_dir
        && fs::write(Path::new(dir).join("cgroup.kill"), "1").is_ok()
    {
        return true;
    }
    systemctl(&["kill", "--signal=SIGKILL", &session.unit])
}

/// Reads `/proc/PID/cmdline` on a throwaway thread, bounded to 200ms. A process
/// stuck in uninterruptible sleep inside a crashed pressure-vessel container makes
/// a plain read block forever; doing that inline on the `synchronize` task would
/// leak its tokio blocking-pool thread, and enough leaks exhaust the pool so every
/// launch/stop hangs. Here only a detached thread leaks (bounded by distinct stuck
/// PIDs, since the caller reads each PID at most once via its cache).
fn read_cmdline_timeout(pid: u32) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read_process_cmdline_blocking(pid));
    });
    rx.recv_timeout(Duration::from_millis(200)).ok().flatten()
}

/// Blocking read of `/proc/PID/cmdline` → space-joined, lowercased, collapsed.
fn read_process_cmdline_blocking(pid: u32) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if raw.is_empty() {
        return None;
    }
    let joined: Vec<u8> = raw.into_iter().map(|b| if b == 0 { b' ' } else { b }).collect();
    let text = String::from_utf8_lossy(&joined).to_lowercase();
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}


/// Normalized, lowercased signature of a game's launch arguments. Drops the
/// `%command%` wrapper prefix (only the trailing real args reach the game's
/// cmdline).
fn cmdline_arg_signature(launch_args: &str) -> Option<String> {
    let tail = match launch_args.split_once("%command%") {
        Some((_, after)) => after,
        None => launch_args,
    };
    let normalized = tail.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

/// A process cmdline belongs to a game launch when it contains the executable
/// basename and (if recorded) the distinguishing launch-argument signature.
fn process_matches_cmdline(cmdline: &str, match_exe: Option<&str>, match_args: Option<&str>) -> bool {
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

/// The leyen-managed PID universe: every PID found in any registered session's
/// scope cgroup, mapped to its cmdline. This spans the shared container's scope,
/// so a sibling's real `client.exe` (running in the first game's scope) is
/// included and can be attributed back to its session by cmdline. Bounded to
/// leyen cgroups — never a whole-`/proc` scan, never a false positive outside.
fn leyen_pid_cmdlines(sessions: &mut [RunningGameSession]) -> HashMap<u32, String> {
    // Per-PID cmdline cache. A process's cmdline is fixed for its lifetime, so
    // each PID is read at most once instead of on every sync. Critically, a PID is
    // recorded as `None` BEFORE the inline read and only upgraded to its value on
    // success — so a process stuck in uninterruptible sleep inside a broken
    // pressure-vessel container costs at most ONE blocked thread, once, and is
    // never read again. No per-PID watchdog threads: spawning ~100 of them per
    // fresh container hammered the OS thread limit until tokio's blocking pool
    // could no longer spawn workers and every game action stalled.
    static CACHE: OnceLock<std::sync::Mutex<HashMap<u32, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));

    let mut pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for session in sessions.iter_mut() {
        if let Some(dir) = resolve_cgroup_dir(session) {
            for pid in read_cgroup_pids(&Path::new(&dir).join("cgroup.procs")) {
                pids.insert(pid);
            }
        }
    }

    // PIDs not yet cached: pessimistically mark them `None` first, so if the read
    // below hangs the PID is never retried.
    let to_read: Vec<u32> = {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        let fresh: Vec<u32> = pids.iter().copied().filter(|pid| !guard.contains_key(pid)).collect();
        for pid in &fresh {
            guard.insert(*pid, None);
        }
        fresh
    };
    for pid in to_read {
        if let Some(cmdline) = read_cmdline_timeout(pid) {
            cache.lock().unwrap_or_else(|e| e.into_inner()).insert(pid, Some(cmdline));
        }
    }

    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    // Drop entries for PIDs that have left every leyen cgroup, bounding the cache
    // and preventing stale attribution after PID reuse.
    guard.retain(|pid, _| pids.contains(pid));
    let mut map = HashMap::new();
    for pid in &pids {
        if let Some(Some(cmdline)) = guard.get(pid) {
            map.insert(*pid, cmdline.clone());
        }
    }
    map
}

/// PIDs in `universe` that belong to `session` by cmdline signature — the game's
/// real processes, wherever they actually run (own scope or a shared one).
fn session_matched_pids(session: &RunningGameSession, universe: &HashMap<u32, String>) -> Vec<u32> {
    universe
        .iter()
        .filter(|(_, cmdline)| {
            process_matches_cmdline(cmdline, session.match_exe.as_deref(), session.match_args.as_deref())
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// Liveness for a session, updating its cosmetic `tracked_pid_count`. When a
/// cmdline signature is recorded the game's real processes are matched in the
/// universe (correct even inside a shared container); otherwise it falls back to
/// the session's own scope cgroup.
fn session_is_live(session: &mut RunningGameSession, universe: &HashMap<u32, String>) -> bool {
    if session.match_exe.is_some() {
        let matched = session_matched_pids(session, universe);
        session.tracked_pid_count = matched.len();
        !matched.is_empty()
    } else {
        let alive = scope_alive(session);
        session.tracked_pid_count = scope_pids(session).len();
        alive
    }
}

/// True when another registered session shares this session's wineprefix and
/// still has a live process — i.e. tearing down the shared container would kill a
/// co-tenant, so the stop must signal only this game's own PIDs.
fn prefix_shared_with_other_live(
    session: &RunningGameSession,
    others: &[RunningGameSession],
    universe: &HashMap<u32, String>,
) -> bool {
    let Some(prefix) = session.match_prefix_path.as_deref() else {
        return false;
    };
    others.iter().any(|other| {
        other.game_id != session.game_id
            && other.match_prefix_path.as_deref() == Some(prefix)
            && !session_matched_pids(other, universe).is_empty()
    })
}

/// Sends `signal` to each PID. Returns true if at least one `kill` was accepted.
fn signal_pids(pids: &[u32], signal: libc::c_int) -> bool {
    let mut signaled = false;
    for &pid in pids {
        if unsafe { libc::kill(pid as i32, signal) } == 0 {
            signaled = true;
        }
    }
    signaled
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

pub async fn launch_game_headless(game: &Game) -> Result<LaunchReport, LaunchError> {
    launch_game_managed(game, true, true).await
}

async fn launch_game_managed(
    game: &Game,
    capture_output: bool,
    reap_child_locally: bool,
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

    // The launch runs inside a transient systemd user scope; without a reachable
    // user manager there is no way to track or stop the game tree reliably.
    if !tokio::task::spawn_blocking(systemd_user_available)
        .await
        .unwrap_or(false)
    {
        return Err(LaunchError::Other(
            t!("A systemd user session is required to launch games."),
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
    let allow_shared_container = settings.use_shared_container;
    match try_lock_prefix(&prefix_path).await {
        PrefixLockState::Available => {}
        PrefixLockState::Busy if allow_shared_container => {
            env_vars.push(("UMU_CONTAINER_NSENTER".to_string(), "1".to_string()));
            notices.push(
                t!("Prefix is already in use. Launching with shared-container fallback."),
            );
        }
        // Shared container disabled for this group: launch in its own container
        // on the same prefix instead of joining the running one (no NSENTER).
        PrefixLockState::Busy => {}
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

    let started_at_epoch_seconds = current_epoch_seconds();
    let scope_unit = scope_unit_name(&game.id, started_at_epoch_seconds);

    // Wrap the launch in a transient systemd user scope. Every process the game
    // tree forks is inherited into this scope's cgroup — across setsid, PID
    // namespaces and pressure-vessel reparenting — giving an authoritative,
    // escape-proof handle for tracking liveness and tearing the game down. The
    // command keeps running as a foreground child of systemd-run, so stdio
    // piping and child reaping behave exactly as a bare spawn would.
    let mut scoped_args: Vec<String> = vec![
        "systemd-run".to_string(),
        "--user".to_string(),
        "--scope".to_string(),
        "--quiet".to_string(),
        "--collect".to_string(),
        format!("--unit={scope_unit}"),
        "--".to_string(),
    ];
    scoped_args.extend(cmd_args);
    let cmd_args = scoped_args;

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
    let match_exe = Path::new(&game.exe_path)
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .filter(|name| !name.is_empty());
    let match_args = cmdline_arg_signature(&game.launch_args);
    let session = RunningGameSession {
        game_id: game.id.clone(),
        pid: child_pid,
        unit: scope_unit.clone(),
        cgroup_dir: None,
        tracked_pid_count: 1,
        started_at_epoch_seconds,
        match_prefix_path: (!prefix_path.is_empty()).then_some(prefix_path.clone()),
        match_exe,
        match_args,
        termination_requested: false,
    };

    let registered = try_register_running_session(session).await?;
    if !registered {
        let unit = scope_unit.clone();
        let _ = tokio::task::spawn_blocking(move || systemctl(&["stop", &unit])).await;
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