# Installation

## Dependencies

### Runtime
- **GTK4 & libadwaita**
- **[umu-launcher](https://github.com/Open-Wine-Components/umu-launcher)** — provides `umu-run`. Leyen auto-downloads if missing. On NixOS, use the system or flake package.
- **Proton** (Steam Proton or GE-Proton)

### Build
- Rust/Cargo (Edition 2024)
- pkg-config
- GTK4 and libadwaita development headers

## Distribution Packages

### Fedora
```bash
sudo dnf install rust cargo pkgconf-pkg-config gtk4-devel libadwaita-devel curl tar
```

### Arch Linux
```bash
sudo pacman -S rust cargo pkgconf gtk4 libadwaita curl tar
```

### Debian / Ubuntu
```bash
sudo apt install rustc cargo pkg-config libgtk-4-dev libadwaita-1-dev curl tar
```

## Nix (Flakes)

Add to flake inputs:
```nix
inputs.leyen.url = "github:sachesi/leyen";
```

Add to system packages:
```nix
environment.systemPackages = [ inputs.leyen.packages.${system}.default ];
```

## Build from Source

```bash
git clone https://github.com/sachesi/leyen.git
cd leyen
cargo build --release
install -Dm755 target/release/leyen ~/.local/bin/leyen
```

## First Run

Leyen creates these directories on first launch:
- `~/.config/leyen/`
- `~/.local/share/leyen/`

If `umu-run` is not found, Leyen prompts to download it to `~/.local/share/leyen/core/umu-launcher/`.
