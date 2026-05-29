use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::get_data_dir;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrefixDependencyState {
    #[serde(default)]
    pub installed: BTreeMap<String, InstalledDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstalledDependency {
    #[serde(default)]
    pub installed_at_epoch_seconds: u64,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub created_files: Vec<String>,
    #[serde(default)]
    pub touched_existing_files: bool,
    #[serde(default)]
    pub dll_overrides: Vec<String>,
    #[serde(default)]
    pub registered_dlls: Vec<String>,
}

impl InstalledDependency {
    pub fn has_removable_changes(&self) -> bool {
        !self.created_files.is_empty()
            || !self.dll_overrides.is_empty()
            || !self.registered_dlls.is_empty()
    }

    pub fn is_prefix_integration(&self) -> bool {
        self.touched_existing_files
            && !self.has_removable_changes()
    }

    pub fn removal_detail(&self) -> String {
        match (self.has_removable_changes(), self.touched_existing_files) {
            (true, true) => "This will remove Leyen-tracked files and overrides from the prefix. Some existing prefix files were changed during installation and may remain.".to_string(),
            (true, false) => "This will remove Leyen-tracked files and overrides from the prefix.".to_string(),
            (false, true) => "Leyen can remove this dependency from tracking, but it cannot undo registry changes. This component is integrated into the Wine prefix.".to_string(),
            (false, false) => "This removes the dependency from Leyen's tracking.".to_string(),
        }
    }
}

pub fn get_deps_cache_dir() -> String {
    get_data_dir()
        .join("deps")
        .join("cache")
        .to_string_lossy()
        .to_string()
}

pub fn get_prefix_deps_dir(prefix_path: &str) -> PathBuf {
    PathBuf::from(prefix_path).join(".leyen/deps")
}

pub fn get_prefix_deps_state_path(prefix_path: &str) -> PathBuf {
    get_prefix_deps_dir(prefix_path).join("state.toml")
}

pub fn read_prefix_dep_state(prefix_path: &str) -> PrefixDependencyState {
    let path = get_prefix_deps_state_path(prefix_path);
    fs::read_to_string(&path)
        .ok()
        .and_then(|content| toml::from_str::<PrefixDependencyState>(&content).ok())
        .unwrap_or_default()
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
    fs::write(&temp_path, content)
        .map_err(|err| format!("Failed to write temporary dependency state: {err}"))?;

    fs::rename(&temp_path, &path).map_err(|err| {
        let _ = fs::remove_file(&temp_path);
        format!("Failed to rename temporary dependency state: {err}")
    })
}

pub fn read_installed_deps(prefix_path: &str) -> BTreeSet<String> {
    read_prefix_dep_state(prefix_path)
        .installed
        .into_keys()
        .collect()
}

pub fn get_installed_dep(prefix_path: &str, dep_id: &str) -> Option<InstalledDependency> {
    read_prefix_dep_state(prefix_path)
        .installed
        .get(dep_id)
        .cloned()
}

pub fn find_installed_dependents(state: &PrefixDependencyState, dep_id: &str) -> Vec<String> {
    state
        .installed
        .iter()
        .filter(|(installed_id, installed)| {
            installed_id.as_str() != dep_id
                && installed
                    .dependencies
                    .iter()
                    .any(|dependency| dependency == dep_id)
        })
        .map(|(installed_id, _)| installed_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
    use super::{
        InstalledDependency, PrefixDependencyState, find_installed_dependents,
        save_prefix_dep_state, upsert_installed_dep,
    };

    use std::collections::BTreeMap;
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

    #[test]
    fn finds_reverse_dependency_links() {
        let state = PrefixDependencyState {
            installed: BTreeMap::from([
                (
                    "base".to_string(),
                    InstalledDependency {
                        dependencies: Vec::new(),
                        ..InstalledDependency::default()
                    },
                ),
                (
                    "child".to_string(),
                    InstalledDependency {
                        dependencies: vec!["base".to_string()],
                        ..InstalledDependency::default()
                    },
                ),
            ]),
        };

        assert_eq!(
            find_installed_dependents(&state, "base"),
            vec!["child".to_string()]
        );
    }
}
