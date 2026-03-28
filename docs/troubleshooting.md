# Troubleshooting

## Game not found on CLI
Use the correct stable ID. List all IDs:
```bash
leyen list
```

## Game fails to launch
Check logs for Wine or Proton errors:
```bash
leyen logs
```

## Missing Proton versions
Verify Proton builds exist in one of the searched directories:
1. `~/.local/share/leyen/proton/`
2. `~/.steam/steam/compatibilitytools.d/`
3. `~/.steam/steam/steamapps/common/`

## umu-launcher download fails
`curl` and `tar` must be installed. If the automatic download fails, install `umu-launcher` manually.
