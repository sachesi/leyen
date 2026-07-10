//! Process-wide logging + the in-memory log ring buffer.
//!
//! In the daemon this captures both Leyen's own operations log and every
//! game's stdout/stderr (piped in via `info!(target: "game:<id>")`). Clients
//! pull batches over D-Bus with [`get_logs_since`]; the daemon emits
//! `LogsAppended` from the [`set_logs_appended_listener`] hook.

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;

use chrono::Local;
use crossbeam_channel::{Sender, bounded};
use log::{Level, LevelFilter, Metadata, Record};
use serde::{Deserialize, Serialize};

use leyen_model::models::GlobalSettings;
use leyen_model::paths::get_config_dir;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LogEntry {
    pub timestamp: String,
    pub line: String,
    pub game_id: Option<String>,
}

pub static LOG_ERRORS: AtomicBool = AtomicBool::new(true);
pub static LOG_WARNINGS: AtomicBool = AtomicBool::new(false);
pub static LOG_OPERATIONS: AtomicBool = AtomicBool::new(false);

/// Set by `clear_log_buffer` after it unlinks `logs.jsonl`: the writer thread
/// holds an open fd to the (now phantom) inode and must reopen the path, or
/// every later line lands in an unreachable file for the rest of the process.
static LOG_FILE_REOPEN: AtomicBool = AtomicBool::new(false);

static LOG_SENDER: Mutex<Option<Sender<LogEntry>>> = Mutex::new(None);
static LOG_THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static UI_LOG_ENTRIES: OnceLock<RwLock<VecDeque<LogEntry>>> = OnceLock::new();
static TOTAL_LOG_LINES_PRODUCED: AtomicU64 = AtomicU64::new(0);
/// Lines dropped because the channel to the writer thread was full. Drained
/// into a single marker line the next time a send succeeds.
static DROPPED_LOG_LINES: AtomicU64 = AtomicU64::new(0);
const MAX_UI_LOGS: usize = 1000; // Reduced to 1000 for better GTK performance
/// Capacity of the channel feeding the writer thread. Bounded so a
/// log-flooding game (piped stdout/stderr) can't grow daemon RSS without
/// limit; `send_log_line` drops lines rather than blocking the caller once full.
const LOG_CHANNEL_CAPACITY: usize = 10_000;

/// Notified (with the new total offset) whenever a log line is appended. The
/// daemon installs a coalescing emitter here to drive the `LogsAppended` signal.
static LOGS_APPENDED_LISTENER: OnceLock<Box<dyn Fn(u64) + Send + Sync>> = OnceLock::new();

/// Installs the append listener. No-op if called more than once.
pub fn set_logs_appended_listener(listener: impl Fn(u64) + Send + Sync + 'static) {
    let _ = LOGS_APPENDED_LISTENER.set(Box::new(listener));
}

fn log_path() -> PathBuf {
    get_config_dir().join("logs.jsonl")
}

