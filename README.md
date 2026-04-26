# Leyen

Leyen is a small GTK4/libadwaita launcher for Windows games on Linux. It manages a local game library, Proton/umu-launcher settings, Wine prefixes, launch flags, logs, playtime, and simple CLI launch/stop actions.

## Features

- Modern GNOME-style GTK4/libadwaita interface
- Per-game executable, Wine prefix, Proton, launch arguments, icon, and compatibility toggles
- Groups with optional default prefix and Proton settings
- `umu-run` based launching with automatic local umu-launcher fallback
- Local Proton-GE helper directory under `~/.local/share/leyen/proton/`
- MangoHud, GameMode, Wayland, WOW64, and NTSYNC toggles
- Running-game tracking, playtime tracking, logs window, and desktop entry generation
- CLI commands for listing, launching, opening logs, and stopping games
- Bash, Zsh, and Fish completion files in `completions/`

## Build

Install GTK4/libadwaita development packages, Rust, Cargo, `pkg-config`, `curl`, and `tar`, then build:

```bash
git clone https://github.com/sachesi/leyen.git
cd leyen
cargo build --release
```

Run from the build tree:

```bash
cargo run --release
```

Or install the binary locally:

```bash
install -Dm755 target/release/leyen ~/.local/bin/leyen
```

## CLI

```text
leyen
leyen list
leyen run <leyen-id>
leyen logs
leyen kill <leyen-id>
```

Use `leyen list` to see generated IDs such as `ly-1234`, then launch or stop a game with that ID.

## Documentation

- [Installation](docs/installation.md)
- [Usage](docs/usage.md)
- [Configuration](docs/configuration.md)
- [Shell completions](docs/shell-completions.md)
- [Troubleshooting](docs/troubleshooting.md)

## Config and data

Leyen stores user configuration in `~/.config/leyen/` and user data in `~/.local/share/leyen/`.

## License

See [LICENSE](LICENSE).
