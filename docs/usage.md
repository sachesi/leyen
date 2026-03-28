# Usage

Leyen is a GTK4 app for managing Windows games. Each game gets a stable ID (`ly-1234`) for tracking and launching.

## Managing Games

Add a game: click **+**, pick a Windows executable (`.exe`).

### Settings
- **Prefixes:** Isolated or shared Wine prefix per game.
- **Proton:** Official Steam Proton builds plus locally installed GE-Proton builds.
- **Toggles:** MangoHud, GameMode, Wayland, NTSYNC.

## Groups

Groups categorize games and set default Proton and prefix. Games inherit group defaults unless overridden per-game.

## Launching

### GUI
Click **Play** on a game card. Playtime tracks while the game process runs.

### CLI
```bash
# List games and IDs
leyen list

# Launch a game
leyen run <id>

# Stop a game
leyen kill <id>
```

## Logs

Logs include both Leyen output and game output. Open the log viewer in the UI or run:
```bash
leyen logs
```

## Launch Arguments

Supports standard Windows CLI args and Steam-style `%command%`:
`DXVK_CONFIG_FILE=/path/to/dxvk.conf %command% -windowed`

## Desktop Integration

Generate `.desktop` files in `~/.local/share/applications/` — launch games from your system app menu.
