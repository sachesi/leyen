use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use log::{Level, LevelFilter, Metadata, Record};
use serde::{Deserialize, Serialize};

use crate::config::get_config_dir;
use crate::models::GlobalSettings;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LogEntry {
    pub line: String,
    pub game_id: Option<String>,
}

/// Atomic flags mirroring GlobalSettings.log_* so background threads can log
/// without reading the settings file on every message.
pub static LOG_ERRORS: AtomicBool = AtomicBool::new(true);
pub static LOG_WARNINGS: AtomicBool = AtomicBool::new(false);
pub static LOG_OPERATIONS: AtomicBool = AtomicBool::new(false);

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
        let line = match &game_id {
            Some(id) => format!("[LEYEN] [{level_str}] [game:{id}] {message}"),
            None => format!("[LEYEN] [{level_str}] {message}"),
        };

        append_log_entry(&LogEntry {
            line: line.clone(),
            game_id,
        });

        eprintln!("{line}");
    }

    fn flush(&self) {}
}

static LOGGER: LeyenLogger = LeyenLogger;

pub fn init() -> Result<(), log::SetLoggerError> {
    log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Trace))
}

fn append_log_entry(entry: &LogEntry) {
    let path = log_path();
    if let Some(parent) = path.parent()
        && !parent.exists()
        && let Err(e) = fs::create_dir_all(parent)
    {
        eprintln!(
            "Failed to create log directory '{}': {}",
            parent.display(),
            e
        );
        return;
    }

    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("Failed to open log file '{}': {}", path.display(), e);
            return;
        }
    };

    let flock_result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if flock_result != 0 {
        eprintln!(
            "Failed to lock log file '{}': {}",
            path.display(),
            std::io::Error::last_os_error()
        );
        return;
    }

    let write_result = serde_json::to_writer(&mut file, entry)
        .map_err(|e| std::io::Error::other(e.to_string()))
        .and_then(|_| writeln!(&mut file));

    let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };

    if let Err(e) = write_result {
        eprintln!("Failed to write log file '{}': {}", path.display(), e);
    }
}

pub fn apply_log_settings(s: &GlobalSettings) {
    LOG_ERRORS.store(s.log_errors, Ordering::Relaxed);
    LOG_WARNINGS.store(s.log_warnings, Ordering::Relaxed);
    LOG_OPERATIONS.store(s.log_operations, Ordering::Relaxed);
}

/// Return a snapshot of every log line captured so far.
pub fn get_log_entries() -> Vec<LogEntry> {
    let path = log_path();
    let Ok(file) = OpenOptions::new().read(true).open(&path) else {
        return Vec::new();
    };

    let reader = BufReader::new(file);
    reader
        .lines()
        .filter_map(|line| match line {
            Ok(line) if !line.trim().is_empty() => serde_json::from_str::<LogEntry>(&line).ok(),
            Ok(_) => None,
            Err(_) => None,
        })
        .collect()
}

/// Clear every captured log line from the in-memory buffer.
pub fn clear_log_buffer() {
    let path = log_path();
    let _ = fs::remove_file(path);
}
