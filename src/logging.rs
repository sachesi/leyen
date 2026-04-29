use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

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

        eprintln!("[LEYEN] {line}");
    }

    fn flush(&self) {}
}

static LOGGER: LeyenLogger = LeyenLogger;

pub fn init() -> Result<(), log::SetLoggerError> {
    let (tx, rx) = unbounded::<LogEntry>();
    let _ = LOG_SENDER.set(tx);

    std::thread::spawn(move || {
        while let Ok(entry) = rx.recv() {
            write_log_entry(&entry);
        }
    });

    log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Trace))
}

const MAX_LOG_SIZE: u64 = 2 * 1024 * 1024; // 2MB

fn write_log_entry(entry: &LogEntry) {
    let path = log_path();
    
    // Check for rotation
    if let Ok(metadata) = fs::metadata(&path) {
        if metadata.len() > MAX_LOG_SIZE {
            let mut old_path = path.clone();
            old_path.set_extension("jsonl.old");
            let _ = fs::rename(&path, &old_path);
        }
    }

    if let Some(parent) = path.parent() {
        if !parent.exists() {
            let _ = fs::create_dir_all(parent);
        }
    }

    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(_) => return,
    };

    if let Ok(json) = serde_json::to_string(entry) {
        let _ = writeln!(file, "{}", json);
    }
}

pub fn apply_log_settings(s: &GlobalSettings) {
    LOG_ERRORS.store(s.log_errors, Ordering::Relaxed);
    LOG_WARNINGS.store(s.log_warnings, Ordering::Relaxed);
    LOG_OPERATIONS.store(s.log_operations, Ordering::Relaxed);
}

pub fn get_log_entries() -> Vec<LogEntry> {
    let path = log_path();
    let mut entries = Vec::new();

    // Try reading .old first if it exists
    let mut old_path = path.clone();
    old_path.set_extension("jsonl.old");
    if let Ok(file) = fs::File::open(&old_path) {
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                entries.push(entry);
            }
        }
    }

    if let Ok(file) = fs::File::open(&path) {
        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                entries.push(entry);
            }
        }
    }

    entries
}

pub fn clear_log_buffer() {
    let path = log_path();
    let _ = fs::remove_file(&path);
    let mut old_path = path.clone();
    old_path.set_extension("jsonl.old");
    let _ = fs::remove_file(old_path);
}
