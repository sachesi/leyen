# Installing

## What you need

To build: Rust 1.92 or newer, `blueprint-compiler`, `just`, `msgfmt` from gettext, and the
development packages of GTK 4.22 and libadwaita 1.9. On Fedora that is `rust cargo
gtk4-devel libadwaita-devel blueprint-compiler just gettext`; on Arch `rust gtk4 libadwaita
blueprint-compiler just gettext`; on Debian and Ubuntu `cargo libgtk-4-dev libadwaita-1-dev
blueprint-compiler just gettext`, on a release that ships GTK 4.22 and libadwaita 1.9.
`just check` also wants `desktop-file-validate` and `appstreamcli`.

To run: GTK 4.22, libadwaita 1.9, a session bus and a systemd user session. Every game
runs in a transient systemd scope, which is how Leyen knows what belongs to it and what to
stop; without `systemctl --user` a launch or a dependency install is refused. `curl` and `tar` fetch umu-launcher
and winetricks the first time they are needed, unless `umu-run` and `winetricks` are
already in `PATH`. MangoHud is optional: its switch appears once `mangohud` is installed.

## Packages

Packages for Fedora, openSUSE, Debian, Ubuntu and Arch Linux, and how to install them, are in
the [README](../README.md#packages). Nix: the flake's package goes in `environment.systemPackages` or `nix
profile install github:sachesi/leyen`; `nix run` does not work, because the session bus only
starts the daemon from a D-Bus service file in an installed `share` directory. On NixOS put
`umu-launcher` and `winetricks` in `environment.systemPackages` as well: there they come from
the system, and Leyen downloads neither. `nix develop`
gives a shell with the build tools. There is no Flatpak: games run in systemd scopes of the
user session, which a sandbox cannot create.

## Build

    just build          # release
    just build-debug
    just run            # debug build, uninstalled
    just check          # rustfmt, clippy, blueprint, desktop entry, metainfo, catalogues
    just test

The build produces three programs: `leyen-gtk`, the window; `leyend`, the daemon that
launches and tracks games; and `leyen`, the command line. The window and the command line
start the daemon over D-Bus when they need it, and it exits again when it has been idle
for a while with nothing running.

An uninstalled `just run` works as long as the daemon can be started: install once, or
start `target/debug/leyend` by hand in another terminal.

## Install

    just install
    just prefix=$HOME/.local install
    DESTDIR=/tmp/stage just install

`install` copies what `just build` produced, building it first only when a binary is
missing. The default prefix is `/usr`, and `sudo` is asked for only when the prefix is not
writable by you. It puts the three programs in `bin`, and under `share` the desktop entry
and metainfo with their translations, the icons, the D-Bus service that starts the daemon,
the shell completions and the compiled catalogues; then it refreshes the desktop and icon
caches.

A daemon still running the old binary is stopped so that the next call starts the new
one, unless it is tracking a game: then it is left alone and says so, and picks up the new
binary after the games end. An install from before the application id changed to
`io.github.sachesi.leyen` leaves files under the old id; `install` removes them.

The D-Bus service file gets the real path to `leyend`, so a custom prefix works as long as
the session bus looks in its `share` directory: `~/.local/share` always, as the data home,
and any other when it is in `XDG_DATA_DIRS`, as `/usr/local/share` is on most systems.

## Remove

    just uninstall
    just prefix=$HOME/.local uninstall

Your library, settings, prefixes and logs stay in `~/.config/leyen` and
`~/.local/share/leyen`.
