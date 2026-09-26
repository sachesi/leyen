use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use leyen_model::models::SandboxFolder;

use super::{
    HostLayout, SandboxRequest, Share, bwrap_args, check_folder, find_program_unresolved,
    is_available, shares_from,
};

fn host(prefix: &Path) -> HostLayout {
    HostLayout {
        home: PathBuf::from("/home/player"),
        runtime_dir: PathBuf::from("/run/user/1000"),
        bus_socket: PathBuf::from("/run/user/1000/leyen/bus"),
        shared_dir: PathBuf::from("/run/user/1000/leyen/shared"),
        wayland_socket: Some(PathBuf::from("/run/user/1000/wayland-0")),
        x11_socket: None,
        xauthority: None,
        resolv_conf: Some(PathBuf::from("/run/systemd/resolve/stub-resolv.conf")),
        devices: vec![PathBuf::from("/dev/dri")],
        umu_writable: vec![PathBuf::from("/home/player/.local/share/umu")],
        umu_read_only: vec![prefix.parent().unwrap().join("core")],
    }
}

fn request(prefix: &Path) -> SandboxRequest {
    SandboxRequest {
        prefix_path: prefix.to_string_lossy().into_owned(),
        proton_path: "/home/player/.local/share/leyen/proton/GE-Proton".to_string(),
        work_dir: None,
        shares: Vec::new(),
        network: true,
    }
}

/// `--bind SOURCE TARGET` triples, whatever the bind mode.
fn binds(args: &[String]) -> Vec<(String, String, String)> {
    args.windows(3)
        .filter(|window| window[0].starts_with("--") && window[0].contains("bind"))
        .map(|window| (window[0].clone(), window[1].clone(), window[2].clone()))
        .collect()
}

fn has_pair(args: &[String], first: &str, second: &str) -> bool {
    args.windows(2)
        .any(|window| window[0] == first && window[1] == second)
}

#[test]
fn the_home_directory_is_a_tmpfs_and_only_named_paths_come_back() {
    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let args = bwrap_args(&request(prefix), &host(prefix), &BTreeMap::new());

    assert!(has_pair(&args, "--tmpfs", "/home/player"));
    assert!(has_pair(&args, "--tmpfs", "/run/user/1000"));
    assert!(has_pair(&args, "--tmpfs", "/tmp"));
    // The home directory itself is never a bind source, so nothing in it is
    // visible beyond the paths bound back in afterwards.
    for (mode, source, _) in binds(&args) {
        assert_ne!(source, "/home/player", "home bound with {mode}");
    }
    // The prefix is read-write, and the cache the sandbox shows as ~/.cache is
    // inside it rather than the user's own.
    assert!(binds(&args).contains(&(
        "--bind".to_string(),
        prefix.to_string_lossy().into_owned(),
        prefix.to_string_lossy().into_owned(),
    )));
    assert!(binds(&args).contains(&(
        "--bind".to_string(),
        format!("{}/.leyen/cache", prefix.display()),
        "/home/player/.cache".to_string(),
    )));
    // Leyen's own configuration is in neither list: with $HOME a tmpfs, a game
    // cannot read or rewrite what Leyen launches next.
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("/.config/leyen") || arg.contains("games.toml"))
    );
}

#[test]
fn a_folder_inside_the_prefix_is_not_bound_over_it() {
    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let inside = prefix.join("drive_c/Game");
    let mut request = request(prefix);
    request.shares = vec![Share::read_only(inside.clone())];
    let args = bwrap_args(&request, &host(prefix), &BTreeMap::new());
    assert!(
        !binds(&args)
            .iter()
            .any(|(_, source, _)| source == &inside.to_string_lossy())
    );
}

