# Troubleshooting

## `leyen run <id>` says no game was found

Run:

```bash
leyen list
```

Use the exact Leyen ID from the first column, for example `ly-1234`.

## Game does not launch

Check:

- the executable path exists
- the selected Proton path exists
- the prefix directory is writable
- `umu-run` works, or Leyen's local umu-launcher download completed
- launch arguments are valid

Run from a terminal and open logs:

```bash
leyen logs
```

## No Proton versions are shown

Put a Proton build in one of these directories:

```text
~/.local/share/leyen/proton/
~/.steam/steam/compatibilitytools.d/
~/.steam/steam/steamapps/common/
```

Restart Leyen after adding a new Proton build.

## umu-launcher download fails

Leyen uses `curl` and `tar` to download and extract umu-launcher when `umu-run` is not available.

Check that these tools are installed and that GitHub is reachable:

```bash
command -v curl
command -v tar
```

You can also install `umu-run` system-wide and Leyen will prefer it.

## Prefix is already in use

Leyen tracks running games and tries to avoid launching multiple isolated sessions into the same prefix. When needed, it may use `UMU_CONTAINER_NSENTER=1` as a shared-container fallback.

If a game crashed and Leyen still thinks it is running, close stale Wine/Proton processes and check:

```bash
leyen list
```

## Desktop entry does not appear

Desktop entries are written under:

```text
~/.local/share/applications/
```

Try refreshing the desktop database or logging out and back in. The entry command should look like:

```text
Exec=leyen run ly-1234
```

## Logs are too quiet

Open global settings and enable warning or operation logging. Then start Leyen from a terminal or open:

```bash
leyen logs
```
