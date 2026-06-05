//! Dependency engine (daemon side): step recipes, the install/uninstall async
//! engine, the flocked state writer and prefix verification. Catalog metadata and
//! the state read side live in `leyen-model`.

pub mod engine;
pub mod recipes;
pub mod state;
pub mod verify;
mod tests;

pub use engine::{execute_dep_step, install_dep, uninstall_dep};
pub use recipes::get_dep_steps;
pub use state::{remove_installed_dep, save_prefix_dep_state, upsert_installed_dep};
