//! `leyend` — the Leyen daemon. Sole owner of systemd scopes, the single
//! running-session monitor, the running-state registry, game-output capture +
//! log ring buffer, the dependency engine, runtime installation, and library
//! writes. Exposes `com.github.sachesi.leyen` on the session bus; clients are
//! thin. D-Bus-activated, singleton via bus-name ownership, idle-exits when no
//! game is running and no request has arrived recently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use log::{info, warn};
use zbus::Connection;
use zbus::connection::Builder;

use leyen_ipc::{INTERFACE, OBJECT_PATH, RuntimeReadiness};
use leyen_model::library::{find_game_by_leyen_id, flatten_games};

/// Seconds of "no running game AND no client request" after which the daemon
/// exits. Activation restarts it on the next call.
const IDLE_EXIT_SECONDS: u64 = 30;

type DepJobs = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[derive(Clone)]
struct Manager {
    conn: Arc<OnceLock<Connection>>,
    dep_jobs: DepJobs,
    last_activity: Arc<Mutex<Instant>>,
}

impl Manager {
    fn touch(&self) {
        if let Ok(mut t) = self.last_activity.lock() {
            *t = Instant::now();
        }
    }

    fn connection(&self) -> Option<Connection> {
        self.conn.get().cloned()
    }
}

/// Maps the engine's running snapshots to the wire type, attaching each game's
/// `leyen_id` from the library (the engine keys on the internal UUID).
async fn map_snapshots(
    core: Vec<leyen_core::launch::RunningGameSnapshot>,
) -> Vec<leyen_ipc::RunningGameSnapshot> {
    let library = leyen_core::config::load_library().await.unwrap_or_default();
    let leyen_ids: HashMap<String, String> = flatten_games(&library)
        .into_iter()
        .map(|g| (g.id, g.leyen_id))
        .collect();
    core.into_iter()
        .map(|s| leyen_ipc::RunningGameSnapshot {
            leyen_id: leyen_ids.get(&s.game_id).cloned().unwrap_or_default(),
            game_id: s.game_id,
            pid: s.pid as u64,
            started_at_epoch_seconds: s.started_at_epoch_seconds,
            tracked_pid_count: s.tracked_pid_count as u64,
        })
        .collect()
}

async fn current_runtime_readiness() -> RuntimeReadiness {
    let umu = tokio::task::spawn_blocking(leyen_core::runtime::umu::is_umu_run_available)
        .await
        .unwrap_or(false);
    let winetricks =
        tokio::task::spawn_blocking(leyen_core::runtime::umu::is_winetricks_available)
            .await
            .unwrap_or(false);
    RuntimeReadiness {
        umu_ready: umu,
        winetricks_ready: winetricks,
    }
}

#[zbus::interface(name = "com.github.sachesi.leyen.Manager")]
impl Manager {
    async fn launch_game(&self, leyen_id: &str) -> bool {
        self.touch();
        let library = leyen_core::config::load_library().await.unwrap_or_default();
        let Some((game, _group)) = find_game_by_leyen_id(&library, leyen_id) else {
            log::error!("LaunchGame: no game for leyen_id '{leyen_id}'");
            return false;
        };
        let game = game.clone();
        match leyen_core::launch::launch_game_headless(&game).await {
            Ok(_) => true,
            Err(e) => {
                log::error!(
                    target: &format!("game:{}", game.id),
                    "Launch of '{}' ({leyen_id}) failed: {e}",
                    game.title
                );
                false
            }
        }
    }

    async fn stop_game(&self, leyen_id: &str) -> bool {
        self.touch();
        let library = leyen_core::config::load_library().await.unwrap_or_default();
        let Some((game, _group)) = find_game_by_leyen_id(&library, leyen_id) else {
            return false;
        };
        let game_id = game.id.clone();
        leyen_core::launch::stop_game(&game_id).await.unwrap_or(false)
    }

