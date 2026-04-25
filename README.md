# Leyen - umu-launcher GUI

A modern GTK4/Libadwaita frontend for managing Wine/Proton games using umu-launcher, similar to Faugus Launcher.

## Features

### Core Features
- **Game Library Management**
  - Add, edit, and delete games from your library
  - Stable `Leyen ID` per game for CLI launch and listing
  - Persistent storage of game configurations
  - Clean, modern GTK4/Libadwaita interface

- **Wine/Proton Support**
  - Automatic detection of installed Proton versions
  - Per-game Proton version selection
  - Support for GE-Proton and Steam's Proton
  - Custom Wine prefix management per game

- **Launch Configuration**
  - Custom launch arguments per game
  - File browser for selecting executables
  - Per-game prefix paths (WINEPREFIX)
  - Environment variable management

- **Tool Integration**
  - **MangoHud**: Performance overlay toggle (per-game or global)
  - **GameMode**: Performance optimization wrapper
  - **Winetricks**: Built-in winetricks launcher for installing dependencies

- **Global Settings**
  - Default prefix path configuration
  - Default Proton version selection
  - Global MangoHud and GameMode toggles
  - Automatic Proton version scanning

### User Interface
- Modern Libadwaita design following GNOME HIG
- Toast notifications for user feedback
- Confirmation dialogs for destructive actions
- File picker dialogs for easy executable selection
- Responsive layout with scrollable game list

## Prerequisites

### Runtime Dependencies
- GTK4 (>= 4.12)
- Libadwaita (>= 1.4)
- umu-launcher (for game launching)
- Optional: MangoHud, GameMode, winetricks

### Build Dependencies
- Rust (2024 edition)
- pkg-config
- GTK4 development files
- Libadwaita development files

### Installation (Arch Linux)
```bash
sudo pacman -S gtk4 libadwaita rust
```

### Installation (Fedora)
```bash
sudo dnf install gtk4-devel libadwaita-devel rust cargo
```

### Installation (Ubuntu/Debian)
```bash
sudo apt install libgtk-4-dev libadwaita-1-dev rustc cargo
```

## Building

```bash
# Clone the repository
git clone https://github.com/sachesi/leyen.git
cd leyen

# Build the project
cargo build --release

# Run the application
cargo run --release
```

The compiled binary will be available at `target/release/leyen`.

## Usage

### Adding a Game
1. Click the "+" button in the header bar
2. Fill in the game details:
   - **Title**: Display name for your game
   - **Path**: Path to the .exe file (use Browse button)
   - **Prefix Path**: Custom Wine prefix (optional, uses global default if empty)
   - **Proton**: Select Proton version from detected versions
   - **Launch Arguments**: Additional command-line arguments
   - **Force MangoHud**: Enable MangoHud performance overlay
   - **Force GameMode**: Wrap launch with GameMode
3. Click "Add" to save

### Editing a Game
1. Click the edit button (pencil icon) on any game in the list
2. Modify any settings
3. Use "Open Winetricks" button to install dependencies for that game's prefix
4. Click "Save" to apply changes

### Deleting a Game
1. Click the delete button (trash icon) on any game
2. Confirm deletion in the dialog

### Global Settings
1. Click the settings button (gear icon) in the header bar
2. Configure:
   - Default prefix path for new games
   - Default Proton version
   - Global MangoHud toggle
   - Global GameMode toggle
3. Settings are automatically saved when closing the dialog

### Launching Games
- Click the play button (green circular button) on any game
- Toast notifications will inform you of launch success/failure
- Run `leyen list` to inspect root games, grouped games, and currently running games from the terminal
- Run `leyen run <leyen-id>` to launch a managed game from the terminal using its saved `Leyen ID`
- Run `leyen logs` to open the log window directly from the terminal
- Run `leyen kill <leyen-id>` to stop a managed game from the terminal

## Configuration Files

Configuration files are stored in `~/.config/leyen/`:

- `games.toml`: Game library database
- `settings.toml`: Global settings and preferences

Proton versions are detected from:
- `~/.local/share/leyen/proton/`: Local Proton installations (checked first)
- `~/.steam/steam/compatibilitytools.d/`: GE-Proton and other compatibility tools
- `~/.steam/steam/steamapps/common/`: Steam's official Proton versions

## How It Works

### Game Launching
Games are launched using `umu-run` with the following environment variables:
- `WINEPREFIX`: Set to the game's prefix path
- `PROTONPATH`: Set to the selected Proton version path
- `MANGOHUD`: Set to "1" if MangoHud is enabled

If GameMode is enabled, the launch command is wrapped with `gamemoderun`.

### Proton Detection
The application automatically scans for Proton versions in:
- `~/.local/share/leyen/proton/` (local Proton installations - checked first)
- `~/.steam/steam/compatibilitytools.d/` (for GE-Proton and other compatibility tools)
- `~/.steam/steam/steamapps/common/` (for Steam's official Proton versions)

The application will automatically create the `~/.local/share/leyen/proton/` directory on first run. You can place custom Proton versions there for use with Leyen.

### Winetricks Integration
When editing a game, you can launch winetricks with the `WINEPREFIX` environment variable set to that game's prefix, allowing you to install dependencies (DirectX, Visual C++ redistributables, .NET Framework, etc.) specific to that game.
If winetricks is not available on your system, Leyen will automatically download the latest script from the upstream repository into `~/.local/share/leyen/umu-launcher/winetricks` and use that copy.

## Troubleshooting

### Game Won't Launch
- Verify that `umu-run` is installed and in your PATH
- Check that the executable path is correct
- Ensure the selected Proton version is installed
- Check that the prefix path exists and has proper permissions

### Winetricks Doesn't Open
- Leyen will attempt to download winetricks automatically; ensure `curl` can reach `https://raw.githubusercontent.com/Winetricks/winetricks/master/src/winetricks`
- Alternatively, install winetricks via your package manager so it is available in your PATH

### Missing Proton Versions
- Install Proton-GE from https://github.com/GloriousEggroll/proton-ge-custom
- Extract to `~/.local/share/leyen/proton/` (recommended) or `~/.steam/steam/compatibilitytools.d/`
- Restart the application to rescan

## Development

### Project Structure
- `src/main.rs`: Main application code
  - Data structures (Game, GlobalSettings)
  - File I/O (load/save games and settings)
  - UI building (main window, dialogs)
  - Game launching logic
  - Proton detection
  - Winetricks integration

### Adding Features
The codebase is organized into sections:
- `DATA STRUCTURES`: Game and settings data models
- `FILE IO`: Configuration persistence
- `MAIN UI`: Main window construction
- `DYNAMIC UI GENERATOR`: Game list population
- `CORE LAUNCH LOGIC`: Game launching
- `DIALOG FUNCTIONS`: Add/edit game, settings, delete confirmation

## Contributing

Contributions are welcome! Please feel free to submit issues or pull requests.

## License

See LICENSE file for details.

## Credits

Inspired by:
- [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher)
- [Faugus Launcher](https://github.com/Faugus/faugus-launcher)
- [Lutris](https://lutris.net/)
- [Bottles](https://usebottles.com/)
