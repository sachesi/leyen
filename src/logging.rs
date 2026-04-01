use std::sync::Mutex;

use crate::models::GlobalSettings;

/// Atomic flags mirroring GlobalSettings.log_* so background threads can log
/// without reading the settings file on every message.
pub static LOG_ERRORS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);
pub static LOG_WARNINGS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub static LOG_OPERATIONS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// In-memory buffer that stores every log line regardless of the per-level
/// flags.  The log window reads from this buffer.
static LOG_BUFFER: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn apply_log_settings(s: &GlobalSettings) {
    use std::sync::atomic::Ordering::Relaxed;
    LOG_ERRORS.store(s.log_errors, Relaxed);
    LOG_WARNINGS.store(s.log_warnings, Relaxed);
    LOG_OPERATIONS.store(s.log_operations, Relaxed);
}

/// Return a snapshot of every log line captured so far.
pub fn get_log_buffer() -> Vec<String> {
    LOG_BUFFER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Print a formatted leyen log line to stderr **and** append it to the
/// in-memory buffer so the log window can display it.
/// Level: "ERROR" | "WARN " | "INFO "
pub fn leyen_log(level: &str, message: &str) {
    use std::sync::atomic::Ordering::Relaxed;
    let line = format!("[LEYEN] [{level}] {message}");

    // Always append to the buffer so the log window shows everything.
    if let Ok(mut buf) = LOG_BUFFER.lock() {
        buf.push(line.clone());
    }

    let enabled = match level {
        "ERROR" => LOG_ERRORS.load(Relaxed),
        "WARN " => LOG_WARNINGS.load(Relaxed),
        _       => LOG_OPERATIONS.load(Relaxed),
    };
    if enabled {
        eprintln!("{line}");
    }
}
