use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use libadwaita as adw;
use serde::{Deserialize, Serialize};

use crate::config::{
    add_game_playtime, effective_game_id, find_game_and_group, get_config_dir, load_library,
    load_settings_with_auto_install, record_game_launch_result, record_game_launch_start,
};
use crate::logging::{leyen_game_log, leyen_log};
use crate::models::{Game, GameGroup};
use crate::proton::resolve_proton_path;
use crate::tools::{gamemode_available, mangohud_available};
use crate::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};

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
) -> Result<R, String> {
    let lock_path = running_registry_lock_path();
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to prepare runtime lock directory: {}", e))?;
    }

    let lock_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "Failed to open runtime lock '{}': {}",
                lock_path.display(),
                e
            )
        })?;

    if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!(
            "Failed to lock runtime state '{}': {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        ));
    }

    let registry_path = running_registry_path();
    let mut registry = fs::read_to_string(&registry_path)
        .ok()
        .and_then(|data| toml::from_str::<RunningGamesRegistry>(&data).ok())
        .unwrap_or_default();

    let (result, dirty) = f(&mut registry);

    if dirty {
        let data = toml::to_string_pretty(&registry)
            .map_err(|e| format!("Failed to serialize running games state: {}", e))?;
        fs::write(&registry_path, data).map_err(|e| {
            format!(
                "Failed to write running games state '{}': {}",
                registry_path.display(),
                e
            )
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
        session.known_pids.hash(&mut hasher);
        session.started_at_epoch_seconds.hash(&mut hasher);
        session.termination_requested.hash(&mut hasher);
    }

    hasher.finish().max(1)
}

fn finalize_finished_session(session: &RunningGameSession) {
    let elapsed_seconds = current_epoch_seconds().saturating_sub(session.started_at_epoch_seconds);
    let status = if session.termination_requested {
        "Last run: stopped"
    } else {
        "Last run: completed"
    };

    let total_playtime = add_game_playtime(&session.game_id, elapsed_seconds);
    let _ = record_game_launch_result(&session.game_id, elapsed_seconds, status);

    leyen_game_log(
        &session.game_id,
        "INFO ",
        &format!(
            "Managed session finished after {}s ({})",
            elapsed_seconds, status
        ),
    );

    if let Some(total) = total_playtime {
        leyen_game_log(
            &session.game_id,
            "INFO ",
            &format!("Total recorded playtime is now {}s", total),
        );
    }
}

fn synchronize_running_sessions() -> Result<Vec<RunningGameSession>, String> {
    let (active_sessions, finished_sessions) = with_running_registry(|registry| {
        let original_sessions = registry.sessions.clone();
        let mut active_sessions = Vec::new();
        let mut finished_sessions = Vec::new();

        for mut session in registry.sessions.drain(..) {
            if refresh_known_pids(&mut session).is_empty() {
                finished_sessions.push(session);
            } else {
                active_sessions.push(session);
            }
        }

        let dirty = active_sessions != original_sessions;
        registry.sessions = active_sessions.clone();
        ((active_sessions, finished_sessions), dirty)
    })?;

    for session in &finished_sessions {
        finalize_finished_session(session);
    }

    Ok(active_sessions)
}

fn try_register_running_session(session: RunningGameSession) -> Result<bool, String> {
    let _ = synchronize_running_sessions();
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
}

fn mark_running_session_termination_requested(game_id: &str) -> Result<bool, String> {
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

fn find_running_session(game_id: &str) -> Result<Option<RunningGameSession>, String> {
    Ok(synchronize_running_sessions()?
        .into_iter()
        .find(|session| session.game_id == game_id))
}

pub fn is_game_running(game_id: &str) -> bool {
    find_running_session(game_id).ok().flatten().is_some()
}

pub fn running_games_version() -> u64 {
    synchronize_running_sessions()
        .map(|sessions| running_sessions_version(&sessions))
        .unwrap_or(0)
}

pub fn running_games_snapshot() -> Vec<RunningGameSnapshot> {
    synchronize_running_sessions()
        .map(|sessions| running_sessions_to_snapshots(&sessions))
        .unwrap_or_default()
}

pub fn monitor_running_game(game_id: &str) -> Result<(), String> {
    loop {
        let active = synchronize_running_sessions()?;
        if !active.iter().any(|session| session.game_id == game_id) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

pub fn stop_game(game_id: &str) -> Result<bool, String> {
    let Some(mut session) = find_running_session(game_id)? else {
        return Ok(false);
    };

    let tracked_pids = refresh_known_pids(&mut session);
    let mut killed_any = false;

    let pgid = -(session.pid as i32);
    let group_result = unsafe { libc::kill(pgid, libc::SIGKILL) };
    if group_result == 0 {
        leyen_game_log(
            game_id,
            "INFO ",
            &format!("Sent SIGKILL to process group {}", session.pid),
        );
        killed_any = true;
    }

    let group_error = std::io::Error::last_os_error();
    for tracked_pid in tracked_pids {
        if unsafe { libc::kill(tracked_pid as i32, libc::SIGKILL) } == 0 {
            killed_any = true;
        }
    }

    if killed_any {
        let _ = mark_running_session_termination_requested(game_id);
        leyen_game_log(
            game_id,
            "INFO ",
            &format!(
                "Sent SIGKILL to tracked processes for root pid {}",
                session.pid
            ),
        );
        return Ok(true);
    }

    Err(format!(
        "Failed to stop pid {}: {}",
        session.pid,
        if group_error.kind() == std::io::ErrorKind::NotFound {
            std::io::Error::last_os_error().to_string()
        } else {
            group_error.to_string()
        }
    ))
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

fn try_lock_prefix(prefix_path: &str) -> PrefixLockState {
    if prefix_path.trim().is_empty() {
        return PrefixLockState::Unavailable;
    }

    if let Err(e) = fs::create_dir_all(prefix_path) {
        leyen_log(
            "WARN ",
            &format!("Failed to create prefix directory '{}': {}", prefix_path, e),
        );
        return PrefixLockState::Unavailable;
    }

    match synchronize_running_sessions() {
        Ok(sessions) => {
            if sessions.iter().any(|session| {
                session.match_prefix_path.as_deref() == Some(prefix_path)
                    || session.known_pids.iter().any(|pid| {
                        read_process_env(*pid)
                            .and_then(|env| env.get("WINEPREFIX").cloned())
                            .as_deref()
                            == Some(prefix_path)
                    })
            }) {
                PrefixLockState::Busy
            } else {
                PrefixLockState::Available
            }
        }
        Err(e) => {
            leyen_log(
                "WARN ",
                &format!(
                    "Failed to inspect runtime prefix usage '{}': {}",
                    prefix_path, e
                ),
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

    for entry in bytes.split(|byte| *byte == 0) {
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
    Path::new(&format!("/proc/{pid}")).exists()
}

fn collect_descendant_pids(roots: &HashSet<u32>) -> HashSet<u32> {
    let children_map = build_process_children_map();
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
) -> HashSet<u32> {
    let mut matched = HashSet::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return matched;
    };

    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };
        let Some(env) = read_process_env(pid) else {
            continue;
        };
        if process_matches_runtime(&env, match_prefix_path, match_game_id) {
            matched.insert(pid);
        }
    }

    matched
}

fn refresh_known_pids(session: &mut RunningGameSession) -> HashSet<u32> {
    let mut roots: HashSet<u32> = session.known_pids.iter().copied().collect();
    roots.insert(session.pid);

    let alive_roots: HashSet<u32> = roots.into_iter().filter(|pid| is_pid_alive(*pid)).collect();
    let mut discovered = collect_descendant_pids(&alive_roots);
    let matched = collect_runtime_matched_pids(
        session.match_prefix_path.as_deref(),
        session.match_game_id.as_deref(),
    );
    discovered.extend(matched);

    if !discovered.is_empty() {
        discovered = collect_descendant_pids(&discovered);
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
                    leyen_game_log(
                        &game_id,
                        "INFO ",
                        &format!("[{}:{}] {}", game_title, stream_name, line),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    leyen_game_log(
                        &game_id,
                        "WARN ",
                        &format!(
                            "Failed to read {} output for '{}': {}",
                            stream_name, game_title, e
                        ),
                    );
                    break;
                }
            }
        }
    });
}

pub fn launch_game(game: &Game, overlay: &adw::ToastOverlay) {
    match launch_game_managed(game, true, true, true) {
        Ok(report) => {
            for notice in report.notices {
                overlay.add_toast(adw::Toast::new(&notice));
            }
        }
        Err(err) => overlay.add_toast(adw::Toast::new(&err)),
    }
}

pub fn launch_game_headless(game: &Game) -> Result<LaunchReport, String> {
    launch_game_managed(game, true, true, false)
}

fn spawn_detached_monitor(game_id: &str) {
    let Ok(current_exe) = std::env::current_exe() else {
        leyen_game_log(
            game_id,
            "WARN ",
            "Failed to resolve the current executable for the runtime monitor",
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
        leyen_game_log(
            game_id,
            "WARN ",
            &format!("Failed to start the runtime monitor: {}", e),
        );
    }
}

fn launch_game_managed(
    game: &Game,
    capture_output: bool,
    reap_child_locally: bool,
    spawn_background_monitor: bool,
) -> Result<LaunchReport, String> {
    let mut notices = Vec::new();

    // Block launch while umu-launcher is being downloaded.
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("umu-launcher is still downloading, please wait…".to_string());
    }

    // Block launch if umu-run is simply not available.
    if !is_umu_run_available() {
        return Err(
            "umu-launcher is not installed. Please check your internet connection and restart."
                .to_string(),
        );
    }

    let settings = load_settings_with_auto_install(false);
    let library = load_library();
    let parent_group = find_game_and_group(&library, &game.id).and_then(|(_, group)| group);
    let prefix_path = resolve_launch_prefix(game, parent_group, &settings.default_prefix_path);
    let launch_game_id = effective_game_id(game);

    if is_game_running(&game.id) {
        return Err("This game is already running".to_string());
    }

    let mut env_vars: Vec<(String, String)> = Vec::new();
    if !prefix_path.is_empty() {
        env_vars.push(("WINEPREFIX".to_string(), prefix_path.clone()));
    }

    if !launch_game_id.is_empty() {
        env_vars.push(("GAMEID".to_string(), launch_game_id.clone()));
    }

    let proton_path = match resolve_launch_proton(game, parent_group, &settings.default_proton) {
        Some(path) if path.starts_with('/') && !Path::new(&path).exists() => {
            leyen_game_log(
                &game.id,
                "ERROR",
                &format!("Proton path for '{}' does not exist: {}", game.title, path),
            );
            return Err("Selected Proton version was not found".to_string());
        }
        Some(path) => {
            env_vars.push(("PROTONPATH".to_string(), path.clone()));
            path
        }
        None => settings.default_proton.clone(),
    };

    if mangohud_available() && (game.force_mangohud || settings.global_mangohud) {
        env_vars.push(("MANGOHUD".to_string(), "1".to_string()));
    }

    env_vars.push((
        "PROTON_ENABLE_WAYLAND".to_string(),
        if game.game_wayland || settings.global_wayland {
            "1".to_string()
        } else {
            "0".to_string()
        },
    ));

    env_vars.push((
        "PROTON_USE_WOW64".to_string(),
        if game.game_wow64 || settings.global_wow64 {
            "1".to_string()
        } else {
            "0".to_string()
        },
    ));

    let ntsync_val = if game.game_ntsync || settings.global_ntsync {
        "1"
    } else {
        "0"
    };
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

        if gamemode_available() && (game.force_gamemode || settings.global_gamemode) {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.extend(cmd_wrappers);
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        cmd_args.extend(postfix);
    } else {
        if gamemode_available() && (game.force_gamemode || settings.global_gamemode) {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        if !game.launch_args.is_empty() {
            cmd_args.extend(split_shell_words(&game.launch_args));
        }
    }

    let working_dir = working_directory_for(&game.exe_path);
    match try_lock_prefix(&prefix_path) {
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
    leyen_game_log(&game.id, "INFO ", &launch_summary);

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

            if !try_register_running_session(session)? {
                let _ = unsafe { libc::kill(-(child_pid as i32), libc::SIGKILL) };
                let _ = child.wait();
                return Err("This game is already running".to_string());
            }

            let _ = record_game_launch_start(&game.id, started_at_epoch_seconds);

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

            leyen_game_log(
                &game.id,
                "INFO ",
                &format!("Spawned '{}' with pid {}", game.title, child_pid),
            );
            notices.push(format!("Launching {}...", game.title));
            Ok(LaunchReport { notices })
        }
        Err(e) => {
            leyen_log(
                "ERROR",
                &format!("Failed to launch '{}': {}", game.title, e),
            );
            Err(format!("Failed to launch: {}", e))
        }
    }
}
