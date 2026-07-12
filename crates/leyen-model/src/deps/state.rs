//! Per-prefix dependency state — types and the **read-only** accessors.
//!
//! Clients read this directly to render installed/available dependencies; the
//! daemon is the only writer (the flocked upsert/remove side lives in
//! `leyen-core`).

use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use crate::paths::get_data_dir;

/// Current per-prefix `state.toml` schema version. Bump when a change to
/// `PrefixDependencyState` needs the reader/writer split below to distinguish
/// old files from new ones.
pub const DEP_STATE_VERSION: u32 = 1;

fn default_dep_state_version() -> u32 {
    DEP_STATE_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PrefixDependencyState {
    #[serde(default = "default_dep_state_version")]
    pub version: u32,
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
        self.touched_existing_files && !self.has_removable_changes()
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
    let state: PrefixDependencyState = fs::read_to_string(&path)
        .ok()
        .and_then(|content| toml::from_str(&content).ok())
        .unwrap_or_default();

    // No Result channel here — same reasoning as settings::load_settings: warn
    // and keep using the parsed data rather than silently defaulting (which a
    // later write could persist over a newer-versioned file).
    if state.version > DEP_STATE_VERSION {
        log::warn!(
            "Dependency state '{}' was written by a newer version of leyen (file version {}, this build supports {}); continuing with the parsed data",
            path.display(),
            state.version,
            DEP_STATE_VERSION
        );
    }

    state
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn missing_version_field_parses_as_one() {
        let state: PrefixDependencyState = toml::from_str("").unwrap();
        assert_eq!(state.version, 1);
    }

    #[test]
    fn finds_reverse_dependency_links() {
        let state = PrefixDependencyState {
            version: DEP_STATE_VERSION,
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
