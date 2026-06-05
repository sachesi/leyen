//! The zbus↔glib bridge. The GTK thread has **no tokio runtime**: a dedicated
//! thread runs a current-thread tokio runtime that owns the `zbus::Connection`
//! and `LeyenProxy`. Commands flow glib→zbus over a channel with a per-call
//! response channel; daemon signals flow zbus→glib as [`DaemonEvent`]s drained
//! on the glib loop. Local-only work (file reads, icon extraction, desktop
//! entries) uses `gio::spawn_blocking`, never tokio.

use std::cell::RefCell;
use std::future::Future;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use futures_util::StreamExt;
use gtk4::glib;
use zbus::Connection;

use leyen_ipc::{LeyenProxy, LogEntry, RunningGameSnapshot, RuntimeReadiness};
use leyen_model::library::read_library_from_disk;
use leyen_model::models::{GlobalSettings, LibraryItem};

type Reply<T> = async_channel::Sender<T>;

/// Commands sent from the glib loop to the zbus thread. Replies carry the
/// daemon's failure reason — a D-Bus error must reach the user as an error,
/// never silently become a defaulted value.
enum DaemonCommand {
    Launch(String, Reply<Result<(), String>>),
    Stop(String, Reply<Result<bool, String>>),
    GetRunning(Reply<Result<Vec<RunningGameSnapshot>, String>>),
    GetRuntime(Reply<Result<RuntimeReadiness, String>>),
    GetLogs(u64, Reply<Result<(u64, Vec<LogEntry>), String>>),
    ClearLogs(Reply<Result<(), String>>),
    SaveLibrary(Vec<u8>, Reply<Result<(), String>>),
    ReloadSettings(Reply<Result<(), String>>),
    InstallDep {
        prefix: String,
        dep_id: String,
        proton: String,
        reply: Reply<Result<String, String>>,
    },
    UninstallDep {
        prefix: String,
        dep_id: String,
        proton: String,
        reply: Reply<Result<String, String>>,
    },
    CancelDep(String, Reply<Result<bool, String>>),
}

/// Daemon signals forwarded to the glib loop. Some fields mirror the D-Bus
/// contract but aren't consumed by the current GTK handlers (the log window
/// pulls by its own offset; the deps dialog tracks its own job).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum DaemonEvent {
    SessionsChanged(Vec<RunningGameSnapshot>),
    LogsAppended(u64),
    DepProgress {
        job_id: String,
        prefix: String,
        dep_id: String,
        phase: String,
        fraction: f64,
        msg: String,
    },
    DepFinished {
        job_id: String,
        success: bool,
        message: String,
    },
    RuntimeStatus {
        umu_ready: bool,
        winetricks_ready: bool,
    },
    LibraryChanged,
    /// A fresh daemon instance took over the bus name (idle-exit restart or
    /// crash recovery): client-side state (library version, running set,
    /// runtime status, log offsets) must resync.
    DaemonRestarted,
    /// A daemon operation failed; the string is the user-facing reason.
    Error(String),
}

static CMD_TX: OnceLock<async_channel::Sender<DaemonCommand>> = OnceLock::new();
/// Glib-side mirror of "is any game running", updated on every SessionsChanged.
static ANY_GAME_RUNNING: AtomicBool = AtomicBool::new(false);
/// The daemon's current library version (the `SaveLibrary` optimistic-
/// concurrency base). Updated by `LibraryChanged`, on connect and on daemon
/// restart; owned by the zbus thread.
static LIBRARY_VERSION: AtomicU64 = AtomicU64::new(1);

/// Timeout for read-only daemon queries.
const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Timeout for state-changing daemon calls (launch can do real work inline).
const ACTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The daemon's error message from a failed method call, without the D-Bus
/// error-name noise.
fn dbus_error_message(err: &zbus::Error) -> String {
    match err {
        zbus::Error::MethodError(_, Some(msg), _) => msg.clone(),
        other => other.to_string(),
    }
}

/// Awaits a proxy call with a timeout; failures are logged and returned as the
/// user-facing message. A wedged daemon must never hang a UI reply forever.
async fn bounded<T>(
    what: &str,
    timeout: std::time::Duration,
    fut: impl Future<Output = zbus::Result<T>>,
) -> Result<T, String> {
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => {
            log::error!("{what} failed: {e}");
            Err(dbus_error_message(&e))
        }
        Err(_) => {
            log::error!("{what} timed out after {timeout:?}");
            Err(format!("{what} timed out"))
        }
    }
}

