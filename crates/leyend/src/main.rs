//! `leyend` — the Leyen daemon. Sole owner of systemd scopes, the single
//! running-session monitor, the running-state registry, game-output capture +
//! log ring buffer, the dependency engine, runtime installation, and library
//! writes. Exposes `com.github.sachesi.leyen` on the session bus; clients are
//! thin. D-Bus-activated, singleton via bus-name ownership, idle-exits when no
//! game is running and no request has arrived recently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use log::{error, info, warn};
use zbus::Connection;
use zbus::connection::Builder;

use leyen_ipc::{INTERFACE, OBJECT_PATH, RuntimeReadiness};
use leyen_model::library::{find_game_by_leyen_id, flatten_games};

/// Seconds of "no running game AND no client request" after which the daemon
/// exits. Activation restarts it on the next call.
const IDLE_EXIT_SECONDS: u64 = 30;

/// Max concurrent dependency jobs (`dep_jobs` entries). Generous for real
/// multi-prefix use, but stops a buggy/looping client from spawning unbounded
/// curl/umu-run children by varying `prefix`.
const MAX_DEP_JOBS: usize = 4;

/// In-flight units of work: D-Bus method bodies, dependency jobs and detached
/// engine tasks (deferred shared-container launches). The idle-exit requires
/// zero — a 30s timer must never kill a winetricks install or a launch waiting
/// for its container.
static ACTIVE_WORK: AtomicU64 = AtomicU64::new(0);

/// RAII work token gating the idle-exit. Created at the start of any unit of
/// work, released on drop (panic-safe).
struct ActivityGuard;

impl ActivityGuard {
    fn new() -> Self {
        ACTIVE_WORK.fetch_add(1, Ordering::SeqCst);
        ActivityGuard
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        ACTIVE_WORK.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Launches currently inside `launch_game`. The launch/dep-job mutual exclusion
/// needs it: until the game is registered as running, `is_any_game_running()`
/// can't see it, so a dep job checking only that could start mid-launch.
static LAUNCHES_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);

/// RAII token for `LAUNCHES_IN_FLIGHT` (panic-safe, like `ActivityGuard`).
struct LaunchGuard;

impl LaunchGuard {
    fn new() -> Self {
        LAUNCHES_IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        LaunchGuard
    }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        LAUNCHES_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Fail closed on a poisoned `dep_jobs` lock: proceeding blind could let a
/// launch and a dependency job overlap — the corruption the lock prevents.
fn poisoned_lock_error() -> leyen_ipc::Error {
    leyen_ipc::Error::Failed("Internal state lock poisoned; restart the daemon".to_string())
}

type DepJobs = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

#[derive(Clone)]
struct Manager {
    conn: Arc<OnceLock<Connection>>,
    dep_jobs: DepJobs,
    last_activity: Arc<Mutex<Instant>>,
    /// Bumped on every successful `SaveLibrary`; the optimistic-concurrency
    /// token clients pass back as `base_version`. Session-scoped.
    library_version: Arc<AtomicU64>,
    /// Serializes `SaveLibrary` bodies so the version check, the write and the
    /// version bump are atomic against concurrent saves.
    save_lock: Arc<tokio::sync::Mutex<()>>,
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
    let library = leyen_core::config::load_library().await.unwrap_or_else(|e| {
        // Signal path — keep publishing (with empty leyen_ids) but say why.
        warn!("map_snapshots: failed to read the library: {e}");
        Vec::new()
    });
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
    async fn launch_game(&self, leyen_id: &str) -> Result<(), leyen_ipc::Error> {
        let _work = ActivityGuard::new();
        self.touch();
        // Dependency jobs mutate prefixes (wineserver, registry); launching a
        // game mid-install would corrupt both. `install_dep` refuses while a
        // game runs or launches — this is the same exclusion in the other
        // direction, made atomic by marking the launch under the same lock.
        let _launch = {
            let jobs = self.dep_jobs.lock().map_err(|_| poisoned_lock_error())?;
            if !jobs.is_empty() {
                return Err(leyen_ipc::Error::Failed(
                    "A dependency operation is in progress; try again when it finishes"
                        .to_string(),
                ));
            }
            LaunchGuard::new()
        };
        let library = leyen_core::config::load_library().await.map_err(|e| {
            log::error!("LaunchGame: failed to read the library: {e}");
            leyen_ipc::Error::Failed(format!("Failed to read the game library: {e}"))
        })?;
        let Some((game, _group)) = find_game_by_leyen_id(&library, leyen_id) else {
            log::error!("LaunchGame: no game for leyen_id '{leyen_id}'");
            return Err(leyen_ipc::Error::Failed(format!(
                "No game with id '{leyen_id}'"
            )));
        };
        let game = game.clone();
        match leyen_core::launch::launch_game_headless(&game).await {
            Ok(_) => Ok(()),
            Err(e) => {
                log::error!(
                    target: &format!("game:{}", game.id),
                    "Launch of '{}' ({leyen_id}) failed: {e}",
                    game.title
                );
                Err(leyen_ipc::Error::Failed(e.to_string()))
            }
        }
    }

