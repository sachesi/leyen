use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

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
pub static LOG_ERRORS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
pub static LOG_WARNINGS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static LOG_OPERATIONS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn log_path() -> PathBuf {
    get_config_dir().join("logs.jsonl")
}

fn append_log_entry(entry: &LogEntry) {
    let path = log_path();
    if let Some(parent) = path.parent()
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
    use std::sync::atomic::Ordering::Relaxed;
    LOG_ERRORS.store(s.log_errors, Relaxed);
    LOG_WARNINGS.store(s.log_warnings, Relaxed);
    LOG_OPERATIONS.store(s.log_operations, Relaxed);
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

/// Print a formatted leyen log line to stderr **and** append it to the
/// shared log file so the log window can display it across GUI/CLI processes.
/// Level: "ERROR" | "WARN " | "INFO "
pub fn leyen_log(level: &str, message: &str) {
    log_impl(level, message, None);
}

pub fn leyen_game_log(game_id: &str, level: &str, message: &str) {
    log_impl(level, message, Some(game_id));
}

fn log_impl(level: &str, message: &str, game_id: Option<&str>) {
    use std::sync::atomic::Ordering::Relaxed;
    let line = match game_id {
        Some(game_id) => format!("[LEYEN] [{level}] [game:{game_id}] {message}"),
        None => format!("[LEYEN] [{level}] {message}"),
    };

    append_log_entry(&LogEntry {
        line: line.clone(),
        game_id: game_id.map(str::to_string),
    });

    let enabled = match level {
        "ERROR" => LOG_ERRORS.load(Relaxed),
        "WARN " => LOG_WARNINGS.load(Relaxed),
        _ => LOG_OPERATIONS.load(Relaxed),
    };
    if enabled {
        eprintln!("{line}");
    }
}
