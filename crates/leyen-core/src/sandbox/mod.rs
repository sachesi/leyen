//! The bubblewrap sandbox every Windows program runs in, with the system-call
//! filter in [`seccomp`] over it. A launch that cannot be sandboxed is refused;
//! there is no unsandboxed path.
//!
//! What a program gets:
//!
//! - a `tmpfs` for `$HOME`, `/tmp` and `$XDG_RUNTIME_DIR`, so nothing in the home
//!   directory is there unless it is bound in by name — Leyen's own configuration
//!   included, which decides what Leyen runs next
//! - its prefix read-write, with `$HOME/.cache` a directory inside it
//! - its own folder read-write, and the folders shared with it, read-only
//!   unless marked writable
//! - `/usr`, `/etc` and `/sys` read-only — on NixOS `/nix/store`, the system's
//!   own programs and the graphics drivers under `/run` as well — and the devices
//!   for graphics, sound and controllers
//! - the display and audio sockets, and a bus socket with nothing behind it. The
//!   session bus is not exposed: a game on it could ask Leyen to launch anything,
//!   or systemd to start a unit outside the sandbox.

mod bus;
mod pidns;
pub mod seccomp;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use leyen_model::i18n::gettext;
use leyen_model::models::SandboxFolder;
use leyen_model::paths::{get_config_dir, get_data_dir};
use leyen_model::runtime::get_umu_runtime_dir;
use log::info;
use tokio::process::Command as AsyncCommand;

pub use pidns::{Lease, release as release_namespace};
use seccomp::{SECCOMP_FD, SeccompFilter};

/// What one sandboxed program reaches on top of the fixed layout.
#[derive(Debug, Default, Clone)]
pub struct SandboxRequest {
    pub prefix_path: String,
    /// Empty when umu-launcher picks a Proton itself.
    pub proton_path: String,
    /// Must be a directory the sandbox exposes.
    pub work_dir: Option<PathBuf>,
    /// The program's own folder first, then whatever else it is given.
    pub shares: Vec<Share>,
    pub network: bool,
}

#[derive(Debug, Clone)]
pub struct Share {
    pub path: PathBuf,
    pub writable: bool,
}

impl Share {
    pub fn read_only(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            writable: false,
        }
    }

    pub fn writable(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            writable: true,
        }
    }
}

/// The tools the sandbox is built from, resolved and probed once.
struct Tools {
    bwrap: PathBuf,
    nsenter: PathBuf,
}

/// A command wrapped in the sandbox and in its systemd scope, ready to spawn.
pub struct Confined {
    pub command: AsyncCommand,
    /// To be dropped once `command` has been spawned.
    pub lease: Lease,
}

/// Resolves the tools and proves a sandbox can be built here, once per daemon
/// life. Without the pieces, or without unprivileged user namespaces, this is
/// where the launch fails, with a message worth reading.
fn tools() -> Result<&'static Tools, String> {
    static TOOLS: OnceLock<Result<Tools, String>> = OnceLock::new();
    TOOLS
        .get_or_init(probe_tools)
        .as_ref()
        .map_err(String::clone)
}

fn probe_tools() -> Result<Tools, String> {
    let bwrap = find_program("bwrap").ok_or_else(|| {
        gettext(
            "bubblewrap is required to run games in a sandbox. Install the “bubblewrap” package.",
        )
    })?;
    let nsenter = find_program("nsenter").ok_or_else(|| {
        gettext("nsenter is required to run games in a sandbox. Install the “util-linux” package.")
    })?;

    // The real thing in miniature: a user namespace, a PID namespace and the
    // filter.
    let filter = SeccompFilter::compile().map_err(prepare_error)?;
    let mut probe = std::process::Command::new(&bwrap);
    probe.args(system_args());
    probe.args([
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--unshare-user",
        "--unshare-pid",
        "--new-session",
        "--seccomp",
    ]);
    probe.arg(SECCOMP_FD.to_string());
    // bwrap itself is the one program known to be there: NixOS has nothing in
    // /usr/bin but `env`.
    probe.arg("--");
    probe.arg(&bwrap);
    probe.arg("--version");
    filter.place_on_std_fd(&mut probe).map_err(prepare_error)?;
    let output = probe
        .output()
        .map_err(|e| gettext("Failed to start bubblewrap: {}").replacen("{}", &e.to_string(), 1))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let reason = if reason.is_empty() {
            "bwrap failed".to_string()
        } else {
            reason
        };
        return Err(gettext("The sandbox could not be created: {}").replacen("{}", &reason, 1));
    }

    info!("Sandbox ready: {}", bwrap.display());
    Ok(Tools { bwrap, nsenter })
}

