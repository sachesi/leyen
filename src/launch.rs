use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use libadwaita as adw;

use crate::config::{
    add_game_playtime, effective_game_id, find_game_and_group, load_library, load_settings,
    record_game_launch_result, record_game_launch_start,
};
use crate::logging::{leyen_game_log, leyen_log};
use crate::models::{Game, GameGroup};
use crate::proton::resolve_proton_path;
use crate::umu::{UMU_DOWNLOADING, get_umu_run_path, is_umu_run_available};

struct RunningGame {
    pid: u32,
    started_at: Instant,
    known_pids: HashSet<u32>,
    match_prefix_path: Option<String>,
    match_game_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunningGameSnapshot {
    pub game_id: String,
    pub pid: u32,
    pub tracked_pid_count: usize,
    pub elapsed_seconds: u64,
}

static RUNNING_GAMES: LazyLock<Mutex<HashMap<String, RunningGame>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static RUNNING_GAMES_VERSION: AtomicU64 = AtomicU64::new(0);

enum PrefixLockState {
    Locked(File),
    Busy,
    Unavailable,
}

fn split_shell_words(input: &str) -> Vec<String> {
    shlex::split(input).unwrap_or_else(|| input.split_whitespace().map(str::to_string).collect())
}

fn is_valid_env_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn try_mark_game_running(
    game_id: &str,
    pid: u32,
    match_prefix_path: Option<String>,
    match_game_id: Option<String>,
) -> bool {
    let mut running = RUNNING_GAMES.lock().unwrap_or_else(|e| e.into_inner());
    if running.contains_key(game_id) {
        return false;
    }
    running.insert(
        game_id.to_string(),
        RunningGame {
            pid,
            started_at: Instant::now(),
            known_pids: HashSet::from([pid]),
            match_prefix_path,
            match_game_id,
        },
    );
    RUNNING_GAMES_VERSION.fetch_add(1, Relaxed);
    true
}

fn clear_running_game(game_id: &str) -> Option<RunningGame> {
    if let Ok(mut running) = RUNNING_GAMES.lock() {
        let removed = running.remove(game_id);
        if removed.is_some() {
            RUNNING_GAMES_VERSION.fetch_add(1, Relaxed);
        }
        removed
    } else {
        None
    }
}

pub fn is_game_running(game_id: &str) -> bool {
    RUNNING_GAMES
        .lock()
        .map(|running| running.contains_key(game_id))
        .unwrap_or(false)
}

pub fn running_games_version() -> u64 {
    RUNNING_GAMES_VERSION.load(Relaxed)
}

pub fn running_games_snapshot() -> Vec<RunningGameSnapshot> {
    let Ok(running) = RUNNING_GAMES.lock() else {
        return Vec::new();
    };

    let mut snapshots: Vec<RunningGameSnapshot> = running
        .iter()
        .map(|(game_id, game)| RunningGameSnapshot {
            game_id: game_id.clone(),
            pid: game.pid,
            tracked_pid_count: game.known_pids.len(),
            elapsed_seconds: game.started_at.elapsed().as_secs(),
        })
        .collect();

    snapshots.sort_by(|left, right| right.elapsed_seconds.cmp(&left.elapsed_seconds));
    snapshots
}

pub fn has_running_games() -> bool {
    RUNNING_GAMES
        .lock()
        .map(|running| !running.is_empty())
        .unwrap_or(false)
}

pub fn running_game_elapsed(game_id: &str) -> Option<Duration> {
    RUNNING_GAMES
        .lock()
        .ok()
        .and_then(|running| running.get(game_id).map(|game| game.started_at.elapsed()))
}

pub fn stop_game(game_id: &str) -> Result<bool, String> {
    let (pid, known_pids) = match RUNNING_GAMES.lock() {
        Ok(running) => match running.get(game_id) {
            Some(game) => (game.pid, game.known_pids.clone()),
            None => return Ok(false),
        },
        Err(_) => return Err("Failed to access running game state".to_string()),
    };

    let tracked_pids = refresh_known_pids(game_id).unwrap_or_else(|| {
        if known_pids.is_empty() {
            HashSet::new()
        } else {
            collect_descendant_pids(&known_pids)
        }
    });

    let mut killed_any = false;

    let pgid = -(pid as i32);
    let group_result = unsafe { libc::kill(pgid, libc::SIGKILL) };
    if group_result == 0 {
        leyen_game_log(
            game_id,
            "INFO ",
            &format!("Sent SIGKILL to process group {}", pid),
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
        leyen_game_log(
            game_id,
            "INFO ",
            &format!("Sent SIGKILL to tracked processes for root pid {}", pid),
        );
        return Ok(true);
    }

    Err(format!(
        "Failed to stop pid {}: {}",
        pid,
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

    let lock_path = Path::new(prefix_path).join(".leyen.lock");
    let lock_file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(e) => {
            leyen_log(
                "WARN ",
                &format!(
                    "Failed to open prefix lock '{}': {}",
                    lock_path.display(),
                    e
                ),
            );
            return PrefixLockState::Unavailable;
        }
    };

    let flock_result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if flock_result == 0 {
        PrefixLockState::Locked(lock_file)
    } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
        PrefixLockState::Busy
    } else {
        leyen_log(
            "WARN ",
            &format!(
                "Failed to acquire prefix lock '{}': {}",
                lock_path.display(),
                std::io::Error::last_os_error()
            ),
        );
        PrefixLockState::Unavailable
    }
}

fn release_prefix_lock(lock_file: File) {
    let _ = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN) };
    drop(lock_file);
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

fn refresh_known_pids(game_id: &str) -> Option<HashSet<u32>> {
    let (roots, match_prefix_path, match_game_id) =
        RUNNING_GAMES.lock().ok().and_then(|running| {
            running.get(game_id).map(|game| {
                (
                    game.known_pids.clone(),
                    game.match_prefix_path.clone(),
                    game.match_game_id.clone(),
                )
            })
        })?;

    let alive_roots: HashSet<u32> = roots.into_iter().filter(|pid| is_pid_alive(*pid)).collect();
    let mut discovered = collect_descendant_pids(&alive_roots);
    let matched =
        collect_runtime_matched_pids(match_prefix_path.as_deref(), match_game_id.as_deref());
    discovered.extend(matched);
    discovered = collect_descendant_pids(&discovered);

    if let Ok(mut running) = RUNNING_GAMES.lock()
        && let Some(game) = running.get_mut(game_id)
    {
        game.known_pids = discovered.clone();
    }

    Some(discovered)
}

fn wait_for_remaining_processes(game_id: &str, timeout: Duration) {
    let started = Instant::now();

    while started.elapsed() < timeout {
        let Some(known_pids) = refresh_known_pids(game_id) else {
            return;
        };

        if known_pids.is_empty() {
            return;
        }

        std::thread::sleep(Duration::from_secs(1));
    }
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
    // Block launch while umu-launcher is being downloaded.
    if UMU_DOWNLOADING.load(std::sync::atomic::Ordering::Relaxed) {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is still downloading, please wait…",
        ));
        return;
    }

    // Block launch if umu-run is simply not available.
    if !is_umu_run_available() {
        overlay.add_toast(adw::Toast::new(
            "umu-launcher is not installed. Please check your internet connection and restart.",
        ));
        return;
    }

    let settings = load_settings();
    let library = load_library();
    let parent_group = find_game_and_group(&library, &game.id).and_then(|(_, group)| group);
    let prefix_path = resolve_launch_prefix(game, parent_group, &settings.default_prefix_path);
    let launch_game_id = effective_game_id(game);

    if is_game_running(&game.id) {
        overlay.add_toast(adw::Toast::new("This game is already running"));
        return;
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
            overlay.add_toast(adw::Toast::new("Selected Proton version was not found"));
            leyen_game_log(
                &game.id,
                "ERROR",
                &format!("Proton path for '{}' does not exist: {}", game.title, path),
            );
            return;
        }
        Some(path) => {
            env_vars.push(("PROTONPATH".to_string(), path.clone()));
            path
        }
        None => settings.default_proton.clone(),
    };

    if game.force_mangohud || settings.global_mangohud {
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

        if game.force_gamemode || settings.global_gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.extend(cmd_wrappers);
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        cmd_args.extend(postfix);
    } else {
        if game.force_gamemode || settings.global_gamemode {
            cmd_args.push("gamemoderun".to_string());
        }
        cmd_args.push(umu.clone());
        cmd_args.push(game.exe_path.clone());
        if !game.launch_args.is_empty() {
            cmd_args.extend(split_shell_words(&game.launch_args));
        }
    }

    let working_dir = working_directory_for(&game.exe_path);
    let prefix_lock = match try_lock_prefix(&prefix_path) {
        PrefixLockState::Locked(file) => Some(file),
        PrefixLockState::Busy => {
            env_vars.push(("UMU_CONTAINER_NSENTER".to_string(), "1".to_string()));
            overlay.add_toast(adw::Toast::new(
                "Prefix is already in use. Launching with shared-container fallback.",
            ));
            None
        }
        PrefixLockState::Unavailable => None,
    };

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
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
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
            if !try_mark_game_running(
                &game.id,
                child_pid,
                (!prefix_path.is_empty()).then_some(prefix_path.clone()),
                (!launch_game_id.is_empty()).then_some(launch_game_id.clone()),
            ) {
                let _ = unsafe { libc::kill(-(child_pid as i32), libc::SIGKILL) };
                let _ = child.wait();
                overlay.add_toast(adw::Toast::new("This game is already running"));
                return;
            }
            let started_at_epoch_seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let _ = record_game_launch_start(&game.id, started_at_epoch_seconds);
            let monitor_game_id = game.id.clone();
            std::thread::spawn(move || {
                while is_game_running(&monitor_game_id) {
                    let _ = refresh_known_pids(&monitor_game_id);
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
            if let Some(stdout) = child.stdout.take() {
                pipe_process_output(stdout, game.id.clone(), game.title.clone(), "stdout");
            }
            if let Some(stderr) = child.stderr.take() {
                pipe_process_output(stderr, game.id.clone(), game.title.clone(), "stderr");
            }

            let game_id = game.id.clone();
            let game_title = game.title.clone();
            std::thread::spawn(move || {
                let status = child.wait();
                if let Some(lock_file) = prefix_lock {
                    release_prefix_lock(lock_file);
                }
                wait_for_remaining_processes(&game_id, Duration::from_secs(30));
                let elapsed_seconds = clear_running_game(&game_id)
                    .map(|session| session.started_at.elapsed().as_secs())
                    .unwrap_or(0);

                let total_playtime = add_game_playtime(&game_id, elapsed_seconds);

                match status {
                    Ok(status) => {
                        let exit_status = status
                            .code()
                            .map(|code| format!("status code {}", code))
                            .unwrap_or_else(|| "signal termination".to_string());
                        leyen_game_log(
                            &game_id,
                            "INFO ",
                            &format!(
                                "'{}' exited with {} after {}s",
                                game_title, exit_status, elapsed_seconds
                            ),
                        );
                        let _ = record_game_launch_result(
                            &game_id,
                            elapsed_seconds,
                            &format!("Last run: {}", exit_status),
                        );
                        if let Some(total) = total_playtime {
                            leyen_game_log(
                                &game_id,
                                "INFO ",
                                &format!(
                                    "'{}' total recorded playtime is now {}s",
                                    game_title, total
                                ),
                            );
                        }
                    }
                    Err(e) => {
                        let _ = record_game_launch_result(
                            &game_id,
                            elapsed_seconds,
                            &format!("Last run failed: {}", e),
                        );
                        leyen_game_log(
                            &game_id,
                            "ERROR",
                            &format!("Failed while waiting for '{}': {}", game_title, e),
                        );
                    }
                }
            });

            overlay.add_toast(adw::Toast::new(&format!("Launching {}...", game.title)));
            leyen_game_log(
                &game.id,
                "INFO ",
                &format!("Spawned '{}' with pid {}", game.title, child_pid),
            );
        }
        Err(e) => {
            leyen_log(
                "ERROR",
                &format!("Failed to launch '{}': {}", game.title, e),
            );
            overlay.add_toast(adw::Toast::new(&format!("Failed to launch: {}", e)));
        }
    }
}
