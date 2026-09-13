# Leyen

Leyen keeps a library of Windows games and runs them with Proton through umu-launcher,
written in Rust with GTK 4 and libadwaita. Each game gets its own Wine prefix or shares one
with its group, and a daemon on the session bus launches, tracks and stops them, so the
window, the applications menu and the command line see the same running games.

You need GTK 4.22, libadwaita 1.9, a session bus and a systemd user session.

Groups with a prefix and a Proton their games inherit, playtime and the last session of
every game, per-game launch arguments with `%command%`, MangoHud, GameMode, Wayland, WoW64,
NTSync and HDR switches, winetricks components managed per prefix with dependencies between
them, the Wine configuration and the registry editor for any prefix, a live log for every
game, menu entries that start a game from the desktop, and `leyen list`, `run`, `kill` and
`logs` for the terminal. umu-launcher and winetricks are fetched when they are not
installed.

## Building and installing

    just build
    just install        # or: just prefix=$HOME/.local install

Build needs Rust 1.92, `blueprint-compiler`, `just`, gettext and the development packages for
GTK 4.22 and libadwaita 1.9. Details, other prefixes and removal are in
[docs/installing.md](docs/installing.md).

## Documentation

- [Installing](docs/installing.md)
- [Using Leyen](docs/usage.md), including [keyboard shortcuts](docs/keyboard-shortcuts.md)
- [Settings and files](docs/settings.md)
- [The command line](docs/command-line.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Contributing](CONTRIBUTING.md), including where things are in the code, and
  [reporting a vulnerability](SECURITY.md)

The interface is available in English and Ukrainian.

GPL-3.0-or-later.
