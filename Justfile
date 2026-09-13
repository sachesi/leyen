# System-wide build, install, uninstall and test tasks.
#
# Installs under /usr by default; override with PREFIX, e.g.:
#   PREFIX=/usr/local just install
#
# `install` reuses existing target/release binaries when present (e.g. built
# in a container), so the install host needs no build dependencies.

prefix := env("PREFIX", "/usr")
bindir := prefix / "bin"
sharedir := prefix / "share"
app_id := "io.github.sachesi.leyen"
sudo := if `id -u` == "0" { "" } else { "sudo" }

default:
    @just --list

# Build all workspace binaries (debug)
build:
    cargo build --workspace

# Build optimized binaries
release:
    cargo build --release --workspace

# Run the test suite
test:
    cargo test --workspace

# Lint, warnings are errors
clippy:
    cargo clippy --workspace -- -D warnings

# test + clippy
check: test clippy

# Smoke-test the daemon's D-Bus surface on an ephemeral session bus
smoke: build
    #!/usr/bin/env bash
    set -euo pipefail
    export DBUS_SESSION_BUS_ADDRESS=$(dbus-daemon --session --fork --print-address)
    ./target/debug/leyend &
    pid=$!
    trap 'kill "$pid" 2>/dev/null || true' EXIT
    sleep 1.5
    mgr={{ app_id }}.Manager
    dest={{ app_id }}.Daemon
    path=/io/github/sachesi/leyen
    gdbus call --session --dest "$dest" --object-path "$path" --method "$mgr.GetRunningGames"
    gdbus call --session --dest "$dest" --object-path "$path" --method "$mgr.GetRuntimeStatus"
    gdbus call --session --dest "$dest" --object-path "$path" --method "$mgr.GetLogs" 0 >/dev/null
    echo "smoke OK"

