use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use gtk4::glib;

use crate::config::{find_game_by_leyen_id, load_library};
use crate::launch::{
    launch_game_headless, monitor_running_game, running_games_snapshot, stop_game,
};
use crate::models::{Game, LibraryItem};
use crate::umu::{UMU_DOWNLOADING, check_or_install_umu, is_umu_run_available};

static OPEN_LOGS_ON_START: AtomicBool = AtomicBool::new(false);

pub fn maybe_run_from_args() -> Option<glib::ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return None;
    }

    if args[0] == "logs" {
        if args.len() > 1 {
            eprintln!(
                "`leyen logs` does not take extra arguments\n\n{}",
                usage_text()
            );
            return Some(glib::ExitCode::FAILURE);
        }
        OPEN_LOGS_ON_START.store(true, Relaxed);
        return None;
    }

    let result = match args[0].as_str() {
        "help" | "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        "list" => list_games(&args[1..]),
        "run" => run_game(&args[1..]),
        "kill" => kill_game(&args[1..]),
        "internal-run" => internal_run(&args[1..]),
        "internal-monitor" => internal_monitor(&args[1..]),
        other => Err(format!("Unknown command '{other}'\n\n{}", usage_text())),
    };

    Some(match result {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            glib::ExitCode::FAILURE
        }
    })
}

pub fn take_open_logs_on_start() -> bool {
    OPEN_LOGS_ON_START.swap(false, Relaxed)
}

fn usage_text() -> &'static str {
    "Usage:
  leyen
  leyen list
  leyen run <leyen-id>
  leyen logs
  leyen kill <leyen-id>"
}

fn print_usage() {
    println!("{}", usage_text());
}

fn list_games(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err(format!(
            "`leyen list` does not take extra arguments\n\n{}",
            usage_text()
        ));
    }

    let items = load_library();
    let running_map = running_games_index();

    if items.is_empty() {
        if running_map.is_empty() {
            println!("No games configured.");
        } else {
            println!("Running");
            for snapshot in running_map.values() {
                println!(
                    "  <unknown>  {}  [running, pid {}, {} process{}]",
                    snapshot.game_id,
                    snapshot.pid,
                    snapshot.tracked_pid_count,
                    if snapshot.tracked_pid_count == 1 {
                        ""
                    } else {
                        "es"
                    }
                );
            }
        }
        return Ok(());
    }

    let indexed_games = index_games(&items);

    if !running_map.is_empty() {
        println!("Running");

        let mut running_games: Vec<&Game> = indexed_games
            .values()
            .copied()
            .filter(|game| running_map.contains_key(&game.id))
            .collect();
        running_games.sort_by_key(|game| game.title.to_lowercase());

        for game in running_games {
            if let Some(snapshot) = running_map.get(&game.id) {
                println!(
                    "  {}  {}  [running, pid {}, {} process{}]",
                    game.leyen_id,
                    game.title,
                    snapshot.pid,
                    snapshot.tracked_pid_count,
                    if snapshot.tracked_pid_count == 1 {
                        ""
                    } else {
                        "es"
                    }
                );
            }
        }

        println!();
    }

    let mut root_game_count = 0usize;
    for item in &items {
        if let LibraryItem::Game(_) = item {
            root_game_count += 1;
        }
    }

    if root_game_count > 0 {
        println!("Games");
        for item in &items {
            if let LibraryItem::Game(game) = item {
                print_list_row(game, running_map.contains_key(&game.id));
            }
        }
    }

    let mut printed_groups = false;
    for item in &items {
        if let LibraryItem::Group(group) = item {
            if !printed_groups {
                if root_game_count > 0 {
                    println!();
                }
                println!("Groups");
                printed_groups = true;
            }

            println!("[group] {}", group.title);
            if group.games.is_empty() {
                println!("  <empty>");
                continue;
            }

            for game in &group.games {
                print_list_row(game, running_map.contains_key(&game.id));
            }
        }
    }

    Ok(())
}

