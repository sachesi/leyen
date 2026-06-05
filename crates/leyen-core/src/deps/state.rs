//! Per-prefix dependency state — the **write** side (daemon-only).
//!
//! Types and read accessors live in `leyen-model`; this module owns the flocked
//! upsert/remove and atomic persistence.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use leyen_model::deps::{
    InstalledDependency, PrefixDependencyState, get_prefix_deps_dir, get_prefix_deps_state_path,
};

/// RAII guard that acquires `LOCK_EX | LOCK_NB` with retry + timeout.
/// Releases the lock on Drop — panic-safe.
struct FlockGuard {
    file: File,
}

impl FlockGuard {
    fn lock(path: &Path, timeout: Duration) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        let fd = file.as_raw_fd();

        let start = std::time::Instant::now();
        loop {
            let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Self { file });
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::WouldBlock {
                return Err(err);
            }
            if start.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Timed out waiting for lock on '{}' after {:?}",
                        path.display(),
                        timeout
                    ),
                ));
            }
            sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for FlockGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn prefix_deps_lock_path(prefix_path: &str) -> PathBuf {
    get_prefix_deps_dir(prefix_path).join(".state.lock")
}

/// Reads dependency state, distinguishing "absent" (defaults) from "present but
/// corrupt" (error). Mutating callers must use this — silently defaulting on a
/// parse error would erase all tracked dependencies on the next write.
fn read_prefix_dep_state_checked(prefix_path: &str) -> Result<PrefixDependencyState, String> {
    let path = get_prefix_deps_state_path(prefix_path);
    match fs::read_to_string(&path) {
        Ok(content) => toml::from_str::<PrefixDependencyState>(&content).map_err(|err| {
            format!(
                "Failed to parse dependency state '{}': {err}",
                path.display()
            )
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(PrefixDependencyState::default()),
        Err(err) => Err(format!(
            "Failed to read dependency state '{}': {err}",
            path.display()
        )),
    }
}

pub fn save_prefix_dep_state(
    prefix_path: &str,
    state: &PrefixDependencyState,
) -> Result<(), String> {
    let path = get_prefix_deps_state_path(prefix_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create dependency state directory: {err}"))?;
    }
    let content = toml::to_string_pretty(state)
        .map_err(|err| format!("Failed to serialize dependency state: {err}"))?;

    let temp_path = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    {
        let mut file = File::create(&temp_path)
            .map_err(|err| format!("Failed to write temporary dependency state: {err}"))?;
        file.write_all(content.as_bytes())
            .map_err(|err| format!("Failed to write temporary dependency state: {err}"))?;
        file.sync_all()
            .map_err(|err| format!("Failed to sync temporary dependency state: {err}"))?;
    }

    fs::rename(&temp_path, &path).map_err(|err| {
        let _ = fs::remove_file(&temp_path);
        format!("Failed to rename temporary dependency state: {err}")
    })
}

pub fn upsert_installed_dep(
    prefix_path: &str,
    dep_id: &str,
    dependencies: &[&str],
    delta: &InstalledDependency,
) -> Result<(), String> {
    let _guard = FlockGuard::lock(&prefix_deps_lock_path(prefix_path), Duration::from_secs(5))
        .map_err(|err| format!("Failed to acquire dependency state lock: {err}"))?;
    let mut state = read_prefix_dep_state_checked(prefix_path)?;
    let entry = state.installed.entry(dep_id.to_string()).or_default();

    entry.installed_at_epoch_seconds = current_epoch_seconds();
    entry.dependencies = unique_sorted_strings(
        dependencies
            .iter()
            .map(|dependency| dependency.to_string())
            .collect(),
    );
    entry.created_files = merge_unique_strings(&entry.created_files, &delta.created_files);
    entry.touched_existing_files |= delta.touched_existing_files;
    entry.dll_overrides = merge_unique_strings(&entry.dll_overrides, &delta.dll_overrides);
    entry.registered_dlls = merge_unique_strings(&entry.registered_dlls, &delta.registered_dlls);

    save_prefix_dep_state(prefix_path, &state)
}

pub fn remove_installed_dep(
    prefix_path: &str,
    dep_id: &str,
) -> Result<Option<InstalledDependency>, String> {
    let _guard = FlockGuard::lock(&prefix_deps_lock_path(prefix_path), Duration::from_secs(5))
        .map_err(|err| format!("Failed to acquire dependency state lock: {err}"))?;
    let mut state = read_prefix_dep_state_checked(prefix_path)?;
    let removed = state.installed.remove(dep_id);
    save_prefix_dep_state(prefix_path, &state)?;
    Ok(removed)
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn merge_unique_strings(existing: &[String], additional: &[String]) -> Vec<String> {
    let mut merged: BTreeSet<String> = existing.iter().cloned().collect();
    merged.extend(additional.iter().cloned());
    merged.into_iter().collect()
}

fn unique_sorted_strings(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{save_prefix_dep_state, upsert_installed_dep};
    use leyen_model::deps::{InstalledDependency, PrefixDependencyState};
    use std::fs;
    use std::path::PathBuf;

    fn temp_prefix() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "leyen-deps-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn installed_dep_merge_keeps_unique_records() {
        let prefix = temp_prefix();
        let prefix_str = prefix.to_string_lossy().to_string();

        save_prefix_dep_state(&prefix_str, &PrefixDependencyState::default()).unwrap();

        let first = InstalledDependency {
            created_files: vec!["drive_c/windows/system32/a.dll".to_string()],
            dll_overrides: vec!["a".to_string()],
            ..InstalledDependency::default()
        };
        upsert_installed_dep(&prefix_str, "test", &["base"], &first).unwrap();

        let second = InstalledDependency {
            created_files: vec!["drive_c/windows/system32/a.dll".to_string()],
            registered_dlls: vec!["a.dll".to_string()],
            touched_existing_files: true,
            ..InstalledDependency::default()
        };
        upsert_installed_dep(&prefix_str, "test", &["base"], &second).unwrap();

        let content =
            fs::read_to_string(prefix.join(".leyen/deps/state.toml")).expect("state file missing");
        assert!(content.contains("drive_c/windows/system32/a.dll"));
        assert!(content.contains("a.dll"));
        assert!(content.contains("touched_existing_files = true"));

        let _ = fs::remove_dir_all(prefix);
    }
}
