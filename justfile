# Leyen build, check and install tasks.
#
# `build` needs cargo and blueprint-compiler. `install` copies what is already in target/release, building it
# only when it is missing, so a release built elsewhere installs on a machine without
# the toolchain. It asks for sudo itself when the prefix
# is not writable: the daemon it restarts belongs to your session, not root's.
#
#   just build
#   just install                   (prefix /usr)
#   just prefix=$HOME/.local install

set shell := ["bash", "-euo", "pipefail", "-c"]

app_id := "io.github.sachesi.leyen"
prefix := env("PREFIX", "/usr")
destdir := env("DESTDIR", "")
bindir := destdir + prefix + "/bin"
datadir := destdir + prefix + "/share"
release := "target/release"
pot_dir := "target/pot"
check_dir := "target/check"
stage_dir := "target/stage"
version := `sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1`

default:
    @just --list

# Release build of the three binaries.
build:
    cargo build --release --workspace

# Debug build.
build-debug:
    cargo build --workspace

# Run the window from the debug build.
run *args: build-debug
    target/debug/leyen-gtk {{args}}

# Lints: rustfmt, clippy, blueprint, desktop entry, metainfo and catalogues.
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    mkdir -p {{check_dir}}
    blueprint-compiler batch-compile {{check_dir}} data/ui data/ui/*.blp >/dev/null
    desktop-file-validate data/{{app_id}}.desktop
    appstreamcli validate --no-net data/{{app_id}}.metainfo.xml
    for lang in $(cat po/LINGUAS); do msgfmt -c -o /dev/null po/$lang.po; done

# Unit tests.
test:
    cargo test --workspace

# The daemon's D-Bus surface on a private session bus.
smoke: build-debug
    #!/usr/bin/env bash
    set -euo pipefail
    # A bus of its own, torn down with everything on it when the script ends.
    [ -n "${LEYEN_SMOKE_BUS:-}" ] || LEYEN_SMOKE_BUS=1 exec dbus-run-session -- bash "$0"
    target/debug/leyend &
    trap 'kill $! 2>/dev/null || true' EXIT
    sleep 1.5
    call() { gdbus call --session --dest {{app_id}}.Daemon --object-path /io/github/sachesi/leyen --method "{{app_id}}.Manager.$1" "${@:2}"; }
    call GetRunningGames
    call GetRuntimeStatus
    call GetLogs 0 >/dev/null
    echo "smoke OK"

# Regenerate po/leyen.pot from the sources, Blueprint files, desktop entry and metainfo.
pot:
    rm -rf {{pot_dir}} && mkdir -p {{pot_dir}}/ui
    blueprint-compiler batch-compile {{pot_dir}}/ui data/ui data/ui/*.blp >/dev/null
    # xgettext has no Rust mode; the C lexer copes once lifetimes ('a, 'static) are stripped.
    find crates -name '*.rs' -exec cp --parents {} {{pot_dir}} \;
    # A quote right after a letter is an apostrophe in a string, not a lifetime.
    find {{pot_dir}}/crates -name '*.rs' -exec sed -i -E "s/(^|[^A-Za-z0-9_])'([A-Za-z_][A-Za-z0-9_]*)([^'A-Za-z0-9_]|$)/\1\2\3/g" {} +
    xgettext --from-code=UTF-8 --package-name=leyen --package-version={{version}} \
        --msgid-bugs-address=https://github.com/sachesi/leyen/issues \
        --language=C --keyword= --keyword=gettext --keyword=ngettext:1,2 \
        --flag=gettext:1:no-c-format --flag=ngettext:1:no-c-format --flag=ngettext:2:no-c-format \
        --add-comments=Translators --sort-by-file --directory={{pot_dir}} -o po/leyen.pot $(cd {{pot_dir}} && find crates -name '*.rs' | sort)
    xgettext -j --from-code=UTF-8 --package-name=leyen --package-version={{version}} --msgid-bugs-address=https://github.com/sachesi/leyen/issues --add-comments=Translators --sort-by-file --directory={{pot_dir}} -o po/leyen.pot $(cd {{pot_dir}} && ls ui/*.ui)
    xgettext -j --from-code=UTF-8 --package-name=leyen --package-version={{version}} --msgid-bugs-address=https://github.com/sachesi/leyen/issues --language=Desktop --sort-by-file -o po/leyen.pot data/{{app_id}}.desktop
    xgettext -j --from-code=UTF-8 --package-name=leyen --package-version={{version}} --msgid-bugs-address=https://github.com/sachesi/leyen/issues --sort-by-file -o po/leyen.pot data/{{app_id}}.metainfo.xml

# Merge the current template into every po/<lang>.po, dropping messages no longer used.
po: pot
    for lang in $(cat po/LINGUAS); do msgmerge --update --backup=none --quiet po/$lang.po po/leyen.pot; done
    for lang in $(cat po/LINGUAS); do msgattrib --no-obsolete -o po/$lang.po po/$lang.po; done
    for lang in $(cat po/LINGUAS); do msgfmt --statistics -o /dev/null po/$lang.po; done

# Install the release build, building it first when it is missing.
install:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -x {{release}}/leyen ] || [ ! -x {{release}}/leyend ] || [ ! -x {{release}}/leyen-gtk ]; then
        command -v cargo >/dev/null || { echo "error: {{release}} is incomplete and cargo is missing; run 'just build' where the toolchain is" >&2; exit 1; }
        cargo build --release --workspace
    fi
    # sudo only for a prefix this user cannot write to.
    dir="{{destdir}}{{prefix}}"
    while [ ! -e "$dir" ]; do dir=$(dirname "$dir"); done
    sudo=""; [ -w "$dir" ] || sudo=sudo
    # What gets translated or filled in on the way, made here as this user.
    stage={{stage_dir}}
    rm -rf "$stage" && mkdir -p "$stage/locale"
    msgfmt --desktop --template=data/{{app_id}}.desktop -d po -o "$stage/{{app_id}}.desktop"
    msgfmt --xml --template=data/{{app_id}}.metainfo.xml -d po -o "$stage/{{app_id}}.metainfo.xml"
    sed 's|@bindir@|{{prefix}}/bin|' data/{{app_id}}.Daemon.service.in > "$stage/{{app_id}}.Daemon.service"
    for lang in $(cat po/LINGUAS); do msgfmt -o "$stage/locale/$lang.mo" po/$lang.po; done
    $sudo install -Dm755 {{release}}/leyen {{bindir}}/leyen
    $sudo install -Dm755 {{release}}/leyend {{bindir}}/leyend
    $sudo install -Dm755 {{release}}/leyen-gtk {{bindir}}/leyen-gtk
    $sudo install -Dm644 "$stage/{{app_id}}.desktop" {{datadir}}/applications/{{app_id}}.desktop
    $sudo install -Dm644 "$stage/{{app_id}}.metainfo.xml" {{datadir}}/metainfo/{{app_id}}.metainfo.xml
    $sudo install -Dm644 "$stage/{{app_id}}.Daemon.service" {{datadir}}/dbus-1/services/{{app_id}}.Daemon.service
    $sudo install -Dm644 data/icons/hicolor/scalable/apps/{{app_id}}.svg {{datadir}}/icons/hicolor/scalable/apps/{{app_id}}.svg
    $sudo install -Dm644 data/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg {{datadir}}/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg
    $sudo install -Dm644 data/completions/leyen.bash {{datadir}}/bash-completion/completions/leyen
    $sudo install -Dm644 data/completions/leyen.fish {{datadir}}/fish/vendor_completions.d/leyen.fish
    $sudo install -Dm644 data/completions/_leyen {{datadir}}/zsh/site-functions/_leyen
    for lang in $(cat po/LINGUAS); do $sudo install -Dm644 "$stage/locale/$lang.mo" {{datadir}}/locale/$lang/LC_MESSAGES/leyen.mo; done
    # Files of installs made under the old com.github application id.
    $sudo rm -f {{datadir}}/applications/com.github.sachesi.leyen.desktop \
        {{datadir}}/icons/hicolor/scalable/apps/com.github.sachesi.leyen.svg \
        {{datadir}}/icons/hicolor/symbolic/apps/com.github.sachesi.leyen-symbolic.svg \
        {{datadir}}/dbus-1/services/com.github.sachesi.leyen.Daemon.service
    if [ -z "{{destdir}}" ]; then
        $sudo update-desktop-database -q {{datadir}}/applications || true
        $sudo gtk4-update-icon-cache -qtf {{datadir}}/icons/hicolor || $sudo gtk-update-icon-cache -qtf {{datadir}}/icons/hicolor || true
        just _stop-idle-daemon
    fi
    echo "installed to {{prefix}}"

# Remove everything `install` placed. Your library and settings stay where they are.
uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="{{destdir}}{{prefix}}"
    while [ ! -e "$dir" ]; do dir=$(dirname "$dir"); done
    sudo=""; [ -w "$dir" ] || sudo=sudo
    $sudo rm -f {{bindir}}/leyen {{bindir}}/leyend {{bindir}}/leyen-gtk
    $sudo rm -f {{datadir}}/applications/{{app_id}}.desktop {{datadir}}/metainfo/{{app_id}}.metainfo.xml
    $sudo rm -f {{datadir}}/dbus-1/services/{{app_id}}.Daemon.service
    $sudo rm -f {{datadir}}/icons/hicolor/scalable/apps/{{app_id}}.svg {{datadir}}/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg
    $sudo rm -f {{datadir}}/bash-completion/completions/leyen {{datadir}}/fish/vendor_completions.d/leyen.fish {{datadir}}/zsh/site-functions/_leyen
    for lang in $(cat po/LINGUAS); do $sudo rm -f {{datadir}}/locale/$lang/LC_MESSAGES/leyen.mo; done
    if [ -z "{{destdir}}" ]; then
        $sudo update-desktop-database -q {{datadir}}/applications || true
        $sudo gtk4-update-icon-cache -qtf {{datadir}}/icons/hicolor || $sudo gtk-update-icon-cache -qtf {{datadir}}/icons/hicolor || true
        just _stop-idle-daemon
    fi
    echo "removed from {{prefix}}; ~/.config/leyen and ~/.local/share/leyen are kept"

# Stop the running daemon, so the next call starts the installed binary, unless it is
# tracking games or cannot say: stopping it then loses their final playtime. A daemon of
# an install from before the id change answers on the old name.
_stop-idle-daemon:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v busctl >/dev/null; then pkill -x leyend || true; exit 0; fi
    for id in {{app_id}} com.github.sachesi.leyen; do
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus NameHasOwner s "$id.Daemon" 2>/dev/null | grep -q true || continue
        if ! games=$(busctl --user call "$id.Daemon" "/${id//.//}" "$id.Manager" GetRunningGames 2>/dev/null); then
            echo "warning: the daemon did not say what is running; it is left running until it exits by itself" >&2
            exit 0
        fi
        # The exact empty reply: a suffix match would take a game whose pid count is 0
        # for no game at all.
        if [ "$games" != "a(ssttt) 0" ]; then
            echo "warning: games are running; the daemon keeps running until they end" >&2
            exit 0
        fi
    done
    pkill -x leyend || true
