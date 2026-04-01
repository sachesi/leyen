%define _debugsource_template %{nil}
%define debug_package %{nil}

Name:           leyen
Version:        0.1.0
Release:        1%{?dist}
Summary:        umu-launcher GUI for managing Wine/Proton games

License:        GPL-3.0-or-later
URL:            https://github.com/sachesi/leyen

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
# Nothing to do — built in-place with --build-in-place

%build
cargo build --release

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 com.github.leyen.desktop \
    %{buildroot}%{_datadir}/applications/com.github.leyen.desktop

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/com.github.leyen.desktop

%changelog
* Tue Apr 01 2026 leyen packager
- Initial Fedora spec file
