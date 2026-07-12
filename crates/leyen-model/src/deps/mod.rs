//! Shared dependency metadata (catalog) and read-only prefix state.

pub mod catalog;
pub mod state;

pub use catalog::{DEP_CATEGORY_ORDER, DEP_PROFILES, DepProfile, get_dep_profile};
pub use state::{
    DEP_STATE_VERSION, InstalledDependency, PrefixDependencyState, find_installed_dependents,
    get_deps_cache_dir, get_installed_dep, get_prefix_deps_dir, get_prefix_deps_state_path,
    read_installed_deps, read_prefix_dep_state,
};
