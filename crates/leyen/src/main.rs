//! `leyen` — the thin CLI client. Routes list/run/kill/logs through the daemon
//! over D-Bus (auto-activating it). The library is read directly from
//! `games.toml`; running state and lifecycle come from the daemon.

use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use zbus::Connection;

use leyen_ipc::{LeyenProxy, RunningGameSnapshot};
use leyen_model::library::{find_game_by_leyen_id, read_library_from_disk};
use leyen_model::models::{Game, LibraryItem};

#[derive(Parser)]
#[command(name = "leyen")]
#[command(about = "A small GTK4/libadwaita launcher for Windows games on Linux", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
    /// Stream game logs to stdout
    Logs {
        /// Keep streaming new log lines until interrupted
        #[arg(short, long)]
        follow: bool,
    },
    /// Stop a running game by its Leyen ID
    Kill {
        /// The Leyen ID of the game to stop (e.g., ly-1234)
        leyen_id: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::List => list_games().await,
        Commands::Run { leyen_id } => run_game(&leyen_id).await,
        Commands::Logs { follow } => stream_logs(follow).await,
        Commands::Kill { leyen_id } => kill_game(&leyen_id).await,
    };
    if let Err(err) = result {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

async fn proxy() -> Result<LeyenProxy<'static>> {
    let connection = Connection::session()
        .await
        .context("Failed to connect to the session bus")?;
    LeyenProxy::new(&connection)
        .await
        .context("Failed to reach the Leyen daemon")
}

fn load_library() -> Result<Vec<LibraryItem>> {
    read_library_from_disk().map_err(anyhow::Error::msg)
}

async fn running_games_index() -> HashMap<String, RunningGameSnapshot> {
    match proxy().await {
        Ok(p) => match p.get_running_games().await {
            Ok(snapshots) => snapshots
                .into_iter()
                .map(|s| (s.game_id.clone(), s))
                .collect(),
            Err(err) => {
                eprintln!("Warning: could not read running games state: {err}");
                HashMap::new()
            }
        },
        Err(err) => {
            eprintln!("Warning: could not reach the Leyen daemon: {err}");
            HashMap::new()
        }
    }
}

async fn list_games() -> Result<()> {
    let items = load_library()?;
    let running_map = running_games_index().await;

    if items.is_empty() {
        if running_map.is_empty() {
            println!("No games configured.");
        } else {
            println!("Running");
            for snapshot in running_map.values() {
                println!(
                    "  {}  <unknown>  [running, pid {}, {} process{}]",
                    snapshot.leyen_id,
                    snapshot.pid,
                    snapshot.tracked_pid_count,
                    if snapshot.tracked_pid_count == 1 { "" } else { "es" }
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
                    if snapshot.tracked_pid_count == 1 { "" } else { "es" }
                );
            }
        }

        println!();
    }

    let root_game_count = items
        .iter()
        .filter(|item| matches!(item, LibraryItem::Game(_)))
        .count();

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

async fn run_game(leyen_id: &str) -> Result<()> {
    let items = load_library()?;
    let Some((game, group)) = find_game_by_leyen_id(&items, leyen_id) else {
        anyhow::bail!(
            "No game found for Leyen ID '{leyen_id}'. Use `leyen list` to inspect available games."
        );
    };
    let (title, group_title) = (game.title.clone(), group.map(|g| g.title.clone()));

    let started = proxy()
        .await?
        .launch_game(leyen_id)
        .await
        .context("Failed to launch game")?;
    if !started {
        anyhow::bail!("Failed to launch '{}' ({}).", title, leyen_id);
    }

    match group_title {
        Some(group) => eprintln!("Managed launch active for '{title}' ({leyen_id}) in group '{group}'."),
        None => eprintln!("Managed launch active for '{title}' ({leyen_id})."),
    }
    Ok(())
}

async fn kill_game(leyen_id: &str) -> Result<()> {
    let items = load_library()?;
    let Some((game, _group)) = find_game_by_leyen_id(&items, leyen_id) else {
        anyhow::bail!(
            "No game found for Leyen ID '{leyen_id}'. Use `leyen list` to inspect available games."
        );
    };
    let title = game.title.clone();

    let was_running = proxy()
        .await?
        .stop_game(leyen_id)
        .await
        .context("Failed to stop game")?;
    if was_running {
        eprintln!("Stopping '{title}' ({leyen_id})...");
        Ok(())
    } else {
        anyhow::bail!("'{}' ({}) is not running", title, leyen_id)
    }
}

async fn stream_logs(follow: bool) -> Result<()> {
    let proxy = proxy().await?;

    // Subscribe before the initial pull so nothing is missed between them.
    let mut appended = proxy.receive_logs_appended().await?;

    let mut offset = 0u64;
    offset = print_logs_since(&proxy, offset).await?;

    if !follow {
        return Ok(());
    }

    while appended.next().await.is_some() {
        offset = print_logs_since(&proxy, offset).await?;
    }
    Ok(())
}

async fn print_logs_since(proxy: &LeyenProxy<'_>, offset: u64) -> Result<u64> {
    let (next, entries) = proxy.get_logs(offset).await.context("Failed to fetch logs")?;
    for entry in entries {
        println!("[{}] {}", entry.timestamp, entry.line);
    }
    Ok(next)
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

fn print_list_row(game: &Game, running: bool) {
    println!(
        "  {}  {}{}",
        game.leyen_id,
        game.title,
        if running { "  [running]" } else { "" }
    );
}