fn prepare_error(error: impl std::fmt::Display) -> String {
    gettext("Failed to prepare the sandbox: {}").replacen("{}", &error.to_string(), 1)
}

/// Every place a program is looked for, in order: `PATH` and the usual system
/// directories. Fedora and openSUSE ship `bwrap` in `/usr/sbin`, which a user
/// session's `PATH` does not always include; NixOS has it in
/// `/run/current-system/sw/bin`.
fn program_candidates(name: &str) -> Vec<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .chain(
            [
                "/usr/bin",
                "/usr/sbin",
                "/bin",
                "/sbin",
                "/run/current-system/sw/bin",
            ]
            .iter()
            .map(PathBuf::from),
        )
        .map(|dir| dir.join(name))
        .filter(|candidate| candidate.is_file())
        .collect()
}

/// Looks for `name`, resolved, because that is the path a sandbox can run: on
/// NixOS the directories searched are symlinks into `/nix/store`, and the store
/// is what a sandbox has.
fn find_program(name: &str) -> Option<PathBuf> {
    program_candidates(name)
        .into_iter()
        .find_map(|candidate| fs::canonicalize(candidate).ok())
}

/// Looks for `name` as it is installed, symlink and all, for a program reached
/// through a whole root rather than a sandbox: resolving `sleep` on NixOS lands
/// on coreutils' single binary, which under its own name only prints a usage
/// error.
fn find_program_unresolved(name: &str) -> Option<PathBuf> {
    program_candidates(name).into_iter().next()
}

/// Whether a sandbox can be built. Cheap after the first call.
pub fn is_available() -> Result<(), String> {
    tools().map(|_| ())
}

