use crate::models::GlobalSettings;

/// Atomic flags mirroring GlobalSettings.log_* so background threads can log
/// without reading the settings file on every message.
pub static LOG_ERRORS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);
pub static LOG_WARNINGS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub static LOG_OPERATIONS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn apply_log_settings(s: &GlobalSettings) {
    use std::sync::atomic::Ordering::Relaxed;
    LOG_ERRORS.store(s.log_errors, Relaxed);
    LOG_WARNINGS.store(s.log_warnings, Relaxed);
    LOG_OPERATIONS.store(s.log_operations, Relaxed);
}

/// Print a formatted leyen log line to stderr.
/// Level: "ERROR" | "WARN " | "INFO "
pub fn leyen_log(level: &str, message: &str) {
    use std::sync::atomic::Ordering::Relaxed;
    let enabled = match level {
        "ERROR" => LOG_ERRORS.load(Relaxed),
        "WARN " => LOG_WARNINGS.load(Relaxed),
        _       => LOG_OPERATIONS.load(Relaxed),
    };
    if enabled {
        eprintln!("[LEYEN] [{level}] {message}");
    }
}
