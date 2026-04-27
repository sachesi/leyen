use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use gtk4::glib;

use crate::config::{find_game_by_leyen_id, load_library};
use crate::launch::{
    launch_game_headless, monitor_running_game, running_games_snapshot, stop_game,
};
use crate::models::{Game, LibraryItem};
use crate::runtime::umu::{UMU_DOWNLOADING, check_or_install_umu, is_umu_run_available};

static OPEN_LOGS_ON_START: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(name = "leyen")]
#[command(about = "A small GTK4/libadwaita launcher for Windows games on Linux", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List configured games
    List,
    /// Launch a game by its Leyen ID
    Run {
        /// The Leyen ID of the game to launch (e.g., ly-1234)
        leyen_id: String,
    },
    /// Open the logs window
    Logs,
    /// Stop a running game by its Leyen ID
    Kill {
        /// The Leyen ID of the game to stop (e.g., ly-1234)
        leyen_id: String,
    },
    /// Internal command to run a game in a detached process
    #[command(hide = true)]
    InternalRun { leyen_id: String },
    /// Internal command to monitor a running game process
    #[command(hide = true)]
    InternalMonitor { game_id: String },
}

pub fn maybe_run_from_args() -> Option<glib::ExitCode> {
    let args = std::env::args().collect::<Vec<_>>();
    
    // We use try_parse to handle errors ourselves if needed, 
    // or just use parse() which will exit on help/error.
    // However, we want to return None if no args were provided to start the GUI.
    if args.len() <= 1 {
        return None;
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // If it's a help or version message, clap handles it.
            // If it's an error, we print it and exit.
            e.print().unwrap();
            return Some(if e.use_stderr() {
                glib::ExitCode::FAILURE
            } else {
                glib::ExitCode::SUCCESS
            });
        }
    };

    let Some(command) = cli.command else {
        return None;
    };

    let result = match command {
        Commands::List => list_games(),
        Commands::Run { leyen_id } => run_game(&leyen_id),
        Commands::Logs => {
            OPEN_LOGS_ON_START.store(true, Relaxed);
            return None; // Return None to continue to GUI start
        }
        Commands::Kill { leyen_id } => kill_game(&leyen_id),
        Commands::InternalRun { leyen_id } => internal_run(&leyen_id),
        Commands::InternalMonitor { game_id } => internal_monitor(&game_id),
    };

    Some(match result {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err:?}");
            glib::ExitCode::FAILURE
        }
    })
}

pub fn take_open_logs_on_start() -> bool {
    OPEN_LOGS_ON_START.swap(false, Relaxed)
}

fn list_games() -> Result<()> {
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

fn run_game(requested_leyen_id: &str) -> Result<()> {
    ensure_umu_available_for_cli()?;

    let items = load_library();
    let Some((game, group)) = find_game_by_leyen_id(&items, requested_leyen_id) else {
        anyhow::bail!("No game found for Leyen ID '{requested_leyen_id}'. Use `leyen list` to inspect available games.");
    };

    let current_exe = std::env::current_exe()
        .context("Failed to resolve the current executable")?;
    Command::new(current_exe)
        .arg("internal-run")
        .arg(&game.leyen_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to start detached launch helper")?;

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

fn internal_run(requested_leyen_id: &str) -> Result<()> {
    let items = load_library();
    let Some((game, _group)) = find_game_by_leyen_id(&items, requested_leyen_id) else {
        anyhow::bail!("No game found for Leyen ID '{requested_leyen_id}'.");
    };

    launch_game_headless(game).context("Failed to launch game")?;
    monitor_running_game(&game.id).context("Failed to monitor game")?;
    Ok(())
}

fn kill_game(requested_leyen_id: &str) -> Result<()> {
    let items = load_library();
    let Some((game, _group)) = find_game_by_leyen_id(&items, requested_leyen_id) else {
        anyhow::bail!("No game found for Leyen ID '{requested_leyen_id}'. Use `leyen list` to inspect available games.");
    };

    match stop_game(&game.id).context("Failed to stop game")? {
        true => {
            eprintln!("Stopping '{}' ({})...", game.title, game.leyen_id);
            Ok(())
        }
        false => anyhow::bail!("'{}' ({}) is not running", game.title, game.leyen_id),
    }
}

fn internal_monitor(game_id: &str) -> Result<()> {
    monitor_running_game(game_id).context("Failed to monitor game")?;
    Ok(())
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

fn ensure_umu_available_for_cli() -> Result<()> {
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
        anyhow::bail!("umu-launcher is not installed and automatic installation failed")
    }
}