/// Folders that are never shared, whatever the settings say, resolved like the
/// folder they are compared with: `/home` is a symlink on some systems. It is on
/// the list in its own right, not only as the parent of `$HOME`: a daemon whose
/// home is elsewhere would otherwise hand a game everyone's home directory.
fn forbidden_folders() -> Vec<PathBuf> {
    let mut forbidden: Vec<PathBuf> = [
        "/", "/boot", "/dev", "/etc", "/home", "/nix", "/proc", "/root", "/run", "/sys", "/usr",
        "/var",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Ok(home) = std::env::var("HOME") {
        forbidden.push(PathBuf::from(home));
    }
    forbidden.extend(leyen_folders());
    forbidden
        .into_iter()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect()
}

/// Leyen's configuration and the launcher it installed: what Leyen runs next.
/// Nothing inside them is shared either.
fn leyen_folders() -> [PathBuf; 2] {
    [get_config_dir(), get_data_dir().join("core")]
}

/// Refuses a folder that is, or holds, one of [`forbidden_folders`]. The path is
/// resolved first, so a symlink cannot stand in for a folder refused by name.
pub fn check_folder(folder: &SandboxFolder) -> Result<Share, String> {
    let path = Path::new(folder.path.trim());
    if !path.is_absolute() {
        return Err(gettext("“{}” is not an absolute path").replacen("{}", &folder.path, 1));
    }
    let resolved = fs::canonicalize(path).map_err(|e| {
        gettext("The folder “{}” cannot be used: {}")
            .replacen("{}", &folder.path, 1)
            .replacen("{}", &e.to_string(), 1)
    })?;
    if !resolved.is_dir() {
        return Err(gettext("“{}” is not a folder").replacen("{}", &folder.path, 1));
    }
    let inside_leyen = leyen_folders()
        .iter()
        .map(|path| fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .any(|path| resolved.starts_with(path));
    for forbidden in forbidden_folders() {
        if forbidden == resolved || inside_leyen {
            return Err(gettext(
                "The folder “{}” cannot be shared with a game: keeping it out is what the sandbox is for.",
            )
            .replacen("{}", &folder.path, 1));
        }
        if forbidden.starts_with(&resolved) {
            return Err(gettext(
                "The folder “{}” cannot be shared with a game: it holds “{}”, and keeping that out is what the sandbox is for.",
            )
            .replacen("{}", &folder.path, 1)
            .replacen("{}", &forbidden.display().to_string(), 1));
        }
    }
    Ok(Share {
        path: path.to_path_buf(),
        writable: folder.writable,
    })
}

/// Turns the folders of a launch into shares, and returns what had to be refused
/// so the caller can say so rather than silently drop it.
pub fn shares_from(folders: &[SandboxFolder]) -> (Vec<Share>, Vec<String>) {
    let mut shares = Vec::new();
    let mut refused = Vec::new();
    for folder in folders {
        match check_folder(folder) {
            Ok(share) => shares.push(share),
            Err(reason) => refused.push(reason),
        }
    }
    (shares, refused)
}

/// The host paths and sockets the argument list is built from. Detected from the
/// environment for a real launch; built by hand in the tests.
#[derive(Debug, Clone)]
pub struct HostLayout {
    pub home: PathBuf,
    pub runtime_dir: PathBuf,
    /// The socket bound into the sandbox as `$XDG_RUNTIME_DIR/bus`, which nothing
    /// listens on.
    pub bus_socket: PathBuf,
    /// The prefix's holder keeps the wineserver's directory and `/dev/shm` here,
    /// the same for every sandbox on the prefix.
    pub shared_dir: PathBuf,
    pub wayland_socket: Option<PathBuf>,
    pub x11_socket: Option<PathBuf>,
    pub xauthority: Option<PathBuf>,
    /// `/etc/resolv.conf`'s target when it points outside `/etc`, which is where
    /// systemd-resolved keeps it; a read-only `/etc` alone would leave the
    /// sandbox unable to resolve a name.
    pub resolv_conf: Option<PathBuf>,
    /// Device nodes to pass through, from what `/dev` actually has.
    pub devices: Vec<PathBuf>,
    /// umu-launcher's own directories: the Steam Linux Runtime, the Proton builds
    /// it downloads and its cache, read-write.
    pub umu_writable: Vec<PathBuf>,
    /// umu-launcher and winetricks as Leyen installed them: read-only, so a game
    /// cannot rewrite the launcher that starts it next time. The Proton builds
    /// are not here — a game is given the one it runs with and no other.
    pub umu_read_only: Vec<PathBuf>,
}

impl HostLayout {
    pub fn detect(bus_socket: PathBuf) -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()));
        let runtime_dir = PathBuf::from(runtime_dir());

        let wayland_socket = std::env::var("WAYLAND_DISPLAY").ok().map(|display| {
            if display.starts_with('/') {
                PathBuf::from(display)
            } else {
                runtime_dir.join(display)
            }
        });
        let x11_socket = std::env::var("DISPLAY").ok().and_then(|display| {
            let number: String = display
                .trim_start_matches(':')
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            (!number.is_empty()).then(|| PathBuf::from(format!("/tmp/.X11-unix/X{number}")))
        });
        let xauthority = std::env::var("XAUTHORITY").ok().map(PathBuf::from);
        let resolv_conf = fs::canonicalize("/etc/resolv.conf")
            .ok()
            .filter(|path| !path.starts_with("/etc"));

        let mut devices: Vec<PathBuf> = ["dri", "snd", "input", "ntsync", "kfd"]
            .iter()
            .map(|node| PathBuf::from("/dev").join(node))
            .collect();
        // Controllers speak through hidraw as well, and NVIDIA's driver through a
        // set of nodes whose names depend on the machine.
        if let Ok(entries) = fs::read_dir("/dev") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("hidraw") || name.starts_with("nvidia") {
                    devices.push(entry.path());
                }
            }
        }
        devices.sort();

        Self {
            home: home.clone(),
            runtime_dir,
            bus_socket,
            shared_dir: pidns::shared_dir(),
            wayland_socket,
            x11_socket,
            xauthority,
            resolv_conf,
            devices,
            umu_writable: vec![
                PathBuf::from(get_umu_runtime_dir()),
                home.join(".local/share/umu"),
                home.join(".cache/umu"),
                // umu-launcher downloads Proton into Steam's tools directory.
                home.join(".local/share/Steam/compatibilitytools.d"),
            ],
            // `core` holds umu-launcher and winetricks both.
            umu_read_only: vec![get_data_dir().join("core")],
        }
    }
}

