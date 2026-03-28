# Leyen

GTK4/libadwaita app for running Windows games on Linux via Proton and `umu-run`.

## Features

- Launch Windows games with `umu-run`.
- Organize games into groups, track playtime.
- Per-game Proton version, Wine prefix, and launch arguments.
- Toggle MangoHud, GameMode, Wayland, WOW64, NTSYNC.
- Create `.desktop` entries — launch games from your DE.
- CLI: list, run, and kill games from a terminal.

## Installation

### Dependencies

- `umu-run` (from [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher))
- GTK4
- libadwaita

### Build from source

```bash
git clone https://github.com/sachesi/leyen.git
cd leyen
cargo build --release
```

### Install binary

```bash
sudo make install
```

## CLI

```
leyen list
leyen run <id>
leyen logs
leyen kill <id>
```

## Documentation

- [Installation](docs/installation.md)
- [Usage](docs/usage.md)
- [Configuration](docs/configuration.md)
- [Shell completions](docs/shell-completions.md)
- [Troubleshooting](docs/troubleshooting.md)

## Config and data

- Config: `~/.config/leyen/`
- Data: `~/.local/share/leyen/`
