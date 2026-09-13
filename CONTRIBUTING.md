# Contributing

Bugs and ideas go to the [issue tracker](https://github.com/sachesi/leyen/issues); security
problems do not, see [SECURITY.md](SECURITY.md).

Before a change goes in:

- `just check` and `just test` pass. CI runs both on Fedora 44, with `cargo deny check`, for
  every push and pull request.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `fix:`, `feat:`, `perf:`, `docs:` and so on, with a subject that says what changed for
  someone using Leyen.
- Every string the user sees goes through `gettext`. `just po` updates the catalogues in
  `po/`, and a change that adds strings brings their translations along where it can.
- Behaviour described in `docs/` changes with the code that implements it.

## Where things are

The workspace has one crate per program and two shared ones:

    crates/leyen-model     the data every program shares: the library and settings formats
                           and their files, the dependency catalogue, where things live,
                           gettext setup. No tokio, no GTK.
    crates/leyen-core      the engine, used by the daemon: launching games in systemd
                           scopes and tracking them (launch.rs), umu-launcher and winetricks
                           (runtime/), dependency installs (deps/), the log (logging.rs)
    crates/leyen-ipc       the D-Bus interface: the proxy, the types on the wire, the errors;
                           the XML beside it describes the same interface
    crates/leyend          the daemon: owns the session bus name, serves the interface,
                           writes the library, exits when idle
    crates/leyen           the command line
    crates/leyen-gtk       the window

And in `leyen-gtk`:

    build.rs               runs blueprint-compiler and bundles the GResource
    data/ui/*.blp          the window, its rows, the dialogs, the log and running-games
                           windows, the shortcuts dialog
    data/style.css         structural CSS; colours come from libadwaita
    src/application.rs     AdwApplication subclass, app actions, starts the daemon bridge
    src/window.rs          the library and a group's page, the win.* actions
    src/game_row.rs, group_row.rs, library_icon.rs
                           the rows of the library and their icons
    src/dialogs/           adding and editing games and groups, the preferences, the
                           prefix tools they share and the dependency manager they open
    src/log_window.rs, running_games.rs
                           the two secondary windows
    src/daemon.rs          the bridge to the daemon (below)
    src/playback.rs        launching and stopping, one request per game at a time
    src/desktop.rs         menu entries; icons.rs the icons read from executables
    src/prefix_tools.rs    winecfg, regedit and programs run in a prefix
    src/migrate.rs         the move from the com.github application id

Widgets are GObject subclasses with composite templates from the Blueprint files. Actions
use the usual prefixes: `app.`, `win.` on the window, and `deps.` and `running.` on the
dependency page and the running-games window, where row buttons carry the id they act on
as the action's target.

The window runs no tokio. `daemon.rs` keeps a thread with a tokio runtime that owns the D-Bus
connection; the window sends it commands over a channel and awaits the reply, and the
daemon's signals come back as `DaemonEvent`s, fanned out to whoever subscribed. Every call
has a timeout. Local work that blocks — reading the library, decoding an icon, writing a
menu entry — goes to `gio_blocking`, which runs it on a thread and returns the result to
the main loop.

The daemon is the only writer of the library. A client reads `games.toml` directly and
saves through `SaveLibrary` with the version it read; a save built on an older version is
refused, and the client reloads. Settings are written by the window and re-read by the
daemon when told to.

## Running

    just run             # the window, debug build, uninstalled
    just check           # fmt, clippy -D warnings, blueprint, validators, catalogues
    just test            # the unit tests
    just smoke           # the daemon's D-Bus interface, on a private session bus
    cargo deny check     # advisories, licences and sources of the dependencies

The window logs through GLib: warnings and errors print by themselves, the rest with
`G_MESSAGES_DEBUG=all`. The daemon's log is what Logs in the window shows.

Games cannot be launched without a systemd user session; a container or a CI runner usually
has none, and the daemon refuses the launch there.

## A few things to know before changing them

User-visible strings go through `gettext` with `{}` placeholders filled by `replacen`;
there is no format string from Rust. Counts use `ngettext` even where English would not need
it, because the plural rules of other languages do. `just pot` reads the Rust sources with
xgettext's C parser, so a translatable string is one literal: a `\` line continuation
would keep the next line's indentation in the message. `just pot` regenerates the template
from the Rust sources, the Blueprint files, the desktop entry and the metainfo, and `just
po` merges it into every `po/<lang>.po`. A new language is a new line in `po/LINGUAS` plus
the `.po` file. `install` compiles the catalogues and merges the desktop and metainfo
translations with `msgfmt`.

A game's processes are whatever lives in its systemd scope, plus, for a game that joined
another's pressure-vessel container, the processes there that match its executable,
arguments and `GAMEID`. Stopping a game kills those and nothing else; read the comments on
`RunningGameSession` and `stop_game` in `launch.rs` before touching either.