fn runtime_dir() -> String {
    std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: getuid cannot fail and takes no arguments.
        format!("/run/user/{}", unsafe { libc::getuid() })
    })
}

/// `/usr` read-only, plus `/bin`, `/lib` and friends as this host has them:
/// symlinks into `/usr` on a merged system, real directories on an older one.
/// Without them a dynamically linked program cannot find its loader. On NixOS the
/// loader, and everything else, is in `/nix/store`, and `/usr` holds only `env`.
fn system_args() -> Vec<String> {
    let mut args = vec![
        "--ro-bind".to_string(),
        "/usr".to_string(),
        "/usr".to_string(),
        "--ro-bind-try".to_string(),
        "/nix".to_string(),
        "/nix".to_string(),
    ];
    for link in ["/bin", "/sbin", "/lib", "/lib32", "/lib64"] {
        match fs::read_link(link) {
            Ok(target) => {
                args.push("--symlink".into());
                args.push(target.to_string_lossy().into_owned());
                args.push(link.into());
            }
            Err(_) if Path::new(link).is_dir() => {
                bind(&mut args, "--ro-bind", Path::new(link), Path::new(link));
            }
            Err(_) => {}
        }
    }
    args
}

fn flag(args: &mut Vec<String>, items: &[&str]) {
    args.extend(items.iter().map(|item| (*item).to_string()));
}

/// The source resolved, the target as given: a program told to look at a path
/// the host keeps as a symlink finds the folder there.
fn bind(args: &mut Vec<String>, mode: &str, source: &Path, target: &Path) {
    let source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    args.push(mode.to_string());
    args.push(source.to_string_lossy().into_owned());
    args.push(target.to_string_lossy().into_owned());
}

