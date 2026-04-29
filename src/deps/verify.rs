use std::path::Path;
use std::process::Command;
use crate::runtime::umu::get_umu_run_path;

pub fn check_registry_key_exists(
    prefix_path: &str,
    proton_path: &str,
    key_path: &str,
) -> Result<bool, String> {
    // We use reg.exe query to check for the existence of a key
    let mut cmd = Command::new(get_umu_run_path());
    cmd.env("WINEPREFIX", prefix_path);
    if !proton_path.is_empty() {
        cmd.env("PROTONPATH", proton_path);
    }
    cmd.env("GAMEID", "leyen-dep-verify");
    cmd.args(["reg.exe", "query", key_path]);
    
    // Silence output
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let status = cmd.status().map_err(|err| format!("Failed to run reg.exe: {err}"))?;
    Ok(status.success())
}

pub fn check_file_exists_in_prefix(
    prefix_path: &str,
    relative_path: &str,
) -> bool {
    let path = Path::new(prefix_path).join(relative_path);
    path.exists()
}