    async fn stop_game(&self, leyen_id: &str) -> Result<bool, leyen_ipc::Error> {
        let _work = ActivityGuard::new();
        self.touch();
        let library = leyen_core::config::load_library().await.map_err(|e| {
            log::error!("StopGame: failed to read the library: {e}");
            leyen_ipc::Error::Failed(format!("Failed to read the game library: {e}"))
        })?;
        let Some((game, _group)) = find_game_by_leyen_id(&library, leyen_id) else {
            return Ok(false);
        };
        let game_id = game.id.clone();
        leyen_core::launch::stop_game(&game_id)
            .await
            .map_err(|e| leyen_ipc::Error::Failed(e.to_string()))
    }

    async fn get_running_games(&self) -> Vec<leyen_ipc::RunningGameSnapshot> {
        let _work = ActivityGuard::new();
        self.touch();
        map_snapshots(leyen_core::launch::running_games_snapshot().await).await
    }

    async fn get_runtime_status(&self) -> RuntimeReadiness {
        let _work = ActivityGuard::new();
        self.touch();
        current_runtime_readiness().await
    }

    async fn get_logs(&self, since_offset: u64) -> (u64, Vec<leyen_ipc::LogEntry>) {
        let _work = ActivityGuard::new();
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
        let _work = ActivityGuard::new();
        self.touch();
        leyen_core::logging::clear_log_buffer();
    }

    async fn save_library(
        &self,
        toml_bytes: Vec<u8>,
        base_version: u64,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) -> Result<u64, leyen_ipc::Error> {
        let _work = ActivityGuard::new();
        self.touch();
        // Serialize saves: version check, write and bump must be atomic
        // against a concurrent SaveLibrary.
        let _save = self.save_lock.lock().await;
        let current = self.library_version.load(Ordering::SeqCst);
        if base_version != current {
            return Err(leyen_ipc::Error::StaleLibraryVersion(format!(
                "library version is {current}, the save was built against {base_version}; \
                 reload and retry"
            )));
        }
        let items = tokio::task::spawn_blocking(move || {
            let text = String::from_utf8(toml_bytes).map_err(|_| {
                warn!("SaveLibrary: payload is not valid UTF-8");
                leyen_ipc::Error::Failed("Library payload is not valid UTF-8".to_string())
            })?;
            toml::from_str::<leyen_model::models::GamesConfig>(&text)
                .map(|c| c.items)
                .map_err(|e| {
                    warn!("SaveLibrary: parse failed: {e}");
                    leyen_ipc::Error::Failed(format!("Library payload failed to parse: {e}"))
                })
        })
        .await
        .map_err(|e| leyen_ipc::Error::Failed(format!("Internal task error: {e}")))??;
        if !leyen_core::config::save_library_merged(items).await {
            return Err(leyen_ipc::Error::Failed(
                "Failed to persist the library".to_string(),
            ));
        }
        let new_version = self.library_version.fetch_add(1, Ordering::SeqCst) + 1;
        if let Err(e) = Manager::library_changed(&emitter, new_version).await {
            warn!("leyend: failed to emit LibraryChanged: {e}");
        }
        Ok(new_version)
    }

    async fn get_library_version(&self) -> u64 {
        let _work = ActivityGuard::new();
        self.touch();
        self.library_version.load(Ordering::SeqCst)
    }

    async fn reload_settings(&self) {
        let _work = ActivityGuard::new();
        self.touch();
        let settings = leyen_core::config::load_settings().await;
        leyen_core::logging::apply_log_settings(&settings);
        info!("leyend: settings reloaded");
    }