# Install binaries, desktop entry, icons, D-Bus activation, completions, locales
install:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -x target/release/leyen ] || [ ! -x target/release/leyend ] || [ ! -x target/release/leyen-gtk ]; then
        if command -v cargo >/dev/null 2>&1; then
            cargo build --release --workspace
        else
            echo "error: target/release binaries missing and cargo is not available." >&2
            echo "Run 'just release' where the toolchain exists, then re-run 'just install'." >&2
            exit 1
        fi
    fi
    {{ sudo }} install -Dm755 target/release/leyen "{{ bindir }}/leyen"
    {{ sudo }} install -Dm755 target/release/leyend "{{ bindir }}/leyend"
    {{ sudo }} install -Dm755 target/release/leyen-gtk "{{ bindir }}/leyen-gtk"
    {{ sudo }} install -Dm644 packaging/usr/share/applications/{{ app_id }}.desktop -t "{{ sharedir }}/applications"
    {{ sudo }} install -Dm644 packaging/usr/share/icons/hicolor/scalable/apps/{{ app_id }}.svg -t "{{ sharedir }}/icons/hicolor/scalable/apps"
    {{ sudo }} install -Dm644 packaging/usr/share/icons/hicolor/symbolic/apps/{{ app_id }}-symbolic.svg -t "{{ sharedir }}/icons/hicolor/symbolic/apps"
    {{ sudo }} install -Dm644 packaging/usr/share/dbus-1/services/{{ app_id }}.Daemon.service -t "{{ sharedir }}/dbus-1/services"
    {{ sudo }} sed -i "s|@LEYEND@|{{ bindir }}/leyend|" "{{ sharedir }}/dbus-1/services/{{ app_id }}.Daemon.service"
    {{ sudo }} install -Dm644 packaging/usr/share/bash-completion/completions/leyen.bash "{{ sharedir }}/bash-completion/completions/leyen"
    {{ sudo }} install -Dm644 packaging/usr/share/fish/vendor_completions.d/leyen.fish -t "{{ sharedir }}/fish/vendor_completions.d"
    {{ sudo }} install -Dm644 packaging/usr/share/zsh/site-functions/_leyen -t "{{ sharedir }}/zsh/site-functions"
    for mo in packaging/usr/share/locale/*/LC_MESSAGES/leyen.mo; do
        lang=$(basename "$(dirname "$(dirname "$mo")")")
        {{ sudo }} install -Dm644 "$mo" "{{ sharedir }}/locale/$lang/LC_MESSAGES/leyen.mo"
    done
    # Files of installs made under the old com.github application id.
    {{ sudo }} rm -f "{{ sharedir }}/applications/com.github.sachesi.leyen.desktop" \
        "{{ sharedir }}/icons/hicolor/scalable/apps/com.github.sachesi.leyen.svg" \
        "{{ sharedir }}/icons/hicolor/symbolic/apps/com.github.sachesi.leyen-symbolic.svg" \
        "{{ sharedir }}/dbus-1/services/com.github.sachesi.leyen.Daemon.service"
    # Stop a running daemon so the next D-Bus call activates the new binary,
    # unless it is tracking games: killing it then loses their final playtime.
    # A daemon of an install from before the id change answers on the old name.
    if command -v busctl >/dev/null 2>&1; then
        busy=0
        for id in io.github.sachesi.leyen com.github.sachesi.leyen; do
            path="/${id//.//}"
            if busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner s "$id.Daemon" 2>/dev/null | grep -q "true"; then
                games_output=$(busctl --user call "$id.Daemon" "$path" "$id.Manager" GetRunningGames 2>/dev/null || echo "a(ssttt) 0")
                # Exact empty-array reply only: a suffix match would false-positive on
                # a running game whose last field (pid count) happens to be 0.
                [ "$games_output" = "a(ssttt) 0" ] || busy=1
            fi
        done
        if [ "$busy" = 1 ]; then
            echo "warning: active game sessions detected. Daemon will not be killed to avoid losing final playtime updates." >&2
            echo "Re-run 'just install' after games are closed, or kill/restart manually." >&2
        else
            pkill -x leyend 2>/dev/null || true
        fi
    else
        # busctl not available; fall back to unconditional kill.
        pkill -x leyend 2>/dev/null || true
    fi
    {{ sudo }} update-desktop-database "{{ sharedir }}/applications" 2>/dev/null || true
    {{ sudo }} gtk-update-icon-cache -qtf "{{ sharedir }}/icons/hicolor" 2>/dev/null || true
    echo "Installed to {{ prefix }}."

# Remove everything `just install` placed
uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    pkill -x leyend 2>/dev/null || true
    {{ sudo }} rm -f "{{ bindir }}/leyen" "{{ bindir }}/leyend" "{{ bindir }}/leyen-gtk"
    {{ sudo }} rm -f "{{ sharedir }}/applications/{{ app_id }}.desktop"
    {{ sudo }} rm -f "{{ sharedir }}/icons/hicolor/scalable/apps/{{ app_id }}.svg"
    # Legacy location used by older installs; remove so stale icon-cache
    # entries cannot point at a missing file.
    {{ sudo }} rm -f "{{ sharedir }}/icons/hicolor/256x256/apps/{{ app_id }}.svg"
    {{ sudo }} rm -f "{{ sharedir }}/icons/hicolor/symbolic/apps/{{ app_id }}-symbolic.svg"
    {{ sudo }} rm -f "{{ sharedir }}/dbus-1/services/{{ app_id }}.Daemon.service"
    {{ sudo }} rm -f "{{ sharedir }}/bash-completion/completions/leyen"
    {{ sudo }} rm -f "{{ sharedir }}/fish/vendor_completions.d/leyen.fish"
    {{ sudo }} rm -f "{{ sharedir }}/zsh/site-functions/_leyen"
    {{ sudo }} rm -f "{{ sharedir }}"/locale/*/LC_MESSAGES/leyen.mo
    {{ sudo }} update-desktop-database "{{ sharedir }}/applications" 2>/dev/null || true
    {{ sudo }} gtk-update-icon-cache -qtf "{{ sharedir }}/icons/hicolor" 2>/dev/null || true
    echo "Uninstalled from {{ prefix }}. Per-user config/data in ~/.config/leyen and ~/.local/share/leyen are kept."
