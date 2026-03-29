mod catalog;
mod engine;

pub use catalog::{
    DEP_CATALOG, DEP_CATEGORY_ORDER, DepCatalogEntry, get_dep_uninstall_steps,
};
pub use engine::{install_dep_async, uninstall_dep_async};

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

// ── Cache & tracking helpers ──────────────────────────────────────────────────

pub fn get_deps_cache_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/leyen/deps/cache", home)
}

pub fn get_prefix_deps_file(prefix_path: &str) -> PathBuf {
    PathBuf::from(prefix_path).join(".leyen/deps/installed.txt")
}

pub fn read_installed_deps(prefix_path: &str) -> HashSet<String> {
    let path = get_prefix_deps_file(prefix_path);
    fs::read_to_string(&path)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

pub fn add_installed_dep(prefix_path: &str, dep_id: &str) {
    let path = get_prefix_deps_file(prefix_path);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut installed = read_installed_deps(prefix_path);
    installed.insert(dep_id.to_string());
    let mut sorted: Vec<String> = installed.into_iter().collect();
    sorted.sort();
    let _ = fs::write(&path, format!("{}\n", sorted.join("\n")));
}

pub fn remove_installed_dep(prefix_path: &str, dep_id: &str) {
    let path = get_prefix_deps_file(prefix_path);
    let mut installed = read_installed_deps(prefix_path);
    installed.remove(dep_id);
    let mut sorted: Vec<String> = installed.into_iter().collect();
    sorted.sort();
    let _ = fs::write(&path, format!("{}\n", sorted.join("\n")));
}