/// Starts the bridge thread and returns the event receiver (drain it on the glib
/// loop). Call once at startup, before building the UI.
pub fn start() -> async_channel::Receiver<DaemonEvent> {
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<DaemonCommand>();
    let (evt_tx, evt_rx) = async_channel::unbounded::<DaemonEvent>();
    let _ = CMD_TX.set(cmd_tx);

    std::thread::Builder::new()
        .name("leyen-zbus".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("zbus thread: failed to build runtime: {e}");
                    return;
                }
            };
            rt.block_on(bridge_main(cmd_rx, evt_tx));
        })
        .expect("failed to spawn zbus bridge thread");

    evt_rx
}

async fn bridge_main(
    cmd_rx: async_channel::Receiver<DaemonCommand>,
    evt_tx: async_channel::Sender<DaemonEvent>,
) {
    // Retry with backoff instead of dying: an early return here used to drop
    // `cmd_rx`, silently turning every later daemon call into a no-op default.
    // Commands queue while we reconnect.
    let mut backoff = std::time::Duration::from_secs(1);
    let mut reported = false;
    let (connection, proxy) = loop {
        match Connection::session().await {
            Ok(connection) => match LeyenProxy::new(&connection).await {
                Ok(proxy) => break (connection, proxy),
                Err(e) => log::error!("zbus thread: failed to build proxy: {e}"),
            },
            Err(e) => log::error!("zbus thread: failed to connect to session bus: {e}"),
        }
        if !reported {
            reported = true;
            let _ = evt_tx
                .send(DaemonEvent::Error(
                    "Cannot reach the session bus; daemon features unavailable until it returns"
                        .to_string(),
                ))
                .await;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(std::time::Duration::from_secs(30));
    };

    // Adopt the daemon's current library version (best-effort; a failure means
    // the daemon isn't up yet — the restart watch below resyncs on activation).
    if let Ok(Ok(version)) =
        tokio::time::timeout(QUERY_TIMEOUT, proxy.get_library_version()).await
    {
        LIBRARY_VERSION.store(version, Ordering::SeqCst);
    }

    spawn_signal_forwarders(&proxy, evt_tx.clone());
    spawn_restart_watch(&connection, &proxy, evt_tx.clone());

    while let Ok(cmd) = cmd_rx.recv().await {
        let proxy = proxy.clone();
        let evt_tx = evt_tx.clone();
        // Handle concurrently so a slow call (e.g. LaunchGame) doesn't stall others.
        tokio::spawn(async move { handle_command(&proxy, &evt_tx, cmd).await });
    }
}

/// Watches `NameOwnerChanged` for the daemon's well-known name. The zbus signal
/// streams themselves survive a daemon restart (the bus matches on the
/// well-known name and zbus tracks the owner), but client-side *state* — the
/// library version, the running set, log offsets — must resync against the
/// fresh instance.
fn spawn_restart_watch(
    connection: &Connection,
    proxy: &LeyenProxy<'static>,
    evt_tx: async_channel::Sender<DaemonEvent>,
) {
    let connection = connection.clone();
    let proxy = proxy.clone();
    tokio::spawn(async move {
        let dbus = match zbus::fdo::DBusProxy::new(&connection).await {
            Ok(p) => p,
            Err(e) => {
                log::error!("zbus thread: failed to build DBus proxy for restart watch: {e}");
                return;
            }
        };
        let mut stream = match dbus
            .receive_name_owner_changed_with_args(&[(0, leyen_ipc::BUS_NAME)])
            .await
        {
            Ok(s) => s,
            Err(e) => {
                log::error!("zbus thread: failed to watch daemon name owner: {e}");
                return;
            }
        };
        while let Some(signal) = stream.next().await {
            let Ok(args) = signal.args() else { continue };
            if args.new_owner().is_none() {
                continue;
            }
            // The fresh instance may still be starting; retry the version
            // fetch briefly. A silently-missed resync would make every
            // subsequent SaveLibrary fail stale until manual recovery.
            let mut synced = false;
            for _ in 0..3 {
                if let Ok(Ok(version)) =
                    tokio::time::timeout(QUERY_TIMEOUT, proxy.get_library_version()).await
                {
                    LIBRARY_VERSION.store(version, Ordering::SeqCst);
                    synced = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            if !synced {
                log::warn!(
                    "daemon restarted but the library version resync failed; \
                     the next save may be rejected as stale"
                );
            }
            let _ = evt_tx.send(DaemonEvent::DaemonRestarted).await;
        }
    });
}

async fn handle_command(
    proxy: &LeyenProxy<'_>,
    evt_tx: &async_channel::Sender<DaemonEvent>,
    cmd: DaemonCommand,
) {
    match cmd {
        DaemonCommand::Launch(id, reply) => {
            let result = bounded("LaunchGame", ACTION_TIMEOUT, proxy.launch_game(&id)).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::Stop(id, reply) => {
            let result = bounded("StopGame", ACTION_TIMEOUT, proxy.stop_game(&id)).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::GetRunning(reply) => {
            let result =
                bounded("GetRunningGames", QUERY_TIMEOUT, proxy.get_running_games()).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::GetRuntime(reply) => {
            let result =
                bounded("GetRuntimeStatus", QUERY_TIMEOUT, proxy.get_runtime_status()).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::GetLogs(offset, reply) => {
            let result = bounded("GetLogs", QUERY_TIMEOUT, proxy.get_logs(offset)).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::ClearLogs(reply) => {
            let result = bounded("ClearLogs", QUERY_TIMEOUT, proxy.clear_logs()).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::SaveLibrary(bytes, reply) => {
            let result = save_library_versioned(proxy, evt_tx, bytes).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::ReloadSettings(reply) => {
            let result =
                bounded("ReloadSettings", QUERY_TIMEOUT, proxy.reload_settings()).await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::InstallDep {
            prefix,
            dep_id,
            proton,
            reply,
        } => {
            let result = bounded(
                "InstallDep",
                ACTION_TIMEOUT,
                proxy.install_dep(&prefix, &dep_id, &proton),
            )
            .await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::UninstallDep {
            prefix,
            dep_id,
            proton,
            reply,
        } => {
            let result = bounded(
                "UninstallDep",
                ACTION_TIMEOUT,
                proxy.uninstall_dep(&prefix, &dep_id, &proton),
            )
            .await;
            let _ = reply.send(result).await;
        }
        DaemonCommand::CancelDep(job, reply) => {
            let result = bounded("CancelDep", QUERY_TIMEOUT, proxy.cancel_dep(&job)).await;
            let _ = reply.send(result).await;
        }
    }
}

/// `SaveLibrary` with the optimistic-concurrency version. On a stale-version
/// rejection the library changed under us (another client saved): adopt the
/// daemon's current version, nudge the UI to reload, and surface the error so
/// the user can redo the edit against fresh state.
async fn save_library_versioned(
    proxy: &LeyenProxy<'_>,
    evt_tx: &async_channel::Sender<DaemonEvent>,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let base_version = LIBRARY_VERSION.load(Ordering::SeqCst);
    match tokio::time::timeout(ACTION_TIMEOUT, proxy.save_library(bytes, base_version)).await {
        Ok(Ok(new_version)) => {
            LIBRARY_VERSION.store(new_version, Ordering::SeqCst);
            Ok(())
        }
        Ok(Err(e)) => {
            log::error!("SaveLibrary failed: {e}");
            if let zbus::Error::MethodError(name, _, _) = &e
                && name.as_str() == "com.github.sachesi.leyen.Error.StaleLibraryVersion"
            {
                match tokio::time::timeout(QUERY_TIMEOUT, proxy.get_library_version()).await {
                    Ok(Ok(version)) => LIBRARY_VERSION.store(version, Ordering::SeqCst),
                    _ => log::warn!(
                        "library version refetch after a stale save failed; \
                         the next save may be rejected as stale again"
                    ),
                }
                // Reload the UI from disk regardless — its model is stale
                // either way.
                let _ = evt_tx.send(DaemonEvent::LibraryChanged).await;
            }
            Err(dbus_error_message(&e))
        }
        Err(_) => {
            log::error!("SaveLibrary timed out after {ACTION_TIMEOUT:?}");
            Err("SaveLibrary timed out".to_string())
        }
    }
}

fn spawn_signal_forwarders(proxy: &LeyenProxy<'static>, evt_tx: async_channel::Sender<DaemonEvent>) {
    // SessionsChanged
    {
        let proxy = proxy.clone();
        let tx = evt_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut stream) = proxy.receive_sessions_changed().await {
                while let Some(sig) = stream.next().await {
                    if let Ok(args) = sig.args() {
                        let _ = tx.send(DaemonEvent::SessionsChanged(args.sessions)).await;
                    }
                }
            }
        });
    }
    // LogsAppended
    {
        let proxy = proxy.clone();
        let tx = evt_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut stream) = proxy.receive_logs_appended().await {
                while let Some(sig) = stream.next().await {
                    if let Ok(args) = sig.args() {
                        let _ = tx.send(DaemonEvent::LogsAppended(args.total_offset)).await;
                    }
                }
            }
        });
    }
    // DepProgress
    {
        let proxy = proxy.clone();
        let tx = evt_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut stream) = proxy.receive_dep_progress().await {
                while let Some(sig) = stream.next().await {
                    if let Ok(a) = sig.args() {
                        let _ = tx
                            .send(DaemonEvent::DepProgress {
                                job_id: a.job_id.to_string(),
                                prefix: a.prefix.to_string(),
                                dep_id: a.dep_id.to_string(),
                                phase: a.phase.to_string(),
                                fraction: a.fraction,
                                msg: a.msg.to_string(),
                            })
                            .await;
                    }
                }
            }
        });
    }
    // DepFinished
    {
        let proxy = proxy.clone();
        let tx = evt_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut stream) = proxy.receive_dep_finished().await {
                while let Some(sig) = stream.next().await {
                    if let Ok(a) = sig.args() {
                        let _ = tx
                            .send(DaemonEvent::DepFinished {
                                job_id: a.job_id.to_string(),
                                success: a.success,
                                message: a.message.to_string(),
                            })
                            .await;
                    }
                }
            }
        });
    }
    // RuntimeStatus
    {
        let proxy = proxy.clone();
        let tx = evt_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut stream) = proxy.receive_runtime_status().await {
                while let Some(sig) = stream.next().await {
                    if let Ok(a) = sig.args() {
                        let _ = tx
                            .send(DaemonEvent::RuntimeStatus {
                                umu_ready: a.umu_ready,
                                winetricks_ready: a.winetricks_ready,
                            })
                            .await;
                    }
                }
            }
        });
    }
    // LibraryChanged
    {
        let proxy = proxy.clone();
        let tx = evt_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut stream) = proxy.receive_library_changed().await {
                while let Some(sig) = stream.next().await {
                    if let Ok(args) = sig.args() {
                        LIBRARY_VERSION.store(args.version, Ordering::SeqCst);
                    }
                    let _ = tx.send(DaemonEvent::LibraryChanged).await;
                }
            }
        });
    }
}