    async fn get_running_games(&self) -> Vec<leyen_ipc::RunningGameSnapshot> {
        self.touch();
        map_snapshots(leyen_core::launch::running_games_snapshot().await).await
    }

    async fn get_runtime_status(&self) -> RuntimeReadiness {
        self.touch();
        current_runtime_readiness().await
    }

    async fn get_logs(&self, since_offset: u64) -> (u64, Vec<leyen_ipc::LogEntry>) {
        self.touch();
        let (next, entries) = leyen_core::logging::get_logs_since(since_offset);
        let mapped = entries
            .into_iter()
            .map(|e| leyen_ipc::LogEntry {
                timestamp: e.timestamp,
                line: e.line,
                game_id: e.game_id.unwrap_or_default(),
            })
            .collect();
        (next, mapped)
    }

    async fn clear_logs(&self) {
        self.touch();
        leyen_core::logging::clear_log_buffer();
    }

    async fn save_library(
        &self,
        toml_bytes: Vec<u8>,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) -> bool {
        self.touch();
        let Ok(text) = String::from_utf8(toml_bytes) else {
            warn!("SaveLibrary: payload is not valid UTF-8");
            return false;
        };
        let items = match toml::from_str::<leyen_model::models::GamesConfig>(&text) {
            Ok(config) => config.items,
            Err(e) => {
                warn!("SaveLibrary: parse failed: {e}");
                return false;
            }
        };
        leyen_core::config::save_library_merged(items).await;
        let _ = Manager::library_changed(&emitter).await;
        true
    }