fn run_game(args: &[String]) -> Result<(), String> {
    let [requested_leyen_id] = args else {
        return Err(format!(
            "`leyen run` requires exactly one Leyen ID\n\n{}",
            usage_text()
        ));
    };

    ensure_umu_available_for_cli()?;

    let items = load_library();
    let Some((game, group)) = find_game_by_leyen_id(&items, requested_leyen_id) else {
        return Err(format!(
            "No game found for Leyen ID '{}'. Use `leyen list` to inspect available games.",
            requested_leyen_id
        ));
    };

    let current_exe = std::env::current_exe()
        .map_err(|e| format!("Failed to resolve the current executable: {}", e))?;
    Command::new(current_exe)
        .arg("internal-run")
        .arg(&game.leyen_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start detached launch helper: {}", e))?;

    match group {
        Some(group) => eprintln!(
            "Managed launch active for '{}' ({}) in group '{}'.",
            game.title, game.leyen_id, group.title
        ),
        None => eprintln!(
            "Managed launch active for '{}' ({}).",
            game.title, game.leyen_id
        ),
    }

    Ok(())
}

fn internal_run(args: &[String]) -> Result<(), String> {
    let [requested_leyen_id] = args else {
        return Err("internal-run requires exactly one Leyen ID".to_string());
    };

    let items = load_library();
    let Some((game, _group)) = find_game_by_leyen_id(&items, requested_leyen_id) else {
        return Err(format!(
            "No game found for Leyen ID '{}'.",
            requested_leyen_id
        ));
    };

    let _ = launch_game_headless(game)?;
    monitor_running_game(&game.id)
}

fn kill_game(args: &[String]) -> Result<(), String> {
    let [requested_leyen_id] = args else {
        return Err(format!(
            "`leyen kill` requires exactly one Leyen ID\n\n{}",
            usage_text()
        ));
    };

    let items = load_library();
    let Some((game, _group)) = find_game_by_leyen_id(&items, requested_leyen_id) else {
        return Err(format!(
            "No game found for Leyen ID '{}'. Use `leyen list` to inspect available games.",
            requested_leyen_id
        ));
    };

    match stop_game(&game.id)? {
        true => {
            eprintln!("Stopping '{}' ({})...", game.title, game.leyen_id);
            Ok(())
        }
        false => Err(format!(
            "'{}' ({}) is not running",
            game.title, game.leyen_id
        )),
    }
}

fn internal_monitor(args: &[String]) -> Result<(), String> {
    let [game_id] = args else {
        return Err("internal-monitor requires exactly one internal game id".to_string());
    };

    monitor_running_game(game_id)
}

fn index_games(items: &[LibraryItem]) -> HashMap<String, &Game> {
    let mut indexed = HashMap::new();

    for item in items {
        match item {
            LibraryItem::Game(game) => {
                indexed.insert(game.id.clone(), game);
            }
            LibraryItem::Group(group) => {
                for game in &group.games {
                    indexed.insert(game.id.clone(), game);
                }
            }
        }
    }

    indexed
}

fn running_games_index() -> HashMap<String, crate::launch::RunningGameSnapshot> {
    running_games_snapshot()
        .into_iter()
        .map(|snapshot| (snapshot.game_id.clone(), snapshot))
        .collect()
}

fn print_list_row(game: &Game, running: bool) {
    println!(
        "  {}  {}{}",
        game.leyen_id,
        game.title,
        if running { "  [running]" } else { "" }
    );
}

fn ensure_umu_available_for_cli() -> Result<(), String> {
    if is_umu_run_available() {
        return Ok(());
    }

    eprintln!("umu-launcher not found. Installing local runtime...");
    check_or_install_umu();

    while UMU_DOWNLOADING.load(Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    if is_umu_run_available() {
        Ok(())
    } else {
        Err("umu-launcher is not installed and automatic installation failed".to_string())
    }
}