#[test]
fn the_session_bus_is_replaced_by_a_socket_with_nothing_behind_it() {
    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let host = host(prefix);
    let mut env = BTreeMap::new();
    env.insert(
        "DBUS_SESSION_BUS_ADDRESS".to_string(),
        "unix:path=/run/user/1000/bus".to_string(),
    );
    let args = bwrap_args(&request(prefix), &host, &env);

    // What the sandbox sees at the bus address is Leyen's own socket, and the
    // session bus socket is never a bind source.
    assert!(binds(&args).contains(&(
        "--bind".to_string(),
        "/run/user/1000/leyen/bus".to_string(),
        "/run/user/1000/bus".to_string(),
    )));
    assert!(
        !binds(&args)
            .iter()
            .any(|(_, source, _)| source == "/run/user/1000/bus")
    );
}

/// The whole of what a program gets in place of a session bus: an address that
/// refuses the connection, as on a machine running without one.
#[test]
fn the_bus_socket_refuses_a_connection() {
    let temp = tempfile::tempdir().expect("temp dir");
    let socket = super::bus::place_dead_socket(temp.path().join("bus")).expect("a socket");
    assert!(std::os::unix::net::UnixStream::connect(&socket).is_err());
    // Already there is not an error: every sandbox of the session binds this one.
    assert_eq!(
        super::bus::place_dead_socket(socket.clone()).expect("the same socket"),
        socket
    );
}

#[test]
fn what_the_program_may_reach_follows_the_request() {
    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let host = host(prefix);

    let online = bwrap_args(&request(prefix), &host, &BTreeMap::new());
    assert!(!online.iter().any(|arg| arg == "--unshare-net"));
    // The PID namespace is the prefix's holder's, entered before bwrap runs.
    assert!(!online.iter().any(|arg| arg == "--unshare-pid"));
    assert!(binds(&online).contains(&(
        "--bind".to_string(),
        "/run/user/1000/leyen/shared/shm".to_string(),
        "/dev/shm".to_string(),
    )));

    let mut offline = request(prefix);
    offline.network = false;
    let offline = bwrap_args(&offline, &host, &BTreeMap::new());
    assert!(offline.iter().any(|arg| arg == "--unshare-net"));
}

#[test]
fn every_sandbox_carries_the_seccomp_filter_and_a_clean_environment() {
    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let mut env = BTreeMap::new();
    env.insert("WINEPREFIX".to_string(), prefix.display().to_string());
    let args = bwrap_args(&request(prefix), &host(prefix), &env);

    assert!(has_pair(&args, "--seccomp", "3"));
    assert!(args.iter().any(|arg| arg == "--clearenv"));
    assert!(args.iter().any(|arg| arg == "--unshare-user"));
    assert_eq!(args.last().map(String::as_str), Some("--"));
    // The environment is set after --clearenv, so nothing of the daemon's own
    // survives into the program.
    let clearenv = args.iter().position(|arg| arg == "--clearenv").unwrap();
    let setenv = args.iter().position(|arg| arg == "--setenv").unwrap();
    assert!(clearenv < setenv);
}