/// Sends `entry` on `tx` without blocking. A log call happens on arbitrary
/// caller threads (including game stdout/stderr readers in launch.rs); if the
/// writer thread falls behind and the channel fills, the caller must not
/// stall on it, so the line is dropped and counted instead. Once a later send
/// succeeds, one marker line reporting the drop count is emitted through the
/// same channel.
fn send_log_line(tx: &Sender<LogEntry>, entry: LogEntry) {
    if tx.try_send(entry).is_err() {
        DROPPED_LOG_LINES.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let dropped = DROPPED_LOG_LINES.swap(0, Ordering::Relaxed);
    if dropped > 0 {
        let marker = LogEntry {
            timestamp: Local::now().to_rfc3339(),
            line: format!("[WARN] {dropped} log lines dropped (writer backlogged)"),
            game_id: None,
        };
        if tx.try_send(marker).is_err() {
            DROPPED_LOG_LINES.fetch_add(dropped, Ordering::Relaxed);
        }
    }
}

struct LeyenLogger;

impl log::Log for LeyenLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Game-targeted lines (piped game output, launch lifecycle, launch
        // failures) are the product of the Logs window, not diagnostics —
        // always capture them regardless of the log settings.
        if metadata.target().starts_with("game:") {
            return true;
        }
        match metadata.level() {
            Level::Error => LOG_ERRORS.load(Ordering::Relaxed),
            Level::Warn => LOG_WARNINGS.load(Ordering::Relaxed),
            _ => LOG_OPERATIONS.load(Ordering::Relaxed),
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let target = record.target();
        let game_id = target.strip_prefix("game:").map(String::from);

        let level_str = match record.level() {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };

        let message = record.args().to_string();

        // Human-first lines: module paths are developer noise in a user-facing
        // log. Game-targeted INFO lines (piped game output, launch lifecycle)
        // already carry their own [Title:stream] context and need no tag at all;
        // everything else keeps a compact level tag.
        let line = if game_id.is_some() && record.level() == Level::Info {
            message
        } else {
            format!("[{level_str}] {message}")
        };

        let entry = LogEntry {
            timestamp: Local::now().to_rfc3339(),
            line,
            game_id,
        };

        if let Ok(sender) = LOG_SENDER.lock()
            && let Some(tx) = sender.as_ref()
        {
            send_log_line(tx, entry);
        }
    }

    fn flush(&self) {}
}

static LOGGER: LeyenLogger = LeyenLogger;

pub fn init() -> Result<(), log::SetLoggerError> {
    let _ = UI_LOG_ENTRIES.set(RwLock::new(VecDeque::with_capacity(MAX_UI_LOGS)));

    let path = log_path();
    let mut old_path = path.clone();
    old_path.set_extension("jsonl.old");
    if path.exists() {
        let _ = fs::rename(&path, &old_path);
    }

    let (tx, rx) = bounded::<LogEntry>(LOG_CHANNEL_CAPACITY);
    if let Ok(mut sender) = LOG_SENDER.lock() {
        *sender = Some(tx);
    }

    let handle = std::thread::spawn(move || {
        let path = log_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let mut lines_since_check = 0;
        let mut lines_since_sync = 0;

        while let Ok(entry) = rx.recv() {
            // Update memory buffer for the log pull API in the background thread
            // so log callers never block.
            if let Some(buf) = UI_LOG_ENTRIES.get()
                && let Ok(mut entries) = buf.write()
            {
                if entries.len() >= MAX_UI_LOGS {
                    entries.pop_front();
                }
                entries.push_back(entry.clone());
                let total = TOTAL_LOG_LINES_PRODUCED.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(listener) = LOGS_APPENDED_LISTENER.get() {
                    listener(total);
                }
            }

            lines_since_check += 1;
            if lines_since_check >= 100 {
                lines_since_check = 0;
                if let Ok(metadata) = fs::metadata(&path)
                    && metadata.len() > MAX_LOG_SIZE
                {
                    let mut old_path = path.clone();
                    old_path.set_extension("jsonl.old");
                    let _ = fs::rename(&path, &old_path);
                    file = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .ok();
                }
            }

            if LOG_FILE_REOPEN.swap(false, Ordering::Relaxed) {
                file = None;
            }

            if file.is_none() {
                file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .ok();
            }

            if let Some(ref mut f) = file
                && let Ok(json) = serde_json::to_string(&entry)
            {
                let _ = writeln!(f, "{}", json);
                // Batched durability: fsync every 50 lines under load, and on
                // queue quiescence so a crash right after a burst loses nothing.
                // Per-line sync would be too slow for chatty game output.
                lines_since_sync += 1;
                if lines_since_sync >= 50 || rx.is_empty() {
                    let _ = f.sync_all();
                    lines_since_sync = 0;
                }
            }
        }
        // Channel closed (shutdown): make the tail durable before exiting.
        if let Some(ref mut f) = file {
            let _ = f.sync_all();
        }
    });

    if let Ok(mut thread) = LOG_THREAD.lock() {
        *thread = Some(handle);
    }

    log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Trace))
}

const MAX_LOG_SIZE: u64 = 5 * 1024 * 1024;

pub fn apply_log_settings(s: &GlobalSettings) {
    LOG_ERRORS.store(s.log_errors, Ordering::Relaxed);
    LOG_WARNINGS.store(s.log_warnings, Ordering::Relaxed);
    LOG_OPERATIONS.store(s.log_operations, Ordering::Relaxed);
}

/// Total number of log lines ever produced (the monotonic pull offset).
pub fn get_log_entry_count() -> u64 {
    TOTAL_LOG_LINES_PRODUCED.load(Ordering::Relaxed)
}

/// All retained log entries (at most `MAX_UI_LOGS`).
pub fn get_log_entries() -> Vec<LogEntry> {
    UI_LOG_ENTRIES
        .get()
        .and_then(|buf| buf.read().ok())
        .map(|entries| entries.iter().cloned().collect())
        .unwrap_or_default()
}

