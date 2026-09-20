# Troubleshooting

The log comes first: Logs in the main menu, filtered to the game, or `leyen logs` in a
terminal. What umu-launcher, Proton and the game print is there, with Leyen's own reasons for
refusing a launch.

## A launch is refused with "A systemd user session…"

Every game runs in a transient scope of the systemd user manager, which is how Leyen tracks
it and stops it, so there has to be one: `systemctl --user status` should answer. Inside a
container or over plain SSH there usually is none.

## A launch is refused with "bubblewrap is required…"

Games only run sandboxed, so `bwrap` has to be there: install `bubblewrap`. It is a
dependency of Leyen's packages; a build installed by hand may be missing it. The same goes for
`nsenter` from `util-linux`, which puts every program on a prefix in one process namespace, and
which every distribution has anyway.

## A launch is refused with "The sandbox could not be created"

The message carries what `bwrap` said. Almost always it is unprivileged user namespaces being
switched off: `sysctl kernel.unprivileged_userns_clone` should be `1` where the setting
exists, and `/proc/sys/user/max_user_namespaces` must not be `0`. Nothing in Leyen runs
Windows code without the sandbox, so this refuses every launch until it is fixed.

## A launch is refused with "Set this game's folder…"

The sandbox gives a game one folder of yours, and Leyen does not guess which: open the game's
settings and fill in **Game Folder**. A game added before the sandbox existed has none.

## A launch is refused with "The executable is outside the game folder"

The executable has to be inside the folder the game is given, or inside its prefix — a game
sees nothing else of yours, so an executable elsewhere could not be started even if Leyen
tried. Point **Game Folder** at the folder that holds the executable, or higher.

The two paths have to be written the same way, too. A folder shared with a game is there
under the path it was given, so a game folder named through a symlink and an executable
named through the folder it points at do not meet, and the refusal is the same. Write both
through the link, or both through the real path.

## A game does not find its own files

The sandbox gives a game one folder of yours: **Game Folder** in its settings. For a game
whose executable lives below its install root — `Binaries/Win64/game.exe` with the content
beside it — that has to be the root, not the folder the executable sits in. Anything else it
needs goes under **Extra Folders**. The log says what it was given: "Sandbox: network … |
folders …".

## An overlay, a chat client or a helper no longer reaches the game

The sandbox passes the display, sound and device sockets and nothing else, so anything else
that used to talk to a game through a socket of the session does not: Discord's rich presence,
OBS's game capture, a helper that attaches to a running game from outside. MangoHud works, and
reads your configuration in `~/.config/MangoHud`; a layer configured elsewhere in the home
directory, such as vkBasalt's, runs with its defaults. A wrapper in **Launch Arguments** has to
be a program the sandbox has — one from the system, not a script in your home directory — and
the folders under **Extra Folders** are how anything else of yours gets in.

## A game cannot reach the internet

**Network Access** for the game, its group, or all games under Sandbox in the preferences. The
narrowest answer wins. On an X11 session without a Wayland compositor, cutting the network can
also cut the X server off, because X11 clients reach it through an abstract socket that lives
with the network namespace.

## There is no GameMode switch

GameMode does not work from inside the sandbox. A game registers with `gamemoded` under its
own process ID, and the daemon looks that ID up on the host; a sandboxed game has a process
ID namespace of its own, so the two never agree. The way round that is to leave the game in
the host's namespace, where it sees every process of yours and can signal them — kill them
included — and to open a service on the session bus to it. Neither is worth a CPU governor
switch; set the power profile for the session instead.

## "Downloading umu-launcher…" does not go away

The daemon fetches umu-launcher and winetricks with `curl` and unpacks them with `tar`, into
`~/.local/share/leyen/core`, the first time it starts without them. Each is a release
pinned in Leyen and is refused if its SHA-256 does not match. Check the log for the
download; without network, install `umu-launcher` and `winetricks` from the distribution
instead, and Leyen uses the ones in `PATH`.

## "Leyen's daemon is not running and could not be started"

The window starts `leyend` through D-Bus activation, which needs the service file
`just install` puts in `share/dbus-1/services`. With a custom prefix other than `~/.local`, its
`share` directory has to be in `XDG_DATA_DIRS`. An uninstalled build can also run `target/debug/leyend` by
hand.

## A game is missing from the Proton list

The list is Default plus the directories in `~/.local/share/leyen/proton` that hold a
`proton` script and a `version` file; Steam's own Proton folders are not scanned. Unpack or
link the build there.

## Dependencies fail with "pressure-vessel-wrap" errors

Repair Runtime in the preferences deletes umu-launcher's `steamrt3`, which it downloads
again on the next install.

## Stopping one game takes down the others on its prefix

Games on one prefix share a wineserver, as Wine requires, and it runs with the game that
started first. Stopping that game from Leyen stops the wineserver too. Close it from inside
the game instead, and the others keep running.

## The prefix tools say a game is running

Nothing changes a prefix while a game runs, whichever prefix it uses: close the games first,
or stop them from Running Games.

## A game says a program is running in its prefix

Wine Configuration, the Registry Editor or a program run from the prefix tools is still open,
or something it started is: an installer often leaves a helper behind for a few seconds.
Close them and try again.

## A game still shows the old icon or name in the applications menu

The menu entry is rewritten when the game or its group is saved. Some menus cache icons
until the next login.
