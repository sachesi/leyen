use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{OnceLock, RwLock};

use chrono::Local;
use crossbeam_channel::{Sender, unbounded};
use log::{Level, LevelFilter, Metadata, Record};
use serde::{Deserialize, Serialize};

use crate::config::get_config_dir;
use crate::models::GlobalSettings;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LogEntry {
    pub timestamp: String,
    pub line: String,
    pub game_id: Option<String>,
}

pub static LOG_ERRORS: AtomicBool = AtomicBool::new(true);
pub static LOG_WARNINGS: AtomicBool = AtomicBool::new(false);
pub static LOG_OPERATIONS: AtomicBool = AtomicBool::new(false);

static LOG_SENDER: OnceLock<Sender<LogEntry>> = OnceLock::new();
static UI_LOG_ENTRIES: OnceLock<RwLock<VecDeque<LogEntry>>> = OnceLock::new();
static TOTAL_LOG_LINES_PRODUCED: AtomicUsize = AtomicUsize::new(0);
const MAX_UI_LOGS: usize = 1000; // Reduced to 1000 for better GTK performance

fn log_path() -> PathBuf {
    get_config_dir().join("logs.jsonl")
}

struct LeyenLogger;

impl log::Log for LeyenLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
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
        let game_id = if target.starts_with("game:") {
            Some(target[5..].to_string())
        } else {
            None
        };

        let level_str = match record.level() {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };

        let message = record.args().to_string();
        let module = record.module_path().unwrap_or("unknown");

        let line = match &game_id {
            Some(id) => format!("[{level_str}] [{module}] [game:{id}] {message}"),
            None => format!("[{level_str}] [{module}] {message}"),
        };

        let entry = LogEntry {
            timestamp: Local::now().to_rfc3339(),
            line: line.clone(),
            game_id,
        };

        if let Some(tx) = LOG_SENDER.get() {
            let _ = tx.send(entry);
        }
    }

    fn flush(&self) {}
}

static LOGGER: LeyenLogger = LeyenLogger;

pub fn init() -> Result<(), log::SetLoggerError> {
    let _ = UI_LOG_ENTRIES.set(RwLock::new(VecDeque::with_capacity(MAX_UI_LOGS)));

    let path = log_path();
    let _ = fs::remove_file(&path);
    let mut old_path = path.clone();
    old_path.set_extension("jsonl.old");
    let _ = fs::remove_file(old_path);

    let (tx, rx) = unbounded::<LogEntry>();
    let _ = LOG_SENDER.set(tx);

    std::thread::spawn(move || {
        let path = log_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let mut lines_since_check = 0;

        while let Ok(entry) = rx.recv() {
            // Update memory buffer for UI in the background thread to avoid blocking log callers
            if let Some(buf) = UI_LOG_ENTRIES.get() {
                if let Ok(mut entries) = buf.write() {
                    if entries.len() >= MAX_UI_LOGS {
                        entries.pop_front();
                    }
                    entries.push_back(entry.clone());
                    TOTAL_LOG_LINES_PRODUCED.fetch_add(1, Ordering::SeqCst);
                }
            }

            lines_since_check += 1;
            if lines_since_check >= 100 {
                lines_since_check = 0;
                if let Ok(metadata) = fs::metadata(&path) {
                    if metadata.len() > MAX_LOG_SIZE {
                        file = None;
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
            }

            if file.is_none() {
                file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .ok();
            }

            if let Some(ref mut f) = file {
                if let Ok(json) = serde_json::to_string(&entry) {
                    let _ = writeln!(f, "{}", json);
                }
            }
        }
    });

    log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Trace))
}

const MAX_LOG_SIZE: u64 = 5 * 1024 * 1024;

pub fn apply_log_settings(s: &GlobalSettings) {
    LOG_ERRORS.store(s.log_errors, Ordering::Relaxed);
    LOG_WARNINGS.store(s.log_warnings, Ordering::Relaxed);
    LOG_OPERATIONS.store(s.log_operations, Ordering::Relaxed);
}

pub fn get_log_entry_count() -> usize {
    TOTAL_LOG_LINES_PRODUCED.load(Ordering::Relaxed)
}

pub fn get_log_entries() -> Vec<LogEntry> {
    UI_LOG_ENTRIES
        .get()
        .and_then(|buf| buf.read().ok())
        .map(|entries| entries.iter().cloned().collect())
        .unwrap_or_default()
}

pub fn clear_log_buffer() {
    if let Some(buf) = UI_LOG_ENTRIES.get() {
        if let Ok(mut entries) = buf.write() {
            entries.clear();
        }
    }
    TOTAL_LOG_LINES_PRODUCED.store(0, Ordering::SeqCst);

    let path = log_path();
    let _ = fs::remove_file(&path);
    let mut old_path = path.clone();
    old_path.set_extension("jsonl.old");
    let _ = fs::remove_file(old_path);
}