/// Builds the `bwrap` argument list for `request`, up to and including the `--`
/// that ends it. Pure, so the layout can be asserted in tests without a session
/// bus, a GPU or a prefix.
pub fn bwrap_args(
    request: &SandboxRequest,
    host: &HostLayout,
    env: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    flag(
        &mut args,
        &["--unshare-user", "--unshare-uts", "--unshare-cgroup"],
    );
    flag(&mut args, &["--hostname", "leyen-sandbox"]);
    if !request.network {
        flag(&mut args, &["--unshare-net"]);
    }
    // X11 clients pass images through SysV shared memory, which lives in the IPC
    // namespace they share with the X server; with no X socket in the sandbox
    // nothing inside has business there.
    if host.x11_socket.is_none() {
        flag(&mut args, &["--unshare-ipc"]);
    }
    // A session of its own has no controlling terminal, so nothing inside can
    // push characters back into the terminal the daemon was started from.
    flag(&mut args, &["--new-session"]);

    args.extend(system_args());
    flag(&mut args, &["--ro-bind", "/etc", "/etc"]);
    flag(&mut args, &["--ro-bind", "/sys", "/sys"]);
    flag(&mut args, &["--proc", "/proc"]);
    flag(&mut args, &["--dev", "/dev"]);
    for device in &host.devices {
        bind(&mut args, "--dev-bind-try", device, device);
    }
    // Controller hotplug reads from here.
    flag(&mut args, &["--ro-bind-try", "/run/udev", "/run/udev"]);
    // NixOS keeps the graphics drivers and the system's own programs outside
    // /usr, and the loaders look for them here by these names.
    for path in [
        "/run/opengl-driver",
        "/run/opengl-driver-32",
        "/run/current-system",
    ] {
        bind(&mut args, "--ro-bind-try", Path::new(path), Path::new(path));
    }
    if let Some(resolv) = &host.resolv_conf {
        bind(&mut args, "--ro-bind-try", resolv, resolv);
    }

    // The PID namespace is the prefix's holder's, entered before bwrap runs.
    flag(&mut args, &["--tmpfs", "/tmp"]);
    flag(&mut args, &["--tmpfs", "/var/tmp"]);
    args.push("--tmpfs".into());
    args.push(host.home.to_string_lossy().into_owned());
    args.push("--tmpfs".into());
    args.push(host.runtime_dir.to_string_lossy().into_owned());

    // Shares first, so on the same path Leyen's own read-only binds win. Shader
    // caches belong to the prefix's Proton, so $HOME/.cache is a directory in it.
    let prefix = Path::new(&request.prefix_path);
    let mut mounts: Vec<(&str, PathBuf, PathBuf)> = Vec::new();
    // A folder inside the prefix is there already, writable.
    for share in request
        .shares
        .iter()
        .filter(|share| !share.path.starts_with(prefix))
    {
        let mode = if share.writable {
            "--bind-try"
        } else {
            "--ro-bind-try"
        };
        mounts.push((mode, share.path.clone(), share.path.clone()));
    }
    mounts.push(("--bind", prefix.to_path_buf(), prefix.to_path_buf()));
    mounts.push((
        "--bind",
        prefix.join(".leyen/cache"),
        host.home.join(".cache"),
    ));
    for path in &host.umu_writable {
        mounts.push(("--bind-try", path.clone(), path.clone()));
    }
    for path in &host.umu_read_only {
        mounts.push(("--ro-bind-try", path.clone(), path.clone()));
    }
    if request.proton_path.starts_with('/') {
        let proton = PathBuf::from(&request.proton_path);
        mounts.push(("--ro-bind-try", proton.clone(), proton));
    }
    // A mount hides what was mounted below it before, so parents go first: a
    // share that holds the prefix must not cover it.
    mounts.sort_by(|a, b| a.2.cmp(&b.2));
    for (mode, source, target) in &mounts {
        bind(&mut args, mode, source, target);
    }

    // One wineserver per prefix: its socket directory and the shared memory
    // fsync keeps in /dev/shm are the holder's.
    // SAFETY: getuid cannot fail and takes no arguments.
    let wine_dir = format!("/tmp/.wine-{}", unsafe { libc::getuid() });
    bind(
        &mut args,
        "--bind",
        &host.shared_dir.join("wine"),
        Path::new(&wine_dir),
    );
    bind(
        &mut args,
        "--bind",
        &host.shared_dir.join("shm"),
        Path::new("/dev/shm"),
    );

    // Fonts, and the configuration of the overlays a game may run with.
    for relative in [
        ".config/MangoHud",
        ".config/fontconfig",
        ".config/pulse",
        ".local/share/fonts",
        ".fonts",
    ] {
        let path = host.home.join(relative);
        bind(&mut args, "--ro-bind-try", &path, &path);
    }

    // Display, sound, and the dead socket in place of the session bus.
    if let Some(socket) = &host.wayland_socket {
        bind(&mut args, "--bind-try", socket, socket);
    }
    if let Some(socket) = &host.x11_socket {
        bind(&mut args, "--bind-try", socket, socket);
    }
    if let Some(xauthority) = &host.xauthority {
        bind(&mut args, "--ro-bind-try", xauthority, xauthority);
    }
    for socket in ["pipewire-0", "pulse"] {
        let path = host.runtime_dir.join(socket);
        bind(&mut args, "--bind-try", &path, &path);
    }
    bind(
        &mut args,
        "--bind",
        &host.bus_socket,
        &host.runtime_dir.join("bus"),
    );

    if let Some(work_dir) = &request.work_dir {
        args.push("--chdir".into());
        args.push(work_dir.to_string_lossy().into_owned());
    }

    // Nothing of the daemon's environment leaks in: what is set here is all the
    // program has.
    args.push("--clearenv".into());
    for (key, value) in env {
        args.push("--setenv".into());
        args.push(key.clone());
        args.push(value.clone());
    }

    args.push("--seccomp".into());
    args.push(SECCOMP_FD.to_string());
    args.push("--".into());
    args
}