/// The layout as `bwrap` actually applies it: needs bubblewrap and unprivileged
/// user namespaces, so it is skipped where there are none (a CI container).
#[test]
fn a_program_in_the_sandbox_sees_the_prefix_and_not_the_home_directory() {
    if let Err(reason) = is_available() {
        eprintln!("skipped: {reason}");
        return;
    }
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    let prefix = temp.path().join("prefix");
    let program_dir = temp.path().join("game");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "secret").unwrap();
    std::fs::create_dir_all(&prefix).unwrap();
    std::fs::create_dir_all(&program_dir).unwrap();
    std::fs::write(program_dir.join("game.exe"), "MZ").unwrap();
    // Stands in for the bus socket: what matters here is that the bind works.
    let bus = temp.path().join("bus");
    std::fs::write(&bus, "").unwrap();
    std::fs::create_dir_all(prefix.join(".leyen/cache")).unwrap();

    let mut request = request(&prefix);
    request.proton_path = String::new();
    request.shares = vec![Share::read_only(program_dir.clone())];
    let mut host = host(&prefix);
    host.home = home.clone();
    host.bus_socket = bus;
    host.shared_dir = temp.path().join("shared");
    std::fs::create_dir_all(host.shared_dir.join("wine")).unwrap();
    std::fs::create_dir_all(host.shared_dir.join("shm")).unwrap();
    host.wayland_socket = None;
    host.umu_writable = Vec::new();
    host.umu_read_only = Vec::new();
    let args = bwrap_args(&request, &host, &BTreeMap::new());

    let filter = super::SeccompFilter::compile().expect("filter");
    let mut command = std::process::Command::new(super::tools().unwrap().bwrap.clone());
    command.args(&args);
    command.args([
        "/bin/sh",
        "-c",
        // The prefix takes writes, the game folder does not, the home directory
        // is empty.
        "set -e; touch \"$1/written\"; test -r \"$2/game.exe\"; ! touch \"$2/new\" 2>/dev/null; test ! -e \"$3/.ssh/id_ed25519\"",
        "sh",
    ]);
    command.arg(&prefix);
    command.arg(&program_dir);
    command.arg(&home);
    filter.place_on_std_fd(&mut command).expect("filter fd");
    let output = command.output().expect("bwrap runs");
    assert!(
        output.status.success(),
        "sandbox failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(prefix.join("written").exists());
    assert!(!program_dir.join("new").exists());
}

fn folder(path: &Path, writable: bool) -> SandboxFolder {
    SandboxFolder {
        path: path.to_string_lossy().into_owned(),
        writable,
    }
}

#[test]
fn a_shared_folder_is_read_only_unless_marked_writable() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mods = temp.path().join("mods");
    let assets = temp.path().join("assets");
    for folder in [&mods, &assets] {
        std::fs::create_dir_all(folder).unwrap();
    }

    let (shares, refused) = shares_from(&[folder(&mods, true), folder(&assets, false)]);
    assert!(refused.is_empty(), "{refused:?}");

    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let mut request = request(prefix);
    request.shares = shares;
    let args = bwrap_args(&request, &host(prefix), &BTreeMap::new());

    let binds = binds(&args);
    assert!(binds.iter().any(|(mode, source, target)| {
        mode == "--bind-try" && source == &mods.to_string_lossy() && target == source
    }));
    assert!(binds.iter().any(|(mode, source, target)| {
        mode == "--ro-bind-try" && source == &assets.to_string_lossy() && target == source
    }));
}

#[test]
fn a_folder_that_would_undo_the_sandbox_is_refused() {
    let home = std::env::var("HOME").expect("a home directory");

    // The home directory itself, and anything holding it: the tmpfs over $HOME
    // is the whole point, and a bind of an ancestor brings it back.
    for path in [home.as_str(), "/", "/home", "/etc", "/usr", "/nix"] {
        if !Path::new(path).is_dir() {
            continue;
        }
        assert!(
            check_folder(&SandboxFolder {
                path: path.to_string(),
                writable: false,
            })
            .is_err(),
            "sharing {path} must be refused"
        );
    }

    // Leyen's own configuration decides what Leyen launches next.
    let config = leyen_model::paths::get_config_dir();
    assert!(check_folder(&folder(&config, false)).is_err());

    // A relative path, and one that is not there at all.
    assert!(
        check_folder(&SandboxFolder {
            path: "games/mods".to_string(),
            writable: false,
        })
        .is_err()
    );
    assert!(
        check_folder(&SandboxFolder {
            path: "/nonexistent-leyen-test-folder".to_string(),
            writable: false,
        })
        .is_err()
    );
}

#[test]
fn a_symlink_cannot_smuggle_in_a_refused_folder() {
    let temp = tempfile::tempdir().expect("temp dir");
    let link = temp.path().join("shortcut");
    std::os::unix::fs::symlink(std::env::var("HOME").expect("a home directory"), &link).unwrap();
    assert!(
        check_folder(&folder(&link, false)).is_err(),
        "a link to the home directory is the home directory"
    );
}

