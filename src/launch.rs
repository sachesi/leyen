use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    #[error("Failed to open runtime lock '{path}': {source}")]
    LockOpenError {
        path: PathBuf,
        source: std::io::Error,
    },
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
        .unwrap_or(0)
}

fn running_registry_path() -> PathBuf {
    get_config_dir().join("running.toml")
}

fn running_registry_lock_path() -> PathBuf {
    get_config_dir().join(".running.lock")
}

fn with_running_registry<R>(
    f: impl FnOnce(&mut RunningGamesRegistry) -> (R, bool),
) -> Result<R, LaunchError> {
    let lock_path = running_registry_lock_path();
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| LaunchError::LockOpenError {
            path: lock_path.clone(),
            source: e,
        })?;

    if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(LaunchError::LockAcquireError {
            path: lock_path,
            source: std::io::Error::last_os_error(),
        });
    }

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

    let _ = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN) };
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
        let children_map = build_process_children_map_cached();
        let all_envs = build_all_envs_map_cached();
        with_running_registry(|registry| {
            let original_sessions = registry.sessions.clone();
            let mut active_sessions = Vec::new();
            let mut finished_sessions = Vec::new();

            for mut session in registry.sessions.drain(..) {
                if refresh_known_pids(&mut session, &children_map, &all_envs).is_empty() {
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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

static RUNNING_SESSIONS_CACHE: OnceLock<RwLock<Vec<RunningGameSnapshot>>> = OnceLock::new();
static RUNNING_SESSIONS_VERSION_CACHE: AtomicU64 = AtomicU64::new(0);

fn get_running_sessions_cache() -> &'static RwLock<Vec<RunningGameSnapshot>> {
    RUNNING_SESSIONS_CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn start_running_sessions_monitor() {
    tokio::spawn(async move {
        let mut consecutive_errors: u32 = 0;
        loop {
            match synchronize_running_sessions().await {
                Ok(sessions) => {
                    let snapshots = running_sessions_to_snapshots(&sessions);
                    let version = running_sessions_version(&sessions);

                    if let Ok(mut cache) = get_running_sessions_cache().write() {
                        *cache = snapshots;
                    }
                    RUNNING_SESSIONS_VERSION_CACHE.store(version, Ordering::Relaxed);
                    consecutive_errors = 0;
                }
                Err(e) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    warn!("Session monitor sync failed (attempt {consecutive_errors}): {e}");
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
    get_running_sessions_cache()
        .read()
        .map(|c| !c.is_empty())
        .unwrap_or(false)
}

pub async fn running_games_version() -> u64 {
    RUNNING_SESSIONS_VERSION_CACHE.load(Ordering::Relaxed)
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
    

    tokio::task::spawn_blocking(move || {
        let children_map = build_process_children_map_cached();
        let all_envs = build_all_envs_map_cached();
        let mut session = session;
        let tracked_pids = refresh_known_pids(&mut session, &children_map, &all_envs);
        let mut killed_any = false;

        let pgid = -(session.pid as i32);
        let group_result = unsafe { libc::kill(pgid, libc::SIGKILL) };
        if group_result == 0 {
            info!(
                target: &format!("game:{}", session.game_id),
                "Sent SIGKILL to process group {}", session.pid
            );
            killed_any = true;
        }

        let group_error = std::io::Error::last_os_error();
        for tracked_pid in &tracked_pids {
            if unsafe { libc::kill(*tracked_pid as i32, libc::SIGKILL) } == 0 {
                killed_any = true;
            }
        }

        if killed_any {
            let _ = mark_running_session_termination_requested(&game_id_clone);
            info!(
                target: &format!("game:{}", game_id_clone),
                "Sent SIGKILL to tracked processes for root pid {}",
                session.pid
            );
            return Ok(true);
        }

        if tracked_pids.is_empty() {
            return Ok(true);
        }

        Err(LaunchError::Other(format!(
            "Failed to stop pid {}: {}",
            session.pid,
            group_error.to_string()
        )))
    })
    .await
    .map_err(|e| LaunchError::Other(join_err(e)))
    .and_then(|r| r)
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
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    let mut fields = after_name.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

fn read_process_env(pid: u32) -> Option<HashMap<String, String>> {
    let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let mut env = HashMap::new();

    for entry in bytes.split(|&byte| byte == 0) {
        if entry.is_empty() {
            continue;
        }

        let text = String::from_utf8_lossy(entry);
        if let Some((key, value)) = text.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        }
    }

    Some(env)
}

fn read_process_comm(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

type EnvsMapCache = RwLock<(HashMap<u32, HashMap<String, String>>, u64)>;

static PROCESS_ENVS_MAP_CACHE: OnceLock<EnvsMapCache> = OnceLock::new();

fn get_process_envs_cache() -> &'static EnvsMapCache {
    PROCESS_ENVS_MAP_CACHE.get_or_init(|| RwLock::new((HashMap::new(), 0)))
}

fn build_all_envs_map_cached() -> HashMap<u32, HashMap<String, String>> {
    let now = current_epoch_seconds();
    if let Ok(cache) = get_process_envs_cache().read()
        && now.saturating_sub(cache.1) < 1
    {
        return cache.0.clone();
    }
    let map = build_all_envs_map();
    if let Ok(mut cache) = get_process_envs_cache().write() {
        *cache = (map.clone(), now);
    }
    map
}

fn build_all_envs_map() -> HashMap<u32, HashMap<String, String>> {
    let mut map = HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return map;
    };

    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };

        if pid < 1000 {
            continue;
        }

        // Smarter filtering: only read environment for processes that could be part of a game
        if let Some(comm) = read_process_comm(pid) {
            let comm_lower = comm.to_lowercase();
            let looks_like_game = comm_lower.contains("wine")
                || comm_lower.contains("steam")
                || comm_lower.contains("proton")
                || comm_lower.ends_with(".exe")
                || comm_lower.contains("leyen");

            if looks_like_game
                && let Some(env) = read_process_env(pid)
                    && (env.contains_key("WINEPREFIX") || env.contains_key("GAMEID")) {
                        map.insert(pid, env);
                    }
        }
    }

    map
}

type ChildrenMapCache = RwLock<(HashMap<u32, Vec<u32>>, u64)>;

static PROCESS_CHILDREN_MAP_CACHE: OnceLock<ChildrenMapCache> = OnceLock::new();

fn get_process_children_cache() -> &'static ChildrenMapCache {
    PROCESS_CHILDREN_MAP_CACHE.get_or_init(|| RwLock::new((HashMap::new(), 0)))
}

fn build_process_children_map_cached() -> HashMap<u32, Vec<u32>> {
    let now = current_epoch_seconds();
    if let Ok(cache) = get_process_children_cache().read()
        && now.saturating_sub(cache.1) < 1 {
            return cache.0.clone();
        }
    let map = build_process_children_map();
    if let Ok(mut cache) = get_process_children_cache().write() {
        *cache = (map.clone(), now);
    }
    map
}

fn build_process_children_map() -> HashMap<u32, Vec<u32>> {
    let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return map;
    };

    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };
        if let Some(ppid) = read_parent_pid(pid) {
            map.entry(ppid).or_default().push(pid);
        }
    }

    map
}

fn is_pid_alive(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
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

fn process_matches_runtime(
    env: &HashMap<String, String>,
    match_prefix_path: Option<&str>,
    match_game_id: Option<&str>,
) -> bool {
    let env_game_id = env.get("GAMEID").map(String::as_str);
    let env_prefix = env.get("WINEPREFIX").map(String::as_str);

    if let Some(target_game_id) = match_game_id {
        if let Some(game_id) = env_game_id {
            return game_id == target_game_id;
        }
        if let Some(target_prefix) = match_prefix_path {
            return env_prefix == Some(target_prefix);
        }
        return false;
    }

    match_prefix_path.is_some_and(|target_prefix| env_prefix == Some(target_prefix))
}

fn collect_runtime_matched_pids(
    match_prefix_path: Option<&str>,
    match_game_id: Option<&str>,
    all_envs: &HashMap<u32, HashMap<String, String>>,
) -> HashSet<u32> {
    let mut matched = HashSet::new();

    for (pid, env) in all_envs {
        if process_matches_runtime(env, match_prefix_path, match_game_id) {
            matched.insert(*pid);
        }
    }

    matched
}

fn refresh_known_pids(
    session: &mut RunningGameSession,
    children_map: &HashMap<u32, Vec<u32>>,
    all_envs: &HashMap<u32, HashMap<String, String>>,
) -> HashSet<u32> {
    let mut roots: HashSet<u32> = session.known_pids.iter().copied().collect();
    roots.insert(session.pid);

    let alive_roots: HashSet<u32> = roots.into_iter().filter(|pid| is_pid_alive(*pid)).collect();
    let mut discovered = collect_descendant_pids(&alive_roots, children_map);

    if alive_roots.is_empty() {
        let matched = collect_runtime_matched_pids(
            session.match_prefix_path.as_deref(),
            session.match_game_id.as_deref(),
            all_envs,
        );
        discovered.extend(matched);

        if !discovered.is_empty() {
            discovered = collect_descendant_pids(&discovered, children_map);
        }
    }

    let mut known_pids: Vec<u32> = discovered.iter().copied().collect();
    known_pids.sort_unstable();
    session.known_pids = known_pids;
    discovered
}

fn pipe_process_output<R>(reader: R, game_id: String, game_title: String, stream_name: &'static str)
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    info!(
                        target: &format!("game:{}", game_id),
                        "[{}:{}] {}", game_title, stream_name, line
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        target: &format!("game:{}", game_id),
                        "Failed to read {} output for '{}': {}",
                        stream_name, game_title, e
                    );
                    break;
                }
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

    if let Err(e) = Command::new(current_exe)
        .arg("internal-monitor")
        .arg(game_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        warn!(
            target: &format!("game:{game_id}"),
            "Failed to start the runtime monitor: {}", e
        );
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
            "umu-launcher is still downloading, please wait…".to_string(),
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
            "umu-launcher is not installed. Please check your internet connection and restart."
                .to_string(),
        ));
    }

    let settings = load_settings_with_auto_install(false).await;
    let library = load_library().await;
    let parent_group = find_game_and_group(&library, &game.id).and_then(|(_, group)| group);
    let prefix_path = resolve_launch_prefix(game, parent_group, &settings.default_prefix_path);
    let launch_game_id = effective_game_id(game);

    if is_game_running(&game.id) {
        return Err(LaunchError::Other(
            "This game is already running".to_string(),
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
                        "Selected Proton version was not found".to_string(),
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
                "Prefix is already in use. Launching with shared-container fallback.".to_string(),
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

    let mut command = Command::new(&cmd_args[0]);
    command.args(&cmd_args[1..]);
    command.stdin(Stdio::null());
    command.stdout(if capture_output {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stderr(if capture_output {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.envs(env_vars.iter().map(|(k, v)| (k, v)));
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    if let Some(cwd) = &working_dir {
        command.current_dir(cwd);
    }

    match command.spawn() {
        Ok(mut child) => {
            let child_pid = child.id();
            let started_at_epoch_seconds = current_epoch_seconds();
            let session = RunningGameSession {
                game_id: game.id.clone(),
                pid: child_pid,
                known_pids: vec![child_pid],
                started_at_epoch_seconds,
                match_prefix_path: (!prefix_path.is_empty()).then_some(prefix_path.clone()),
                match_game_id: (!launch_game_id.is_empty()).then_some(launch_game_id.clone()),
                termination_requested: false,
            };

            if !try_register_running_session(session).await? {
                let _ = unsafe { libc::kill(-(child_pid as i32), libc::SIGKILL) };
                let _ = child.wait();
                return Err(LaunchError::Other(
                    "This game is already running".to_string(),
                ));
            }

            if !record_game_launch_start(&game.id, started_at_epoch_seconds).await {
                warn!(target: &format!("game:{}", game.id), "Failed to record game launch start");
            }

            if capture_output && let Some(stdout) = child.stdout.take() {
                pipe_process_output(stdout, game.id.clone(), game.title.clone(), "stdout");
            }
            if capture_output && let Some(stderr) = child.stderr.take() {
                pipe_process_output(stderr, game.id.clone(), game.title.clone(), "stderr");
            }

            if spawn_background_monitor {
                spawn_detached_monitor(&game.id);
            }

            if reap_child_locally {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }

            info!(
                target: &format!("game:{}", game.id),
                "Spawned '{}' with pid {}", game.title, child_pid
            );
            notices.push(format!("Launching {}...", game.title));
            Ok(LaunchReport { notices })
        }
        Err(e) => {
            error!("Failed to launch '{}': {}", game.title, e);
            Err(LaunchError::Other(format!("Failed to launch: {}", e)))
        }
    }
}
