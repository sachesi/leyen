//! The bus socket a sandboxed program finds at `$XDG_RUNTIME_DIR/bus`.
//!
//! It is a unix socket with nothing listening on it, so a connection is refused
//! the moment it is tried: a program reaching for the session bus gets what it
//! would get on a machine that has none. Two reasons it is there at all rather
//! than missing. pressure-vessel binds whatever `DBUS_SESSION_BUS_ADDRESS` names
//! into its own container and its `bwrap` fails outright when the path is not
//! there. And a program with no bus address falls back to D-Bus autolaunch,
//! which on an X11 session reads the real session bus address from the root
//! window and starts a bus of its own.

use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use super::{prepare_error, runtime_dir};

/// The socket to bind into every sandbox, created once per session and shared:
/// it carries no state, and a refused connection is the same for everyone.
pub(super) fn dead_socket() -> Result<PathBuf, String> {
    let dir = PathBuf::from(runtime_dir()).join("leyen");
    fs::create_dir_all(&dir).map_err(prepare_error)?;
    place_dead_socket(dir.join("bus"))
}

/// Puts a socket nobody listens on at `path`. Bound under a temporary name and
/// renamed over it, so a sandbox starting at the same moment sees one socket or
/// the other and never a half-made one.
pub(super) fn place_dead_socket(path: PathBuf) -> Result<PathBuf, String> {
    if is_socket(&path) {
        return Ok(path);
    }
    let pending = path.with_extension("pending");
    let _ = fs::remove_file(&pending);
    // Binding is the only way to put a socket in the filesystem, and it listens.
    // Shut it down before anything can be accepted on it: a program that forks
    // while this is open — the daemon launching a game — passes the listening
    // descriptor to its child until the exec closes it, and a connection made in
    // that moment would be accepted rather than refused.
    let listener = UnixListener::bind(&pending).map_err(prepare_error)?;
    // SAFETY: the descriptor is this listener's and outlives the call.
    unsafe { libc::shutdown(listener.as_raw_fd(), libc::SHUT_RDWR) };
    drop(listener);
    fs::rename(&pending, &path).map_err(prepare_error)?;
    Ok(path)
}

fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
}