    async fn install_dep(
        &self,
        prefix: &str,
        dep_id: &str,
        proton_path: &str,
    ) -> Result<String, leyen_ipc::Error> {
        let _work = ActivityGuard::new();
        self.touch();
        let Some(conn) = self.connection() else {
            return Err(leyen_ipc::Error::Failed(
                "The daemon is still starting; try again shortly".to_string(),
            ));
        };
        let job_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            // Check-and-insert under one lock: `launch_game` checks `dep_jobs`
            // under the same lock, so a launch can't slip in between the
            // running check and the job becoming visible.
            let mut jobs = self.dep_jobs.lock().map_err(|_| poisoned_lock_error())?;
            if leyen_core::launch::is_any_game_running()
                || LAUNCHES_IN_FLIGHT.load(Ordering::SeqCst) > 0
            {
                return Err(leyen_ipc::Error::Failed(
                    "Cannot install dependencies while a game is running".to_string(),
                ));
            }
            if jobs.len() >= MAX_DEP_JOBS {
                return Err(leyen_ipc::Error::Failed(format!(
                    "Too many dependency operations in progress (max {MAX_DEP_JOBS})"
                )));
            }
            jobs.insert(job_id.clone(), cancel.clone());
        }
        self.spawn_dep_job(conn, job_id.clone(), prefix, dep_id, proton_path, cancel, true);
        Ok(job_id)
    }

    async fn uninstall_dep(
        &self,
        prefix: &str,
        dep_id: &str,
        proton_path: &str,
    ) -> Result<String, leyen_ipc::Error> {
        let _work = ActivityGuard::new();
        self.touch();
        let Some(conn) = self.connection() else {
            return Err(leyen_ipc::Error::Failed(
                "The daemon is still starting; try again shortly".to_string(),
            ));
        };
        let job_id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            // Same atomic check-and-insert as `install_dep`.
            let mut jobs = self.dep_jobs.lock().map_err(|_| poisoned_lock_error())?;
            if leyen_core::launch::is_any_game_running()
                || LAUNCHES_IN_FLIGHT.load(Ordering::SeqCst) > 0
            {
                return Err(leyen_ipc::Error::Failed(
                    "Cannot uninstall dependencies while a game is running".to_string(),
                ));
            }
            if jobs.len() >= MAX_DEP_JOBS {
                return Err(leyen_ipc::Error::Failed(format!(
                    "Too many dependency operations in progress (max {MAX_DEP_JOBS})"
                )));
            }
            jobs.insert(job_id.clone(), cancel.clone());
        }
        self.spawn_dep_job(conn, job_id.clone(), prefix, dep_id, proton_path, cancel, false);
        Ok(job_id)
    }

    async fn cancel_dep(&self, job_id: &str) -> bool {
        let _work = ActivityGuard::new();
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
        let _work = ActivityGuard::new();
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
        version: u64,
    ) -> zbus::Result<()>;
}

/// RAII guard on a `dep_jobs` entry: removes it on drop (panic-safe), like
/// `ActivityGuard`/`LaunchGuard`. `finish()` marks the completion as normal;
/// if dropped without it (the job's future panicked), the entry is still
/// cleared but a warning is logged.
struct DepJobGuard {
    jobs: DepJobs,
    job_id: String,
    finished: bool,
}

impl DepJobGuard {
    fn new(jobs: DepJobs, job_id: String) -> Self {
        DepJobGuard {
            jobs,
            job_id,
            finished: false,
        }
    }

    fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for DepJobGuard {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.remove(&self.job_id);
        }
        if !self.finished {
            warn!(
                "leyend: dep job '{}' ended without a normal finish (likely panicked); entry cleared",
                self.job_id
            );
        }
    }
}

