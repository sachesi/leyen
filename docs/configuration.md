# Configuration

Leyen stores configuration in:

```text
~/.config/leyen/
```

User data is stored in:

```text
~/.local/share/leyen/
```

## Files

### `~/.config/leyen/games.toml`

The game library. It stores root games, groups, per-game launch settings, Leyen IDs, playtime, and last-run state.

### `~/.config/leyen/settings.toml`

Global settings:

- default prefix path
- default Proton version
- global MangoHud toggle
- global GameMode toggle
- global Wayland toggle
- global WOW64 toggle
- global NTSYNC toggle
- detected Proton versions
- log settings

### `~/.config/leyen/running.toml`

Runtime state for currently managed games. Leyen updates this automatically.

### `~/.config/leyen/logs.jsonl`

JSON lines log storage used by the log window.

## Proton discovery

Leyen scans these locations:

```text
~/.local/share/leyen/proton/
~/.steam/steam/compatibilitytools.d/
~/.steam/steam/steamapps/common/
```

The local Leyen Proton directory is checked first. Put custom Proton or Proton-GE builds there when you want them independent from Steam.

## Prefixes

The default prefix directory is:

```text
~/.local/share/leyen/prefixes/default/
```

A game can use:

- its own prefix path
- its group's default prefix path
- the global default prefix path

The per-game value wins, then the group default, then the global default.

## Launch environment

Leyen launches games through `umu-run` and sets environment variables as needed:

- `WINEPREFIX`
- `PROTONPATH`
- `GAMEID`
- `MANGOHUD=1`
- `PROTON_ENABLE_WAYLAND`
- `PROTON_USE_WOW64`
- `PROTON_USE_NTSYNC`
- `WINENTSYNC`

When a prefix is already in use, Leyen may set:

```text
UMU_CONTAINER_NSENTER=1
```

This lets a second launch reuse the existing umu container context instead of fighting for the same prefix.

## Logging

The log settings control what Leyen prints to stderr. The log file is still used by the GUI log window for captured entries.

Useful while debugging:

- enable warnings
- enable operations
- run from a terminal
- open `leyen logs`
