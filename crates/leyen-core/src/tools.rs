//! Engine-local helpers. Tool probes live in `leyen-model::tools` (shared with
//! the GUI); re-exported here so existing `crate::tools::*` call sites keep
//! working.

pub use leyen_model::tools::{command_available, gamemode_available, mangohud_available};

pub fn join_err(e: tokio::task::JoinError) -> String {
    if e.is_panic() {
        format!("blocking task panicked: {e}")
    } else {
        format!("blocking task cancelled: {e}")
    }
}
