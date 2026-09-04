use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::process::Command as AsyncCommand;

use super::engine::{configure_umu_command_async, run_umu_command};
use crate::runtime::umu::get_umu_run_path;

/// Queries `key_path` with `reg.exe` inside the prefix. Runs through the
/// engine's command runner so Cancel and the timeout kill the whole wine
/// process group; a timeout is an error, not a missing key.
pub async fn check_registry_key_exists(
    prefix_path: &str,
    proton_path: &str,
    key_path: &str,
    cancel: Arc<AtomicBool>,
) -> Result<bool, String> {
    let mut cmd = AsyncCommand::new(get_umu_run_path());
    configure_umu_command_async(&mut cmd, prefix_path, proton_path);
    cmd.env("GAMEID", "leyen-dep-verify");
    cmd.args(["reg.exe", "query", key_path]);
    let output = run_umu_command(cmd, "reg.exe query".to_string(), cancel).await?;
    Ok(output.status.success())
}
