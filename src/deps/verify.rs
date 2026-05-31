use crate::runtime::umu::get_umu_run_path;
use std::process::Command;
use std::time::{Duration, Instant};

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

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("Failed to run reg.exe: {err}"))?;

    let timeout = Duration::from_secs(60);
    let start = Instant::now();
    let poll_interval = Duration::from_millis(100);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(false);
                }
                std::thread::sleep(poll_interval);
            }
            Err(err) => return Err(format!("Failed to wait for reg.exe: {err}")),
        }
    }
}