/// Returns `(next_offset, entries)` for the batch of log lines produced at or
/// after `since_offset`. `next_offset` is the current total; pass it back on the
/// next call. Entries older than the retained ring (rotated out) are skipped —
/// the client resyncs from the oldest retained line.
pub fn get_logs_since(since_offset: u64) -> (u64, Vec<LogEntry>) {
    let total = TOTAL_LOG_LINES_PRODUCED.load(Ordering::Relaxed);
    let entries = match UI_LOG_ENTRIES.get().and_then(|buf| buf.read().ok()) {
        Some(guard) => guard,
        None => return (total, Vec::new()),
    };
    let skip = batch_skip(total, entries.len() as u64, since_offset);
    let batch = entries.iter().skip(skip).cloned().collect();
    (total, batch)
}

/// How many retained ring entries to skip to serve everything at or after
/// `since` offset. Entries older than the retained window (`total - len`) are
/// already gone, so a far-behind client resyncs from the oldest retained line.
fn batch_skip(total: u64, len: u64, since: u64) -> usize {
    let ring_start = total.saturating_sub(len);
    since.saturating_sub(ring_start).min(len) as usize
}

pub fn clear_log_buffer() {
    if let Some(buf) = UI_LOG_ENTRIES.get()
        && let Ok(mut entries) = buf.write()
    {
        entries.clear();
    }
    TOTAL_LOG_LINES_PRODUCED.store(0, Ordering::Relaxed);

    // Synchronous: two unlinks are cheap, and a detached thread could be killed
    // by daemon exit before the files are actually removed.
    let path = log_path();
    let _ = fs::remove_file(&path);
    let mut old_path = path.clone();
    old_path.set_extension("jsonl.old");
    let _ = fs::remove_file(old_path);
    // Unlinking doesn't invalidate the writer thread's open fd — tell it to
    // reopen so lines keep reaching the on-disk file.
    LOG_FILE_REOPEN.store(true, Ordering::Relaxed);
}

pub fn shutdown() {
    if let Ok(mut sender) = LOG_SENDER.lock() {
        *sender = None;
    }

    if let Ok(mut thread) = LOG_THREAD.lock()
        && let Some(handle) = thread.take()
    {
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::{DROPPED_LOG_LINES, LogEntry, batch_skip, send_log_line};
    use crossbeam_channel::bounded;
    use std::sync::atomic::Ordering;

    fn entry(line: &str) -> LogEntry {
        LogEntry {
            timestamp: String::new(),
            line: line.to_string(),
            game_id: None,
        }
    }

    #[test]
    fn send_log_line_drops_when_full_then_marks_on_recovery() {
        // Writer stalled/absent: nothing drains the channel.
        let (tx, rx) = bounded::<LogEntry>(2);
        DROPPED_LOG_LINES.store(0, Ordering::Relaxed);

        send_log_line(&tx, entry("a"));
        send_log_line(&tx, entry("b"));
        assert_eq!(DROPPED_LOG_LINES.load(Ordering::Relaxed), 0);

        // Channel full: these must not block and must be dropped + counted.
        send_log_line(&tx, entry("c"));
        send_log_line(&tx, entry("d"));
        assert_eq!(DROPPED_LOG_LINES.load(Ordering::Relaxed), 2);

        // Writer catches up, freeing room for the next send and its marker.
        rx.recv().unwrap();
        rx.recv().unwrap();
        send_log_line(&tx, entry("e"));
        assert_eq!(DROPPED_LOG_LINES.load(Ordering::Relaxed), 0);

        assert_eq!(rx.recv().unwrap().line, "e");
        let marker = rx.recv().unwrap();
        assert!(marker.line.contains("2 log lines dropped"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn batch_skip_offset_math() {
        // total=100, 1000-cap not hit so len=100, ring covers offsets [0,100).
        assert_eq!(batch_skip(100, 100, 0), 0); // from the start → skip none
        assert_eq!(batch_skip(100, 100, 60), 60); // caught up to 60 → skip 60
        assert_eq!(batch_skip(100, 100, 100), 100); // already current → empty batch
        assert_eq!(batch_skip(100, 100, 200), 100); // ahead (shouldn't happen) → empty

        // Ring rotated: 5000 produced, only last 1000 retained → ring_start=4000.
        assert_eq!(batch_skip(5000, 1000, 4000), 0); // oldest retained
        assert_eq!(batch_skip(5000, 1000, 4500), 500);
        assert_eq!(batch_skip(5000, 1000, 100), 0); // far behind → resync from oldest
        assert_eq!(batch_skip(5000, 1000, 5000), 1000); // current → empty
    }
}