async fn call<T: Send + 'static>(make: impl FnOnce(Reply<T>) -> DaemonCommand) -> Option<T> {
    let (tx, rx) = async_channel::bounded(1);
    CMD_TX.get()?.send(make(tx)).await.ok()?;
    rx.recv().await.ok()
}

// ── Glib-side API (drop-in replacements for the old in-process engine) ──────

/// Reads `games.toml` directly (read-only) off the glib thread.
pub async fn load_library() -> Result<Vec<LibraryItem>, String> {
    gio_blocking(read_library_from_disk).await
}

/// Reads settings directly (read-only). Settings are client-written.
pub async fn load_settings() -> GlobalSettings {
    gio_blocking(leyen_model::settings::load_settings).await
}

/// Persists settings directly (client owns settings.toml), then tells the
/// daemon to re-read them so daemon-side settings (log gating) apply at once
/// instead of on its next restart.
pub async fn save_settings(settings: GlobalSettings) {
    let _ = gio_blocking(move || leyen_model::settings::save_settings(&settings)).await;
    if let Some(Err(e)) = call(DaemonCommand::ReloadSettings).await {
        log::error!("save_settings: daemon settings reload failed: {e}");
    }
}

/// Persists the library via the daemon (the only library writer). An `Err`
/// carries the user-facing reason; a stale-version rejection has already
/// triggered a UI reload by the time it surfaces here.
pub async fn save_library(items: Vec<LibraryItem>) -> Result<(), String> {
    let bytes = match leyen_model::library::serialize_library(&items) {
        Ok(text) => text.into_bytes(),
        Err(e) => {
            log::error!("save_library: serialize failed: {e}");
            return Err(e);
        }
    };
    call(|tx| DaemonCommand::SaveLibrary(bytes, tx))
        .await
        .unwrap_or_else(|| Err(bridge_down_message()))
}

