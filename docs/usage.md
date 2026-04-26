# Usage

## Start the GUI

```bash
leyen
```

The main window shows root games and groups. Running games are sorted near the top and show an elapsed running time.

## Add a game

1. Press the add button.
2. Choose a Windows executable.
3. Set a title.
4. Choose a prefix and Proton version, or keep the defaults.
5. Add launch arguments when needed.
6. Enable per-game options such as MangoHud, GameMode, Wayland, WOW64, or NTSYNC.
7. Save the game.

Each game receives a stable Leyen ID like `ly-1234`. This ID is used by the CLI and generated desktop entries.

## Groups

Groups can hold games and optional launch defaults:

- default prefix path
- default Proton version

A game inside a group can still override its own prefix or Proton version.

## Launch and stop games

From the GUI, press the play button on a game card. When the game is running, the same primary action becomes stop.

From the terminal:

```bash
leyen list
leyen run ly-1234
leyen kill ly-1234
```

`leyen run` starts a managed launch helper and returns control to the shell. Leyen keeps a small runtime registry in its config directory so the GUI and CLI can see running games.

## Logs

Open logs directly:

```bash
leyen logs
```

Logs include Leyen operation messages and captured stdout/stderr from managed game launches when logging is enabled.

## Launch arguments

Simple arguments are appended to the executable launch:

```text
-windowed -dx11
```

Leyen also understands `%command%` in Steam-style launch arguments. Tokens before `%command%` are treated as wrappers or environment variables, and tokens after it are passed after the executable.

Example:

```text
DXVK_CONFIG_FILE=/home/user/.dxvk/dxvk.conf %command% -skiplauncher
```

## Desktop entries

When a desktop entry is created for a game, it uses:

```text
Exec=leyen run <leyen-id>
```

The entry is written under:

```text
~/.local/share/applications/
```