impl Manager {
    /// Spawns a dependency install/uninstall job: progress → `DepProgress`,
    /// terminal result → `DepFinished`.
    #[allow(clippy::too_many_arguments)]
    fn spawn_dep_job(
        &self,
        conn: Connection,
        job_id: String,
        prefix: &str,
        dep_id: &str,
        proton_path: &str,
        cancel: Arc<AtomicBool>,
        install: bool,
    ) {
        let prefix = prefix.to_string();
        let dep_id = dep_id.to_string();
        let proton_path = proton_path.to_string();
        let jobs = self.dep_jobs.clone();
        // Hold a work token for the job's lifetime: a multi-minute winetricks
        // install must keep the idle-exit at bay even with no game running.
        let work = ActivityGuard::new();

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
                if let Err(e) =
                    emit_dep_progress(conn, job_id, prefix, dep_id, "running", fraction, &msg).await
                {
                    warn!("leyend: failed to emit DepProgress: {e}");
                }
            });
        };

        tokio::spawn(async move {
            let _work = work;
            let mut job_guard = DepJobGuard::new(jobs, job_id.clone());
            let result = if install {
                leyen_core::deps::install_dep(&dep_id, &prefix, &proton_path, cancel, on_progress)
                    .await
            } else {
                leyen_core::deps::uninstall_dep(
                    &dep_id,
                    &prefix,
                    &proton_path,
                    cancel,
                    on_progress,
                )
                .await
            };
            job_guard.finish();
            drop(job_guard);
            let (success, message) = match result {
                Ok(note) => (true, note.unwrap_or_default()),
                Err(e) => (false, e),
            };
            if let Err(e) = emit_dep_finished(&conn, &job_id, success, &message).await {
                warn!("leyend: failed to emit DepFinished: {e}");
            }
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
        library_version: Arc::new(AtomicU64::new(1)),
        save_lock: Arc::new(tokio::sync::Mutex::new(())),
    };
    let conn_holder = manager.conn.clone();
    let last_activity = manager.last_activity.clone();
    let dep_jobs = manager.dep_jobs.clone();

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

    // Detached engine tasks (deferred shared-container launches) hold a work
    // token so the idle-exit cannot kill the daemon mid-launch. The token also
    // carries a LaunchGuard: during the container wait nothing is registered
    // as running yet, and a dependency job slipping in would mutate the very
    // prefix the pending launch is about to use.
    leyen_core::launch::set_work_guard_source(|| {
        Box::new((ActivityGuard::new(), LaunchGuard::new()))
    });

    // Crash recovery: re-adopt live scopes, then start the one monitor.
    leyen_core::launch::reconcile_stale_sessions_on_startup().await;
    leyen_core::launch::start_running_sessions_monitor();

    // Runtime install off the request path; emit readiness as it changes.
    leyen_core::runtime::umu::check_or_install_umu().await;
    leyen_core::runtime::umu::check_or_install_winetricks().await;
    spawn_runtime_status_watcher(connection.clone());

    let shutdown = Arc::new(tokio::sync::Notify::new());
    spawn_idle_exit(last_activity, shutdown.clone());
    spawn_signal_handlers(shutdown.clone());

    // Serve until the idle-exit or a termination signal requests shutdown,
    // then tear down gracefully.
    shutdown.notified().await;
    graceful_shutdown(&connection, &dep_jobs).await;
    Ok(())
}