pub async fn launch_game(leyen_id: &str) -> Result<(), String> {
    call(|tx| DaemonCommand::Launch(leyen_id.to_string(), tx))
        .await
        .unwrap_or_else(|| Err(bridge_down_message()))
}

pub async fn stop_game(leyen_id: &str) -> Result<bool, String> {
    call(|tx| DaemonCommand::Stop(leyen_id.to_string(), tx))
        .await
        .unwrap_or_else(|| Err(bridge_down_message()))
}

pub async fn running_games_snapshot() -> Vec<RunningGameSnapshot> {
    call(DaemonCommand::GetRunning)
        .await
        .and_then(Result::ok)
        .unwrap_or_default()
}

pub async fn get_runtime_status() -> RuntimeReadiness {
    call(DaemonCommand::GetRuntime)
        .await
        .and_then(Result::ok)
        .unwrap_or(RuntimeReadiness {
            umu_ready: false,
            winetricks_ready: false,
        })
}

pub async fn get_logs(since_offset: u64) -> (u64, Vec<LogEntry>) {
    call(|tx| DaemonCommand::GetLogs(since_offset, tx))
        .await
        .and_then(Result::ok)
        .unwrap_or((since_offset, Vec::new()))
}

pub async fn clear_logs() {
    let _ = call(DaemonCommand::ClearLogs).await;
}