/// The Proton a game runs with is often a link to the build of the day. It is
/// bound where the game was told to look, with the build itself as the source.
#[test]
fn a_proton_kept_as_a_symlink_is_bound_where_the_game_looks_for_it() {
    let temp = tempfile::tempdir().expect("temp dir");
    let build = temp.path().join("GE-Proton10-3");
    let latest = temp.path().join("ge-proton-latest");
    std::fs::create_dir_all(&build).unwrap();
    std::os::unix::fs::symlink(&build, &latest).unwrap();

    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let mut request = request(prefix);
    request.proton_path = latest.to_string_lossy().into_owned();
    let args = bwrap_args(&request, &host(prefix), &BTreeMap::new());

    assert!(binds(&args).contains(&(
        "--ro-bind-try".to_string(),
        build.to_string_lossy().into_owned(),
        latest.to_string_lossy().into_owned(),
    )));
    // Nothing brings the directory holding the link into the sandbox, so no
    // other build of Proton is in there with it.
    assert!(
        !binds(&args)
            .iter()
            .any(|(_, source, _)| source == &temp.path().to_string_lossy())
    );
}

#[test]
fn a_sandbox_starts_with_a_proton_that_is_a_symlink() {
    if let Err(reason) = is_available() {
        eprintln!("skipped: {reason}");
        return;
    }
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    let prefix = temp.path().join("prefix");
    let build = temp.path().join("proton/GE-Proton10-3");
    let latest = temp.path().join("proton/ge-proton-latest");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(prefix.join(".leyen/cache")).unwrap();
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("proton"), "#!/usr/bin/env python3\n").unwrap();
    std::os::unix::fs::symlink(&build, &latest).unwrap();
    let bus = temp.path().join("bus");
    std::fs::write(&bus, "").unwrap();

    let mut request = request(&prefix);
    request.proton_path = latest.to_string_lossy().into_owned();
    let mut host = host(&prefix);
    host.home = home;
    host.bus_socket = bus;
    host.shared_dir = temp.path().join("shared");
    std::fs::create_dir_all(host.shared_dir.join("wine")).unwrap();
    std::fs::create_dir_all(host.shared_dir.join("shm")).unwrap();
    host.wayland_socket = None;
    host.umu_writable = Vec::new();
    host.umu_read_only = Vec::new();
    let args = bwrap_args(&request, &host, &BTreeMap::new());

    let filter = super::SeccompFilter::compile().expect("filter");
    let mut command = std::process::Command::new(super::tools().unwrap().bwrap.clone());
    command.args(&args);
    command.args(["/bin/sh", "-c", "test -f \"$1/proton\"", "sh"]);
    command.arg(&latest);
    filter.place_on_std_fd(&mut command).expect("filter fd");
    let output = command.output().expect("bwrap runs");
    assert!(
        output.status.success(),
        "the game must find Proton at the path it was given: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// On NixOS `/usr` holds only `env`: the loader, the libraries, the programs and
/// the graphics drivers are all reached through paths other systems do not have.
#[test]
fn the_store_the_drivers_and_the_system_profile_are_bound_for_nixos() {
    let prefix = Path::new("/home/player/.local/share/leyen/prefixes/game");
    let args = bwrap_args(&request(prefix), &host(prefix), &BTreeMap::new());

    for path in [
        "/nix",
        "/run/opengl-driver",
        "/run/opengl-driver-32",
        "/run/current-system",
    ] {
        assert!(
            binds(&args)
                .iter()
                .any(|(mode, _, target)| mode == "--ro-bind-try" && target == path),
            "{path} is not bound read-only"
        );
    }

    let env = super::sandbox_env(&tokio::process::Command::new("true"), &host(prefix));
    assert!(
        env["PATH"]
            .split(':')
            .any(|dir| dir == "/run/current-system/sw/bin"),
        "PATH misses the system profile: {}",
        env["PATH"]
    );
}

/// The holder reaches its program through a whole root, so it runs the path as
/// installed. Resolved, `sleep` on NixOS is coreutils' single binary, which under
/// that name only prints a usage error.
#[test]
fn the_program_the_holder_runs_keeps_the_name_it_was_found_by() {
    let sleep = find_program_unresolved("sleep").expect("sleep is installed");

    assert_eq!(
        sleep.file_name().expect("a file name"),
        "sleep",
        "the holder would run {} instead of sleep",
        sleep.display()
    );
}

#[test]
fn a_share_holding_the_prefix_is_mounted_before_it() {
    let prefix = Path::new("/mnt/games/prefixes/game");
    let mut request = request(prefix);
    request.shares = vec![Share::read_only("/mnt/games")];
    let args = bwrap_args(&request, &host(prefix), &BTreeMap::new());

    let targets: Vec<String> = binds(&args)
        .into_iter()
        .map(|(_, _, target)| target)
        .collect();
    let share = targets.iter().position(|t| t == "/mnt/games").unwrap();
    let bound_prefix = targets
        .iter()
        .position(|t| t == &prefix.to_string_lossy())
        .unwrap();
    assert!(share < bound_prefix, "the share would cover the prefix");
}

/// The folder behind the sandbox's ~/.cache is a bind source the next launch
/// resolves on the host, so a program must not be able to swap it for a link to
/// the real home directory.
#[test]
fn a_program_cannot_turn_its_cache_into_a_link_out_of_the_sandbox() {
    if let Err(reason) = is_available() {
        eprintln!("skipped: {reason}");
        return;
    }
    let temp = tempfile::tempdir().expect("temp dir");
    let home = temp.path().join("home");
    let prefix = temp.path().join("prefix");
    std::fs::create_dir_all(&home).unwrap();
    let bus = temp.path().join("bus");
    std::fs::write(&bus, "").unwrap();

    let mut request = request(&prefix);
    request.proton_path = String::new();
    let mut host = host(&prefix);
    host.home = home.clone();
    host.bus_socket = bus;
    host.shared_dir = temp.path().join("shared");
    std::fs::create_dir_all(host.shared_dir.join("wine")).unwrap();
    std::fs::create_dir_all(host.shared_dir.join("shm")).unwrap();
    host.wayland_socket = None;
    host.umu_writable = Vec::new();
    host.umu_read_only = Vec::new();
    super::prepare_directories(&request).expect("directories");
    let args = bwrap_args(&request, &host, &BTreeMap::new());

    let filter = super::SeccompFilter::compile().expect("filter");
    let mut command = std::process::Command::new(super::tools().unwrap().bwrap.clone());
    command.args(&args);
    command.args([
        "/bin/sh",
        "-c",
        "set -e; touch \"$2/.cache/written\"; \
         ! rm -rf \"$1/.leyen/cache\" 2>/dev/null; ! ln -s \"$2\" \"$1/.leyen/link\" 2>/dev/null; \
         ! mv \"$1/.leyen\" \"$1/moved\" 2>/dev/null",
        "sh",
    ]);
    command.arg(&prefix);
    command.arg(&home);
    filter.place_on_std_fd(&mut command).expect("filter fd");
    let output = command.output().expect("bwrap runs");
    assert!(
        output.status.success(),
        "sandbox failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(prefix.join(".leyen/cache/written").exists());
    assert!(
        std::fs::symlink_metadata(prefix.join(".leyen/cache"))
            .unwrap()
            .is_dir()
    );

    // One left behind by a program from before is refused, not followed.
    std::fs::remove_dir_all(prefix.join(".leyen/cache")).unwrap();
    std::os::unix::fs::symlink(&home, prefix.join(".leyen/cache")).unwrap();
    assert!(super::prepare_directories(&request).is_err());
}

/// The prefix is bound read-write, so it is held to the rule a shared folder is.
#[test]
fn a_prefix_that_would_undo_the_sandbox_is_refused() {
    let home = std::env::var("HOME").expect("a home directory");
    for prefix in [home.as_str(), "/", "games/prefix"] {
        assert!(
            super::prepare_directories(&request(Path::new(prefix))).is_err(),
            "a prefix at {prefix} must be refused"
        );
    }
}