/// Graceful teardown: release the bus name first — a client call from here on
/// activates a fresh daemon instead of hitting a dying one — then cancel any
/// in-flight dependency jobs, drain in-flight work (bounded) and flush the
/// log thread.
async fn graceful_shutdown(connection: &Connection, dep_jobs: &DepJobs) {
    if let Err(e) = connection.release_name(leyen_ipc::BUS_NAME).await {
        warn!("leyend: failed to release bus name: {e}");
    }
    // Flip every in-flight dep job's cancel flag: `run_umu_command_inner`
    // notices within ~200ms and kills the process group, so this turns a
    // SIGTERM mid-install into a clean cancel instead of an orphaned child.
    let cancelled = dep_jobs
        .lock()
        .map(|jobs| {
            for cancel in jobs.values() {
                cancel.store(true, Ordering::Relaxed);
            }
            jobs.len()
        })
        .unwrap_or_default();
    if cancelled > 0 {
        info!("leyend: cancelling {cancelled} in-flight dependency job(s) for shutdown");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while ACTIVE_WORK.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let remaining = ACTIVE_WORK.load(Ordering::SeqCst);
    if remaining > 0 {
        warn!("leyend: exiting with {remaining} unit(s) of work still in flight");
    }
    // Last log line before the log thread is torn down — anything logged after
    // `shutdown()` is dropped.
    info!("leyend: shutdown complete");
    leyen_core::logging::shutdown();
}

/// Spawns a long-lived background task under a supervisor: a panic inside
/// `fut` is caught and logged instead of silently killing the loop forever
/// (the task itself never returns, so nothing else would ever notice).
fn spawn_supervised(name: &'static str, fut: impl std::future::Future<Output = ()> + Send + 'static) {
    tokio::spawn(async move {
        if let Err(e) = tokio::spawn(fut).await
            && e.is_panic()
        {
            error!("leyend: background task '{name}' panicked and stopped");
        }
    });
}

/// Requests shutdown on SIGTERM/SIGINT (systemd stop, session logout, Ctrl-C)
/// so scopes keep running but logs are flushed and the bus name is released
/// cleanly. Scopes survive the daemon; the next start re-adopts them.
fn spawn_signal_handlers(shutdown: Arc<tokio::sync::Notify>) {
    spawn_supervised("signal-handlers", async move {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!("leyend: failed to install SIGTERM handler: {e}");
                return;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                warn!("leyend: failed to install SIGINT handler: {e}");
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => info!("leyend: received SIGTERM, shutting down"),
            _ = int.recv() => info!("leyend: received SIGINT, shutting down"),
        }
        shutdown.notify_one();
    });
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
    spawn_supervised("sessions-relay", async move {
        // The engine publishes every monitor tick; only forward a snapshot
        // that differs from the last one sent so idle ticks stay off the bus.
        let mut last: Option<Vec<leyen_ipc::RunningGameSnapshot>> = None;
        while let Some(core) = sessions_rx.recv().await {
            let mapped = map_snapshots(core).await;
            if last.as_ref() == Some(&mapped) {
                continue;
            }
            last = Some(mapped.clone());
            if let Err(e) = conn
                .emit_signal(
                    Option::<&str>::None,
                    OBJECT_PATH,
                    INTERFACE,
                    "SessionsChanged",
                    &(mapped,),
                )
                .await
            {
                warn!("leyend: failed to emit SessionsChanged: {e}");
            }
        }
    });

    // LogsAppended: coalesce the per-line notifications (watch keeps only the
    // latest total) and throttle emissions.
    let (logs_tx, mut logs_rx) = tokio::sync::watch::channel(0u64);
    leyen_core::logging::set_logs_appended_listener(move |total| {
        let _ = logs_tx.send(total);
    });
    let conn = connection.clone();
    spawn_supervised("logs-relay", async move {
        while logs_rx.changed().await.is_ok() {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let total = *logs_rx.borrow_and_update();
            if let Err(e) = conn
                .emit_signal(
                    Option::<&str>::None,
                    OBJECT_PATH,
                    INTERFACE,
                    "LogsAppended",
                    &(total,),
                )
                .await
            {
                warn!("leyend: failed to emit LogsAppended: {e}");
            }
        }
    });
}

/// Emits `RuntimeStatus` whenever umu/winetricks readiness changes. Cheap
/// (readiness checks are cached) and not the per-game scanning we eliminated.
fn spawn_runtime_status_watcher(connection: Connection) {
    spawn_supervised("runtime-status-watcher", async move {
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
                if let Err(e) = connection
                    .emit_signal(
                        Option::<&str>::None,
                        OBJECT_PATH,
                        INTERFACE,
                        "RuntimeStatus",
                        &(now.umu_ready, now.winetricks_ready),
                    )
                    .await
                {
                    warn!("leyend: failed to emit RuntimeStatus: {e}");
                }
                last = Some(now);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

/// Requests shutdown when no game runs, no work is in flight and no request
/// has arrived for `IDLE_EXIT_SECONDS`. The daemon must outlive any running
/// game (it owns output capture + playtime finalize), so `is_any_game_running`
/// gates the timer; `ACTIVE_WORK` gates it for game-less work (dependency
/// jobs, deferred launches, in-flight method calls).
fn spawn_idle_exit(last_activity: Arc<Mutex<Instant>>, shutdown: Arc<tokio::sync::Notify>) {
    spawn_supervised("idle-exit", async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if leyen_core::launch::is_any_game_running()
                || ACTIVE_WORK.load(Ordering::SeqCst) > 0
            {
                continue;
            }
            let idle = last_activity
                .lock()
                .map(|t| t.elapsed())
                .unwrap_or_default();
            if idle >= Duration::from_secs(IDLE_EXIT_SECONDS) {
                info!("leyend: idle for {}s, exiting", idle.as_secs());
                shutdown.notify_one();
                return;
            }
        }
    });
}