pub async fn install_dep(prefix: &str, dep_id: &str, proton: &str) -> Result<String, String> {
    call(|reply| DaemonCommand::InstallDep {
        prefix: prefix.to_string(),
        dep_id: dep_id.to_string(),
        proton: proton.to_string(),
        reply,
    })
    .await
    .unwrap_or_else(|| Err(bridge_down_message()))
}

pub async fn uninstall_dep(prefix: &str, dep_id: &str, proton: &str) -> Result<String, String> {
    call(|reply| DaemonCommand::UninstallDep {
        prefix: prefix.to_string(),
        dep_id: dep_id.to_string(),
        proton: proton.to_string(),
        reply,
    })
    .await
    .unwrap_or_else(|| Err(bridge_down_message()))
}

pub async fn cancel_dep(job_id: &str) -> bool {
    call(|tx| DaemonCommand::CancelDep(job_id.to_string(), tx))
        .await
        .and_then(Result::ok)
        .unwrap_or(false)
}

/// The error every action call collapses to when the bridge itself is gone
/// (no command channel / reply dropped) rather than the daemon failing.
fn bridge_down_message() -> String {
    log::error!("daemon bridge unavailable: command could not be delivered");
    "The daemon connection is unavailable".to_string()
}

/// Lock-free "is any game running", updated by the SessionsChanged handler.
pub fn is_any_game_running() -> bool {
    ANY_GAME_RUNNING.load(Ordering::Relaxed)
}

// ── Glib-side event fan-out ─────────────────────────────────────────────────
// `async_channel` receivers steal (not broadcast), so a single bridge receiver
// is fanned out to per-component subscribers on the (single-threaded) glib loop.

thread_local! {
    static SUBSCRIBERS: RefCell<Vec<async_channel::Sender<DaemonEvent>>> =
        const { RefCell::new(Vec::new()) };
}

/// Returns a fresh receiver of daemon events. Each UI component (main window,
/// log window, deps dialog, running-games window) gets its own; closed ones are
/// pruned automatically. Call on the glib thread.
pub fn subscribe_events() -> async_channel::Receiver<DaemonEvent> {
    let (tx, rx) = async_channel::unbounded();
    SUBSCRIBERS.with(|s| s.borrow_mut().push(tx));
    rx
}

/// Drains the bridge receiver on the glib loop and broadcasts each event to all
/// subscribers, keeping `ANY_GAME_RUNNING` in sync. Call once at startup.
pub fn run_event_dispatch(evt_rx: async_channel::Receiver<DaemonEvent>) {
    glib::spawn_future_local(async move {
        while let Ok(evt) = evt_rx.recv().await {
            if let DaemonEvent::SessionsChanged(sessions) = &evt {
                ANY_GAME_RUNNING.store(!sessions.is_empty(), Ordering::Relaxed);
            }
            SUBSCRIBERS.with(|subs| {
                subs.borrow_mut()
                    .retain(|tx| tx.try_send(evt.clone()).is_ok());
            });
        }
    });
}

/// Runs `f` on a throwaway thread and awaits the result on the glib loop. Used
/// for local blocking work (file reads, icon extraction, desktop entries) since
/// the GTK thread has no tokio runtime. Infrequent, so a per-call thread is fine.
pub async fn gio_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx.recv().await.expect("blocking task dropped before completion")
}