/// Session variables a game needs. Everything else the daemon was started with —
/// the real bus address, an SSH agent socket — stops here.
const SESSION_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "TZ",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "PULSE_SERVER",
    // How NixOS points at the graphics drivers on a system whose loaders are not
    // patched to find them by themselves.
    "LD_LIBRARY_PATH",
    // Which GPU and which driver, on a machine that has more than one.
    "DRI_PRIME",
    "__NV_PRIME_RENDER_OFFLOAD",
    "__GLX_VENDOR_LIBRARY_NAME",
    "__VK_LAYER_NV_optimus",
    "VK_DRIVER_FILES",
    "VK_ICD_FILENAMES",
    "LIBVA_DRIVER_NAME",
    "MESA_LOADER_DRIVER_OVERRIDE",
];

/// The environment a sandboxed program gets: the session variables above, the
/// ones the caller set on the command, and a bus address pointing at the socket
/// nothing listens on.
pub fn sandbox_env(command: &AsyncCommand, host: &HostLayout) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in SESSION_ENV {
        if let Ok(value) = std::env::var(key) {
            env.insert((*key).to_string(), value);
        }
    }
    // /run/current-system/sw/bin is where NixOS has the programs other systems
    // keep in /usr/bin, winetricks' helpers among them.
    env.insert(
        "PATH".to_string(),
        "/usr/bin:/bin:/run/current-system/sw/bin".to_string(),
    );
    for (key, value) in command.as_std().get_envs() {
        let key = key.to_string_lossy().into_owned();
        match value {
            Some(value) => env.insert(key, value.to_string_lossy().into_owned()),
            None => env.remove(&key),
        };
    }
    env.insert(
        "DBUS_SESSION_BUS_ADDRESS".to_string(),
        format!("unix:path={}", host.runtime_dir.join("bus").display()),
    );
    env
}

/// Wraps `command` in the sandbox and in the transient systemd scope `unit`.
///
/// The order is `systemd-run` → `bwrap` → the program: the scope stays outside
/// the sandbox, so the daemon still tracks and stops the whole tree through its
/// cgroup, and everything inside — any `%command%` wrapper included — runs
/// confined.
pub async fn confine_in_scope(
    command: &AsyncCommand,
    unit: &str,
    request: &SandboxRequest,
) -> Result<Confined, String> {
    if request.prefix_path.trim().is_empty() {
        return Err(gettext("Prefix path is required"));
    }
    let tools = tools()?;

    let directories = request.clone();
    let host = tokio::task::spawn_blocking(move || {
        let host = HostLayout::detect(bus::dead_socket()?);
        prepare_directories(&directories)?;
        Ok::<_, String>(host)
    })
    .await
    .map_err(prepare_error)??;

    let env = sandbox_env(command, &host);
    let filter = SeccompFilter::compile().map_err(prepare_error)?;

    let (holder, lease) = pidns::holder(&request.prefix_path).await?;
    let inner = command.as_std();
    let mut sandboxed = pidns::enter(&tools.nsenter, holder);
    sandboxed.arg(&tools.bwrap);
    sandboxed.args(bwrap_args(request, &host, &env));
    sandboxed.arg(inner.get_program());
    sandboxed.args(inner.get_args());
    if let Some(dir) = inner.get_current_dir() {
        sandboxed.current_dir(dir);
    }

    let mut scoped = crate::launch::in_scope(&sandboxed, unit);
    filter.place_on_fd(&mut scoped).map_err(prepare_error)?;

    Ok(Confined {
        command: scoped,
        lease,
    })
}

/// `bwrap` refuses a bind whose source is missing. The prefix too: a dependency
/// install that starts with `createprefix` runs in one that does not exist yet.
fn prepare_directories(request: &SandboxRequest) -> Result<(), String> {
    let prefix = Path::new(&request.prefix_path);
    for directory in [prefix.to_path_buf(), prefix.join(".leyen/cache")] {
        fs::create_dir_all(&directory).map_err(|e| {
            gettext("Failed to prepare the sandbox directory “{}”: {}")
                .replacen("{}", &directory.display().to_string(), 1)
                .replacen("{}", &e.to_string(), 1)
        })?;
    }
    Ok(())
}
