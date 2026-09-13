# Settings and files

## Preferences

Ctrl+, or Preferences in the main menu. What is changed is saved when the dialog closes, and
the daemon reads it again at once.

- **Default Paths**: the prefix that games and groups without one of their own use,
  `~/.local/share/leyen/prefixes/default` to begin with, and the default Proton. Default
  leaves the choice to umu-launcher, which runs its own UMU-Proton build.
- **Tools** for the default prefix, the same as a game's: see
  [prefix tools](usage.md#prefix-tools).
- **Global Environment**: the switches a new game starts with. A game keeps its own
  switches afterwards; changing these does not change the games already in the library.
- **Logging**: which of Leyen's own errors, warnings and operations the log keeps. What
  games print is always kept.
- **Use Shared Container**: when a game launches while another on the same Wine prefix is
  running, it joins that game's pressure-vessel container (`UMU_CONTAINER_NSENTER`), which
  is what two programs sharing a prefix usually need. Off, it gets a container of its own
  on the same prefix.
- **Repair Runtime**: deletes umu-launcher's Steam Linux Runtime, `steamrt3`, which it
  downloads again the next time it is needed. The cure for "pressure-vessel-wrap" errors
  while installing dependencies. Refused while a game runs.

## Where things are

    ~/.config/leyen/games.toml       the library: games, groups and their settings
    ~/.config/leyen/settings.toml    the preferences
    ~/.config/leyen/running.toml     what the daemon is tracking, so it survives a restart
    ~/.config/leyen/logs.jsonl       the log
    ~/.local/share/leyen/prefixes/   the default prefix and, usually, the others
    ~/.local/share/leyen/proton/     Proton builds offered next to Default
    ~/.local/share/leyen/core/       umu-launcher and winetricks, when not installed
    ~/.local/share/icons/hicolor/256x256/apps/   game and group icons
    ~/.local/share/applications/     the menu entries of games

The library and the preferences are written whole to a temporary file and renamed over the
old one, so a crash cannot leave them half written. Only the daemon writes the library; the
window asks it to, and a change made elsewhere in the meantime is refused rather than
overwritten.

## Proton

The Proton list is Default followed by every directory in `~/.local/share/leyen/proton`
that holds a `proton` script and a `version` file, such as an unpacked GE-Proton release.
A game uses its own choice, then its group's, then the default; Default all the way down
lets umu-launcher pick.

## Prefixes

A game uses its own prefix, then its group's, then the default. The dependencies installed
through Leyen are recorded in the prefix itself, in `.leyen/deps/state.toml`, so a prefix
shared by several games shows the same components for all of them. Downloads for them are
cached in `~/.local/share/leyen/deps/cache`.
