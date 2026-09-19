%define _debugsource_template %{nil}
%define debug_package %{nil}

%global app_id io.github.sachesi.leyen

Name:           leyen
# The release workflow sets Version to the tag it builds; OBS counts the Release.
Version:        0.9.1
Release:        0
Summary:        Run Windows games with Proton

License:        GPL-3.0-or-later
URL:            https://github.com/sachesi/leyen
# Named as the Debian source package names them, which OBS builds from the same files.
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}_%{version}.orig.tar.gz
# The crates the build needs, from the release, so that it runs without a network.
Source1:        %{url}/releases/download/v%{version}/%{name}-%{version}-vendor.tar.xz#/%{name}_%{version}.orig-vendor.tar.xz

BuildRequires:  cargo
BuildRequires:  rust >= 1.92
BuildRequires:  gcc
BuildRequires:  blueprint-compiler
BuildRequires:  desktop-file-utils
BuildRequires:  gettext-tools
BuildRequires:  AppStream
BuildRequires:  pkgconfig(gtk4) >= 4.22
BuildRequires:  pkgconfig(libadwaita-1) >= 1.9
# glib-compile-resources builds the interface into the binary.
BuildRequires:  pkgconfig(glib-2.0)

Requires:       libgtk-4-1 >= 4.22
Requires:       libadwaita-1-0 >= 1.9
Requires:       hicolor-icon-theme
# Every game runs in a transient scope of the systemd user manager, which is how
# the daemon tracks and stops it; without one a launch is refused.
Requires:       systemd
# umu-launcher and winetricks are fetched with curl and unpacked with tar the first
# time they are needed, unless umu-run and winetricks are already in PATH.
Requires:       curl
Requires:       tar
# Their switches appear once the tools are installed.
Recommends:     mangohud
Recommends:     gamemode

%description
Leyen keeps a library of Windows games and runs them with Proton through
umu-launcher, each in its own Wine prefix or in one shared with its group. A
daemon on the session bus launches, tracks and stops the games, so the window,
the applications menu and the command line see the same running games.

Groups with a prefix and a Proton their games inherit, playtime and the last
session of every game, per-game launch arguments, MangoHud, GameMode, Wayland,
WoW64, NTSync and HDR switches, winetricks components managed per prefix, the
Wine configuration and the registry editor for any prefix, a live log, menu
entries that start a game from the desktop, and leyen list, run, kill and logs
for the terminal.

%prep
%autosetup -n %{name}-%{version} -b 1

%build
export CARGO_HOME="$PWD/.cargo-home"
export RUSTFLAGS="%{?build_rustflags}"
%if 0%{?_cargo_target_dir:1}
export CARGO_TARGET_DIR="%{_cargo_target_dir}"
%endif
cargo build --release --workspace --offline --locked

%install
%if 0%{?_cargo_target_dir:1}
target="%{_cargo_target_dir}/release"
%else
target="target/release"
%endif
install -Dpm 0755 "$target/leyen" %{buildroot}%{_bindir}/leyen
install -Dpm 0755 "$target/leyend" %{buildroot}%{_bindir}/leyend
install -Dpm 0755 "$target/leyen-gtk" %{buildroot}%{_bindir}/leyen-gtk

install -d %{buildroot}%{_datadir}/applications %{buildroot}%{_datadir}/metainfo
msgfmt --desktop --template=data/%{app_id}.desktop -d po \
  -o %{buildroot}%{_datadir}/applications/%{app_id}.desktop
msgfmt --xml --template=data/%{app_id}.metainfo.xml -d po \
  -o %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
install -Dpm 0644 data/icons/hicolor/scalable/apps/%{app_id}.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
install -Dpm 0644 data/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg \
  %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

install -d %{buildroot}%{_datadir}/dbus-1/services
sed 's|@bindir@|%{_bindir}|' data/%{app_id}.Daemon.service.in \
  > %{buildroot}%{_datadir}/dbus-1/services/%{app_id}.Daemon.service

install -Dpm 0644 data/completions/leyen.bash \
  %{buildroot}%{_datadir}/bash-completion/completions/leyen
install -Dpm 0644 data/completions/leyen.fish \
  %{buildroot}%{_datadir}/fish/vendor_completions.d/leyen.fish
install -Dpm 0644 data/completions/_leyen \
  %{buildroot}%{_datadir}/zsh/site-functions/_leyen

for lang in $(cat po/LINGUAS); do
  install -d %{buildroot}%{_datadir}/locale/$lang/LC_MESSAGES
  msgfmt -o %{buildroot}%{_datadir}/locale/$lang/LC_MESSAGES/%{name}.mo po/$lang.po
done
%find_lang %{name}

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_datadir}/metainfo/%{app_id}.metainfo.xml
grep -qx 'Exec=%{_bindir}/leyend' \
  %{buildroot}%{_datadir}/dbus-1/services/%{app_id}.Daemon.service

%files -f %{name}.lang
%license LICENSE
%doc README.md docs
%{_bindir}/leyen
%{_bindir}/leyend
%{_bindir}/leyen-gtk
%{_datadir}/applications/%{app_id}.desktop
%{_datadir}/metainfo/%{app_id}.metainfo.xml
%{_datadir}/dbus-1/services/%{app_id}.Daemon.service
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg
%{_datadir}/bash-completion/completions/leyen
%dir %{_datadir}/fish
%dir %{_datadir}/fish/vendor_completions.d
%{_datadir}/fish/vendor_completions.d/leyen.fish
%dir %{_datadir}/zsh
%dir %{_datadir}/zsh/site-functions
%{_datadir}/zsh/site-functions/_leyen

%changelog
