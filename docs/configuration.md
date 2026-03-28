# Configuration

## File Locations

- **Config:** `~/.config/leyen/`
- **Data:** `~/.local/share/leyen/`

### Key Files
- `games.toml` — Game library, group assignments, launch settings.
- `settings.toml` — Global preferences, Proton search paths.
- `running.toml` — Active process tracking (internal).
- `logs.jsonl` — App and game output logs.

## Proton Discovery

Leyen scans these directories for Proton builds:
1. `~/.local/share/leyen/proton/`
2. `~/.steam/steam/compatibilitytools.d/`
3. `~/.steam/steam/steamapps/common/`

## Wine Prefixes

Default prefix: `~/.local/share/leyen/prefixes/`.

Priority for prefix selection:
1. Per-game setting
2. Group default
3. Global default

## Launch Environment

Leyen uses `umu-run` to set environment variables (`WINEPREFIX`, `PROTONPATH`).

If a prefix is already active when launching another game, Leyen passes `UMU_CONTAINER_NSENTER=1` to join the existing container.

## Debugging

Enable verbose logging with `RUST_LOG`:
```bash
RUST_LOG=debug leyen
```
Use `leyen logs` to view output.
