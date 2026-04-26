# Installation

Leyen is a Rust GTK4/libadwaita application. It currently builds from source.

## Runtime dependencies

Required:

- GTK4
- libadwaita
- `curl`
- `tar`

Used for game launching:

- `umu-run` from umu-launcher, or Leyen's automatic local umu-launcher download
- A Proton build, for example Proton-GE or Steam Proton

Optional integrations:

- MangoHud
- GameMode
- winetricks or a Windows installer launched through Leyen's prefix tools

## Build dependencies

- Rust/Cargo, edition 2024 capable toolchain
- `pkg-config`
- GTK4 development files
- libadwaita development files

## Fedora

```bash
sudo dnf install rust cargo pkgconf-pkg-config gtk4-devel libadwaita-devel curl tar
```

Optional:

```bash
sudo dnf install mangohud gamemode winetricks
```

Build and install:

```bash
git clone https://github.com/sachesi/leyen.git
cd leyen
cargo build --release
install -Dm755 target/release/leyen ~/.local/bin/leyen
```

Make sure `~/.local/bin` is in `PATH`.

## Arch Linux

```bash
sudo pacman -S rust cargo pkgconf gtk4 libadwaita curl tar
```

Optional:

```bash
sudo pacman -S mangohud gamemode winetricks
```

## Debian/Ubuntu

```bash
sudo apt install rustc cargo pkg-config libgtk-4-dev libadwaita-1-dev curl tar
```

Optional packages depend on your distribution release.

## First run

Start the GUI:

```bash
leyen
```

On first run Leyen creates:

- `~/.config/leyen/`
- `~/.local/share/leyen/proton/`
- `~/.local/share/leyen/prefixes/default/`

If no system `umu-run` is found, Leyen can download a local umu-launcher zipapp into:

```text
~/.local/share/leyen/core/umu-launcher/
```
