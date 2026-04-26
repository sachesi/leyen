%define _debugsource_template %{nil}
%define debug_package %{nil}
%global app_id com.github.sachesi.leyen

Name:           leyen
Version:        0.2.0
Release:        1%{?dist}
Summary:        umu-launcher GUI for managing Wine/Proton games

License:        GPL-3.0-or-later
URL:            https://github.com/sachesi/leyen
Source0:        %{url}/archive/refs/tags/v%{version}.tar.gz#/%{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust >= 1.85
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)

%description
Leyen is a modern GTK4/Libadwaita frontend for managing Wine/Proton games
using umu-launcher. It supports per-game Proton selection, custom Wine
prefixes, MangoHud, GameMode, NTSync, WoW64, and a built-in dependency
installer for Visual C++, .NET, DirectX, and more.

%prep
%autosetup

%build
cargo build --release

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 assets/usr/share/applications/%{app_id}.desktop \
    %{buildroot}%{_datadir}/applications/%{app_id}.desktop
install -Dm644 assets/usr/share/icons/hicolor/256x256/apps/%{app_id}.svg \
    %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/%{app_id}.svg

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/%{app_id}.desktop
%{_datadir}/icons/hicolor/256x256/apps/%{app_id}.svg

%changelog
* Sun Apr 26 2026 sachesi <sachesi.bb.passp@proton.me> - 0.2.0-1
- Rework dependency management around tracked prefix state and profiles
- Add group, game, and global dependency tools with managed-prefix ownership
- Add menu entry naming updates and pick-and-run prefix tools

* Sat Apr 25 2026 sachesi <sachesi.bb.passp@proton.me> - 0.1.5-1
- Add desktop integration for per-game menu launchers via `leyen run`
- Store managed game and group icons under hicolor app icon paths
- Add menu toggle actions to game settings tools

* Sat Apr 25 2026 sachesi <sachesi.bb.passp@proton.me> - 0.1.4-1
- Add managed game and group icons with desktop asset packaging
- Refine library sorting, running-game promotion, and card icon rendering
- Make custom prefix overrides opt-in in add and edit dialogs

* Sat Apr 25 2026 sachesi <sachesi.bb.passp@proton.me> - 0.1.3-1
- Recheck runtime tracking, CLI flow, and packaging helper docs
- Tighten download transport defaults for runtime fetches

* Sat Apr 25 2026 sachesi <sachesi.bb.passp@proton.me> - 0.1.2-1
- Bump release version to 0.1.2

* Fri Apr 24 2026 sachesi <sachesi.bb.passp@proton.me> - 0.1.0-1
- COPR/local workflow
- Use GitHub tag archive Source0 with %autosetup

* Wed Apr 01 2026 sachesi <sachesi.bb.passp@proton.me> - 0.1.0-1
- Initial Fedora spec file
