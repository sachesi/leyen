//! The zbus↔glib bridge. The GTK thread has **no tokio runtime**: a dedicated
//! thread runs a current-thread tokio runtime that owns the `zbus::Connection`
//! and `LeyenProxy`. Commands flow glib→zbus over a channel with a per-call
//! response channel; daemon signals flow zbus→glib as [`DaemonEvent`]s drained
//! on the glib loop. Local-only work (file reads, icon extraction, desktop
//! entries) uses `gio::spawn_blocking`, never tokio.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use gtk4::glib;
use zbus::Connection;

use leyen_ipc::{LeyenProxy, LogEntry, RunningGameSnapshot, RuntimeReadiness};
use leyen_model::library::read_library_from_disk;
use leyen_model::models::{GlobalSettings, LibraryItem};

type Reply<T> = async_channel::Sender<T>;

/// Commands sent from the glib loop to the zbus thread.
enum DaemonCommand {
    Launch(String, Reply<bool>),
    Stop(String, Reply<bool>),
    GetRunning(Reply<Vec<RunningGameSnapshot>>),
    GetRuntime(Reply<RuntimeReadiness>),
    GetLogs(u64, Reply<(u64, Vec<LogEntry>)>),
    ClearLogs(Reply<()>),
    SaveLibrary(Vec<u8>, Reply<bool>),
    InstallDep {
        prefix: String,
        dep_id: String,
        proton: String,
        reply: Reply<String>,
    },
    UninstallDep {
        prefix: String,
        dep_id: String,
        proton: String,
        reply: Reply<String>,
    },
    CancelDep(String, Reply<bool>),
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
}

static CMD_TX: OnceLock<async_channel::Sender<DaemonCommand>> = OnceLock::new();
/// Glib-side mirror of "is any game running", updated on every SessionsChanged.
static ANY_GAME_RUNNING: AtomicBool = AtomicBool::new(false);

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
    let connection = match Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("zbus thread: failed to connect to session bus: {e}");
            return;
        }
    };
    let proxy = match LeyenProxy::new(&connection).await {
        Ok(p) => p,
        Err(e) => {
            log::error!("zbus thread: failed to build proxy: {e}");
            return;
        }
    };

    spawn_signal_forwarders(&proxy, evt_tx);

    while let Ok(cmd) = cmd_rx.recv().await {
        let proxy = proxy.clone();
        // Handle concurrently so a slow call (e.g. LaunchGame) doesn't stall others.
        tokio::spawn(async move { handle_command(&proxy, cmd).await });
    }
}

async fn handle_command(proxy: &LeyenProxy<'_>, cmd: DaemonCommand) {
    match cmd {
        DaemonCommand::Launch(id, reply) => {
            let _ = reply.send(proxy.launch_game(&id).await.unwrap_or(false)).await;
        }
        DaemonCommand::Stop(id, reply) => {
            let _ = reply.send(proxy.stop_game(&id).await.unwrap_or(false)).await;
        }
        DaemonCommand::GetRunning(reply) => {
            let _ = reply
                .send(proxy.get_running_games().await.unwrap_or_default())
                .await;
        }
        DaemonCommand::GetRuntime(reply) => {
            let status = proxy.get_runtime_status().await.unwrap_or(RuntimeReadiness {
                umu_ready: false,
                winetricks_ready: false,
            });
            let _ = reply.send(status).await;
        }
        DaemonCommand::GetLogs(offset, reply) => {
            let _ = reply
                .send(proxy.get_logs(offset).await.unwrap_or((offset, Vec::new())))
                .await;
        }
        DaemonCommand::ClearLogs(reply) => {
            let _ = proxy.clear_logs().await;
            let _ = reply.send(()).await;
        }
        DaemonCommand::SaveLibrary(bytes, reply) => {
            let _ = reply
                .send(proxy.save_library(bytes).await.unwrap_or(false))
                .await;
        }
        DaemonCommand::InstallDep {
            prefix,
            dep_id,
            proton,
            reply,
        } => {
            let job = proxy
                .install_dep(&prefix, &dep_id, &proton)
                .await
                .unwrap_or_default();
            let _ = reply.send(job).await;
        }
        DaemonCommand::UninstallDep {
            prefix,
            dep_id,
            proton,
            reply,
        } => {
            let job = proxy
                .uninstall_dep(&prefix, &dep_id, &proton)
                .await
                .unwrap_or_default();
            let _ = reply.send(job).await;
        }
        DaemonCommand::CancelDep(job, reply) => {
            let _ = reply.send(proxy.cancel_dep(&job).await.unwrap_or(false)).await;
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
                while stream.next().await.is_some() {
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

/// Persists settings directly (client owns settings.toml).
pub async fn save_settings(settings: GlobalSettings) {
    let _ = gio_blocking(move || leyen_model::settings::save_settings(&settings)).await;
}

/// Persists the library via the daemon (the only library writer).
pub async fn save_library(items: Vec<LibraryItem>) -> bool {
    let bytes = match leyen_model::library::serialize_library(&items) {
        Ok(text) => text.into_bytes(),
        Err(e) => {
            log::error!("save_library: serialize failed: {e}");
            return false;
        }
    };
    call(|tx| DaemonCommand::SaveLibrary(bytes, tx))
        .await
        .unwrap_or(false)
}

pub async fn launch_game(leyen_id: &str) -> bool {
    call(|tx| DaemonCommand::Launch(leyen_id.to_string(), tx))
        .await
        .unwrap_or(false)
}

pub async fn stop_game(leyen_id: &str) -> bool {
    call(|tx| DaemonCommand::Stop(leyen_id.to_string(), tx))
        .await
        .unwrap_or(false)
}

pub async fn running_games_snapshot() -> Vec<RunningGameSnapshot> {
    call(DaemonCommand::GetRunning).await.unwrap_or_default()
}

pub async fn get_runtime_status() -> RuntimeReadiness {
    call(DaemonCommand::GetRuntime)
        .await
        .unwrap_or(RuntimeReadiness {
            umu_ready: false,
            winetricks_ready: false,
        })
}

pub async fn get_logs(since_offset: u64) -> (u64, Vec<LogEntry>) {
    call(|tx| DaemonCommand::GetLogs(since_offset, tx))
        .await
        .unwrap_or((since_offset, Vec::new()))
}

pub async fn clear_logs() {
    let _ = call(DaemonCommand::ClearLogs).await;
}

pub async fn install_dep(prefix: &str, dep_id: &str, proton: &str) -> String {
    call(|reply| DaemonCommand::InstallDep {
        prefix: prefix.to_string(),
        dep_id: dep_id.to_string(),
        proton: proton.to_string(),
        reply,
    })
    .await
    .unwrap_or_default()
}

pub async fn uninstall_dep(prefix: &str, dep_id: &str, proton: &str) -> String {
    call(|reply| DaemonCommand::UninstallDep {
        prefix: prefix.to_string(),
        dep_id: dep_id.to_string(),
        proton: proton.to_string(),
        reply,
    })
    .await
    .unwrap_or_default()
}

pub async fn cancel_dep(job_id: &str) -> bool {
    call(|tx| DaemonCommand::CancelDep(job_id.to_string(), tx))
        .await
        .unwrap_or(false)
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
