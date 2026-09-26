//! One PID namespace per prefix. Wine serves a prefix from a single wineserver,
//! which finds its clients by process ID, so every sandbox on a prefix has to
//! share a PID namespace, the wineserver's socket directory and `/dev/shm`. An
//! idle `bwrap` in a scope of its own holds them; sandboxes enter it with
//! `nsenter` and build their own view of the filesystem from there.

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use leyen_model::i18n::gettext;
use sha2::{Digest, Sha256};
use tokio::process::Command as AsyncCommand;
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::{runtime_dir, tools};
use crate::launch::{in_scope, stop_scope_verified, systemctl_show_property, unit_is_active};

/// Held from looking the holder up until the program that enters it is spawned,
/// so [`release`] cannot stop a holder someone is about to enter.
pub struct Lease(#[allow(dead_code)] OwnedMutexGuard<()>);

impl Lease {
    /// Call once the program is spawned. It has not entered the holder yet at
    /// that point, so the lease is kept a moment longer.
    pub fn spawned(self) {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            drop(self);
        });
    }
}

fn gate() -> std::sync::Arc<Mutex<()>> {
    static GATE: std::sync::OnceLock<std::sync::Arc<Mutex<()>>> = std::sync::OnceLock::new();
    GATE.get_or_init(Default::default).clone()
}

/// Named after the folder rather than how the prefix is spelled: a trailing slash
/// or a symlink must not get a second holder, and with it a second wineserver
/// that loses the first one's registry writes.
fn unit_name(prefix: &str) -> String {
    let path = Path::new(prefix);
    let folder = fs::canonicalize(path).unwrap_or_else(|_| path.components().collect());
    let digest = Sha256::digest(folder.as_os_str().as_bytes());
    format!("leyen-prefix-{}.scope", hex::encode(&digest[..8]))
}

/// Where the holder keeps what its sandboxes share: a `tmpfs` only it and they
/// see, mounted over this directory.
pub(super) fn shared_dir() -> PathBuf {
    PathBuf::from(runtime_dir()).join("leyen/shared")
}

/// The holder's process inside its namespaces, as the host numbers it. The scope
/// holds the outer `bwrap` as well, which stays in the daemon's own namespaces;
/// what `nsenter` needs is a process that left them. Found that way rather than
/// by name: the single binary NixOS links `sleep` to takes its name from the
/// whole path it was called by.
fn holder_pid(unit: &str) -> Option<u32> {
    let cgroup = systemctl_show_property(unit, "ControlGroup")?;
    let procs = fs::read_to_string(format!("/sys/fs/cgroup{cgroup}/cgroup.procs")).ok()?;
    let own = fs::read_link("/proc/self/ns/pid").ok()?;
    procs
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .find(|pid| {
            fs::read_link(format!("/proc/{pid}/ns/pid")).is_ok_and(|namespace| namespace != own)
        })
}

/// The holder for `prefix`, started if there is none, and the lease to keep
/// until the program entering it has been spawned.
pub(super) async fn holder(prefix: &str) -> Result<(u32, Lease), String> {
    let lease = Lease(gate().lock_owned().await);
    let prefix = prefix.to_string();
    let pid = tokio::task::spawn_blocking(move || start_holder(&unit_name(&prefix)))
        .await
        .map_err(|e| e.to_string())??;
    Ok((pid, lease))
}

fn start_holder(unit: &str) -> Result<u32, String> {
    if unit_is_active(unit) == Some(true)
        && let Some(pid) = holder_pid(unit)
    {
        return Ok(pid);
    }
    let failed = || gettext("The sandbox's process namespace could not be created.");
    let tools = tools()?;
    let sleep = super::find_program_unresolved("sleep").ok_or_else(failed)?;
    let shared = shared_dir();
    fs::create_dir_all(&shared).map_err(|_| failed())?;

    let mut bwrap = AsyncCommand::new(&tools.bwrap);
    bwrap.args(["--unshare-user", "--unshare-pid", "--bind", "/", "/"]);
    bwrap.args(["--dev-bind", "/dev", "/dev", "--proc", "/proc", "--tmpfs"]);
    bwrap.arg(&shared);
    for dir in ["wine", "shm"] {
        bwrap.args(["--perms", "0700", "--dir"]);
        bwrap.arg(shared.join(dir));
    }
    bwrap.arg("--");
    bwrap.arg(&sleep);
    bwrap.arg("infinity");
    let mut scoped = in_scope(&bwrap, unit);
    scoped
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    scoped.into_std().spawn().map_err(|_| failed())?;

    for _ in 0..50 {
        if let Some(pid) = holder_pid(unit) {
            return Ok(pid);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(failed())
}

/// Stops the holder of `prefix` once nothing but the holder is left in it.
pub async fn release(prefix: &str) {
    let _lease = gate().lock_owned().await;
    let prefix = prefix.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        let unit = unit_name(&prefix);
        let Some(pid) = holder_pid(&unit) else {
            return;
        };
        let Ok(namespace) = fs::read_link(format!("/proc/{pid}/ns/pid")) else {
            return;
        };
        let members = fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                fs::read_link(entry.path().join("ns/pid")).is_ok_and(|link| link == namespace)
            })
            .count();
        // bwrap as the namespace's init, and the sleep it runs.
        if members <= 2 {
            stop_scope_verified(&unit);
        }
    })
    .await;
}

/// `nsenter` into the holder, in front of the sandbox's own `bwrap`.
pub(super) fn enter(nsenter: &Path, holder: u32) -> AsyncCommand {
    let mut command = AsyncCommand::new(nsenter);
    command.args(["--target", &holder.to_string()]);
    command.args(["--user", "--pid", "--mount", "--preserve-credentials", "--"]);
    command
}

#[cfg(test)]
mod tests {
    use super::unit_name;

    #[test]
    fn one_prefix_has_one_holder_however_it_is_spelled() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("pfx");
        std::fs::create_dir(&prefix).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&prefix, &link).unwrap();

        let plain = unit_name(prefix.to_str().unwrap());
        assert_eq!(unit_name(&format!("{}/", prefix.display())), plain);
        assert_eq!(unit_name(link.to_str().unwrap()), plain);
        // A prefix that is not there yet, or no longer, still loses the slash.
        assert_eq!(unit_name("/nowhere/pfx/"), unit_name("/nowhere/pfx"));
        assert_ne!(unit_name("/nowhere/pfx"), plain);
    }
}