    async fn install_dep(&self, prefix: &str, dep_id: &str, proton_path: &str) -> String {
        self.touch();
        if leyen_core::launch::is_any_game_running() {
            return String::new();
        }
        let job_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut jobs) = self.dep_jobs.lock() {
            jobs.insert(job_id.clone(), cancel.clone());
        }
        self.spawn_dep_job(job_id.clone(), prefix, dep_id, proton_path, cancel, true);
        job_id
    }

    async fn uninstall_dep(&self, prefix: &str, dep_id: &str, proton_path: &str) -> String {
        self.touch();
        let job_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut jobs) = self.dep_jobs.lock() {
            jobs.insert(job_id.clone(), cancel.clone());
        }
        self.spawn_dep_job(job_id.clone(), prefix, dep_id, proton_path, cancel, false);
        job_id
    }

    async fn cancel_dep(&self, job_id: &str) -> bool {
        self.touch();
        if let Ok(jobs) = self.dep_jobs.lock()
            && let Some(cancel) = jobs.get(job_id)
        {
            cancel.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    async fn get_dep_status(&self, prefix: &str) -> leyen_ipc::DepStatus {
        self.touch();
        let prefix = prefix.to_string();
        let installed = tokio::task::spawn_blocking(move || {
            leyen_model::deps::read_installed_deps(&prefix)
                .into_iter()
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        leyen_ipc::DepStatus { installed }
    }

    #[zbus(signal)]
    async fn sessions_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        sessions: Vec<leyen_ipc::RunningGameSnapshot>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn logs_appended(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        total_offset: u64,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn dep_progress(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        job_id: &str,
        prefix: &str,
        dep_id: &str,
        phase: &str,
        fraction: f64,
        msg: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn dep_finished(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        job_id: &str,
        success: bool,
        message: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn runtime_status(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        umu_ready: bool,
        winetricks_ready: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn library_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::Result<()>;
}

impl Manager {
    /// Spawns a dependency install/uninstall job: progress → `DepProgress`,
    /// terminal result → `DepFinished`.
    fn spawn_dep_job(
        &self,
        job_id: String,
        prefix: &str,
        dep_id: &str,
        proton_path: &str,
        cancel: Arc<AtomicBool>,
        install: bool,
    ) {
        let Some(conn) = self.connection() else {
            return;
        };
        let prefix = prefix.to_string();
        let dep_id = dep_id.to_string();
        let proton_path = proton_path.to_string();
        let jobs = self.dep_jobs.clone();

        // Progress callback emits DepProgress. Called on the tokio runtime.
        let progress_ctx = Arc::new((conn.clone(), job_id.clone(), prefix.clone(), dep_id.clone()));
        let on_progress = move |step: usize, total: usize, msg: String| {
            let ctx = progress_ctx.clone();
            let fraction = if total > 0 {
                step as f64 / total as f64
            } else {
                0.0
            };
            tokio::spawn(async move {
                let (conn, job_id, prefix, dep_id) = (&ctx.0, &ctx.1, &ctx.2, &ctx.3);
                let _ = emit_dep_progress(conn, job_id, prefix, dep_id, "running", fraction, &msg)
                    .await;
            });
        };

        tokio::spawn(async move {
            let result = if install {
                leyen_core::deps::install_dep(&dep_id, &prefix, &proton_path, cancel, on_progress)
                    .await
            } else {
                leyen_core::deps::uninstall_dep(&dep_id, &prefix, &proton_path, on_progress).await
            };
            if let Ok(mut jobs) = jobs.lock() {
                jobs.remove(&job_id);
            }
            let (success, message) = match result {
                Ok(note) => (true, note.unwrap_or_default()),
                Err(e) => (false, e),
            };
            let _ = emit_dep_finished(&conn, &job_id, success, &message).await;
        });
    }
}

async fn emit_dep_progress(
    conn: &Connection,
    job_id: &str,
    prefix: &str,
    dep_id: &str,
    phase: &str,
    fraction: f64,
    msg: &str,
) -> zbus::Result<()> {
    conn.emit_signal(
        Option::<&str>::None,
        OBJECT_PATH,
        INTERFACE,
        "DepProgress",
        &(job_id, prefix, dep_id, phase, fraction, msg),
    )
    .await
}

async fn emit_dep_finished(
    conn: &Connection,
    job_id: &str,
    success: bool,
    message: &str,
) -> zbus::Result<()> {
    conn.emit_signal(
        Option::<&str>::None,
        OBJECT_PATH,
        INTERFACE,
        "DepFinished",
        &(job_id, success, message),
    )
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    leyen_model::i18n::init();
    if let Err(e) = leyen_core::logging::init() {
        eprintln!("Failed to initialize logging: {e}");
    }
    let settings = leyen_core::config::load_settings().await;
    leyen_core::logging::apply_log_settings(&settings);

    let manager = Manager {
        conn: Arc::new(OnceLock::new()),
        dep_jobs: Arc::new(Mutex::new(HashMap::new())),
        last_activity: Arc::new(Mutex::new(Instant::now())),
    };
    let conn_holder = manager.conn.clone();
    let last_activity = manager.last_activity.clone();

    // Bus-name ownership is the singleton guarantee: build() fails if another
    // daemon already holds the name.
    let connection = Builder::session()?
        .name(leyen_ipc::BUS_NAME)?
        .serve_at(OBJECT_PATH, manager)?
        .build()
        .await?;
    let _ = conn_holder.set(connection.clone());
    info!("leyend: owning {} at {}", leyen_ipc::BUS_NAME, OBJECT_PATH);

    install_listeners(&connection);
    install_bus_name_probe(&connection).await;

    // Crash recovery: re-adopt live scopes, then start the one monitor.
    leyen_core::launch::reconcile_stale_sessions_on_startup().await;
    leyen_core::launch::start_running_sessions_monitor();

    // Runtime install off the request path; emit readiness as it changes.
    leyen_core::runtime::umu::check_or_install_umu().await;
    leyen_core::runtime::umu::check_or_install_winetricks().await;
    spawn_runtime_status_watcher(connection.clone());

    spawn_idle_exit(last_activity);

    // Serve forever; idle-exit terminates the process.
    std::future::pending::<()>().await;
    Ok(())
}

/// Gives the engine a session-bus name probe so it can wait for a shared
/// pressure-vessel container (`com.steampowered.App<md5(prefix)>`) to become
/// joinable before launching a same-prefix follower with NSENTER.
async fn install_bus_name_probe(connection: &Connection) {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        warn!("leyend: failed to build DBus proxy; shared-container waits disabled");
        return;
    };
    leyen_core::launch::set_bus_name_probe(move |name| {
        let dbus = dbus.clone();
        Box::pin(async move {
            match zbus::names::BusName::try_from(name) {
                Ok(bus_name) => dbus.name_has_owner(bus_name).await.unwrap_or(false),
                Err(_) => false,
            }
        })
    });
}

/// Wires the engine's publish/log hooks to D-Bus signals.
fn install_listeners(connection: &Connection) {
    // SessionsChanged: the engine publishes core snapshots; map + emit.
    let (sessions_tx, mut sessions_rx) =
        tokio::sync::mpsc::unbounded_channel::<Vec<leyen_core::launch::RunningGameSnapshot>>();
    leyen_core::launch::set_sessions_listener(move |snaps| {
        let _ = sessions_tx.send(snaps);
    });
    let conn = connection.clone();
    tokio::spawn(async move {
        while let Some(core) = sessions_rx.recv().await {
            let mapped = map_snapshots(core).await;
            let _ = conn
                .emit_signal(
                    Option::<&str>::None,
                    OBJECT_PATH,
                    INTERFACE,
                    "SessionsChanged",
                    &(mapped,),
                )
                .await;
        }
    });

    // LogsAppended: coalesce the per-line notifications (watch keeps only the
    // latest total) and throttle emissions.
    let (logs_tx, mut logs_rx) = tokio::sync::watch::channel(0u64);
    leyen_core::logging::set_logs_appended_listener(move |total| {
        let _ = logs_tx.send(total);
    });
    let conn = connection.clone();
    tokio::spawn(async move {
        while logs_rx.changed().await.is_ok() {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let total = *logs_rx.borrow_and_update();
            let _ = conn
                .emit_signal(
                    Option::<&str>::None,
                    OBJECT_PATH,
                    INTERFACE,
                    "LogsAppended",
                    &(total,),
                )
                .await;
        }
    });
}

/// Emits `RuntimeStatus` whenever umu/winetricks readiness changes. Cheap
/// (readiness checks are cached) and not the per-game scanning we eliminated.
fn spawn_runtime_status_watcher(connection: Connection) {
    tokio::spawn(async move {
        let mut last: Option<RuntimeReadiness> = None;
        loop {
            let now = current_runtime_readiness().await;
            let changed = match last {
                Some(prev) => {
                    prev.umu_ready != now.umu_ready
                        || prev.winetricks_ready != now.winetricks_ready
                }
                None => true,
            };
            if changed {
                let _ = connection
                    .emit_signal(
                        Option::<&str>::None,
                        OBJECT_PATH,
                        INTERFACE,
                        "RuntimeStatus",
                        &(now.umu_ready, now.winetricks_ready),
                    )
                    .await;
                last = Some(now);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

/// Exits the process when no game runs and no request has arrived for
/// `IDLE_EXIT_SECONDS`. The daemon must outlive any running game (it owns output
/// capture + playtime finalize), so `is_any_game_running` gates the timer.
fn spawn_idle_exit(last_activity: Arc<Mutex<Instant>>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if leyen_core::launch::is_any_game_running() {
                continue;
            }
            let idle = last_activity
                .lock()
                .map(|t| t.elapsed())
                .unwrap_or_default();
            if idle >= Duration::from_secs(IDLE_EXIT_SECONDS) {
                info!("leyend: idle for {}s, exiting", idle.as_secs());
                leyen_core::logging::shutdown();
                std::process::exit(0);
            }
        }
    });
}
