//! The D-Bus contract for Leyen: shared `zvariant` types + the `zbus` proxy
//! trait. The daemon implements the matching `#[zbus::interface]`; clients (CLI,
//! GTK, and future Qt/COSMIC frontends) use the generated `LeyenProxy`.
//!
//! Bus name `com.github.sachesi.leyen`, object path `/com/github/sachesi/leyen`,
//! interface `com.github.sachesi.leyen.Manager`, on the **session** bus.

use serde::{Deserialize, Serialize};
use zvariant::Type;

/// Errors the daemon returns from method calls, carried as D-Bus errors
/// (`com.github.sachesi.leyen.Error.*`) so every client sees the reason instead
/// of a defaulted value. `StaleLibraryVersion` is matched by name on the client
/// side to drive the reload-and-retry flow.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "com.github.sachesi.leyen.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// Generic operation failure; the string is the human-readable cause.
    Failed(String),
    /// `SaveLibrary` was built against an outdated library version; reload and
    /// retry.
    StaleLibraryVersion(String),
}

/// The daemon's well-known bus name. Distinct from the GUI's GApplication id
/// (`com.github.sachesi.leyen`), which registers its own session-bus name for
/// single-instance — they must not collide.
pub const BUS_NAME: &str = "com.github.sachesi.leyen.Daemon";
pub const OBJECT_PATH: &str = "/com/github/sachesi/leyen";
pub const INTERFACE: &str = "com.github.sachesi.leyen.Manager";

/// A running game as seen by the daemon's single monitor. Wire signature
/// `(ssttt)`. `elapsed` is derived client-side from `started_at_epoch_seconds`
/// to avoid staleness. `game_id` is the internal UUID (used to match library
/// cards); `leyen_id` is the user-facing `ly-XXXX` (used for `StopGame`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct RunningGameSnapshot {
    pub game_id: String,
    pub leyen_id: String,
    pub pid: u64,
    pub started_at_epoch_seconds: u64,
    pub tracked_pid_count: u64,
}

/// A captured log line. Wire signature `(sss)`. `game_id` is empty for
/// non-game (operations) lines.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LogEntry {
    pub timestamp: String,
    pub line: String,
    pub game_id: String,
}

/// Runtime (umu/winetricks) readiness. Wire signature `(bb)`. (Named
/// `RuntimeReadiness` to avoid clashing with the generated `RuntimeStatus`
/// signal-args type.)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
pub struct RuntimeReadiness {
    pub umu_ready: bool,
    pub winetricks_ready: bool,
}

/// Installed-dependency ids tracked for a prefix. Wire signature `(as)`.
/// Convenience for non-local clients; the local GTK dialog reads prefix state
/// files directly.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DepStatus {
    pub installed: Vec<String>,
}

#[zbus::proxy(
    interface = "com.github.sachesi.leyen.Manager",
    default_service = "com.github.sachesi.leyen.Daemon",
    default_path = "/com/github/sachesi/leyen"
)]
pub trait Leyen {
    /// Launch a game by its `ly-XXXX` id. Errors carry the failure reason
    /// (unknown id, missing executable, launch failure).
    fn launch_game(&self, leyen_id: &str) -> zbus::Result<()>;

    /// Stop a running game by its `ly-XXXX` id. Returns whether it was running.
    fn stop_game(&self, leyen_id: &str) -> zbus::Result<bool>;

    /// Snapshot of currently running games.
    fn get_running_games(&self) -> zbus::Result<Vec<RunningGameSnapshot>>;

    /// Current umu/winetricks readiness.
    fn get_runtime_status(&self) -> zbus::Result<RuntimeReadiness>;

    /// Pull the batch of log lines produced at or after `since_offset`.
    /// Returns `(next_offset, entries)`.
    fn get_logs(&self, since_offset: u64) -> zbus::Result<(u64, Vec<LogEntry>)>;

    /// Clear the daemon's log ring buffer and on-disk log files.
    fn clear_logs(&self) -> zbus::Result<()>;

    /// Persist a TOML-encoded library (the only library writer). The daemon
    /// preserves its authoritative playtime/last-run fields. `base_version`
    /// must equal the current library version (see [`get_library_version`]) or
    /// the call fails with `StaleLibraryVersion` — a client editing a stale
    /// read must reload and retry instead of silently clobbering concurrent
    /// edits. Returns the new version.
    fn save_library(&self, toml_bytes: Vec<u8>, base_version: u64) -> zbus::Result<u64>;

    /// Current library version: bumped on every successful `SaveLibrary`.
    /// Session-scoped (resets when the daemon restarts).
    fn get_library_version(&self) -> zbus::Result<u64>;

    /// Re-read `settings.toml` and apply daemon-side settings (log gating).
    /// Clients call this after persisting settings.
    fn reload_settings(&self) -> zbus::Result<()>;

    /// Begin installing `dep_id` into `prefix` (using `proton_path`). Returns a
    /// job id; progress arrives via `DepProgress`, completion via `DepFinished`.
    fn install_dep(&self, prefix: &str, dep_id: &str, proton_path: &str) -> zbus::Result<String>;

    /// Begin uninstalling `dep_id` from `prefix`. Returns a job id.
    fn uninstall_dep(&self, prefix: &str, dep_id: &str, proton_path: &str) -> zbus::Result<String>;

    /// Request cancellation of an in-flight dependency job.
    fn cancel_dep(&self, job_id: &str) -> zbus::Result<bool>;

    /// Installed dependencies tracked for `prefix`.
    fn get_dep_status(&self, prefix: &str) -> zbus::Result<DepStatus>;

    /// Emitted whenever the running-game set changes.
    #[zbus(signal)]
    fn sessions_changed(&self, sessions: Vec<RunningGameSnapshot>) -> zbus::Result<()>;

    /// Emitted (coalesced) when new log lines are available; pull with `GetLogs`.
    #[zbus(signal)]
    fn logs_appended(&self, total_offset: u64) -> zbus::Result<()>;

    /// Dependency install/uninstall progress.
    #[zbus(signal)]
    fn dep_progress(
        &self,
        job_id: &str,
        prefix: &str,
        dep_id: &str,
        phase: &str,
        fraction: f64,
        msg: &str,
    ) -> zbus::Result<()>;

    /// Terminal result of a dependency job.
    #[zbus(signal)]
    fn dep_finished(&self, job_id: &str, success: bool, message: &str) -> zbus::Result<()>;

    /// Runtime (umu/winetricks) readiness changed.
    #[zbus(signal)]
    fn runtime_status(&self, umu_ready: bool, winetricks_ready: bool) -> zbus::Result<()>;

    /// The library was persisted at `version`; clients should re-read
    /// `games.toml` and adopt the version as their `SaveLibrary` base.
    #[zbus(signal)]
    fn library_changed(&self, version: u64) -> zbus::Result<()>;
}
