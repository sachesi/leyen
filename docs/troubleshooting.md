# Troubleshooting

The log comes first: Logs in the main menu, filtered to the game, or `leyen logs` in a
terminal. What umu-launcher, Proton and the game print is there, with Leyen's own reasons for
refusing a launch.

## A launch is refused with "A systemd user session…"

Every game runs in a transient scope of the systemd user manager, which is how Leyen tracks
it and stops it, so there has to be one: `systemctl --user status` should answer. Inside a
container or over plain SSH there usually is none.

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
