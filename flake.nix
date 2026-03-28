{
  description = "Leyen - A GTK4/libadwaita launcher for games using Proton and umu-launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    let
      # Define the overlay so it can be used in other flakes
      overlay = final: prev: {
        leyen = self.packages.${final.system}.default;
      };
    in
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          overlays = [ (import rust-overlay) ];
          pkgs = import nixpkgs {
            inherit system overlays;
          };

          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" ];
          };

          buildInputs = with pkgs; [
            gtk4
            libadwaita
            glib
            cairo
            pango
            gdk-pixbuf
            graphene
            curl
            umu-launcher
            zstd
            winetricks
          ];

        in
        {
          packages.default = (pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          }).buildRustPackage {
            pname = "leyen";
            version = "0.2.8";
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.wrapGAppsHook4
              rustToolchain
            ];

            buildInputs = buildInputs;

            postInstall = ''
              install -Dm644 packaging/usr/share/applications/com.github.sachesi.leyen.desktop -t $out/share/applications
              install -Dm644 packaging/usr/share/icons/hicolor/256x256/apps/com.github.sachesi.leyen.svg -t $out/share/icons/hicolor/256x256/apps
              install -Dm644 packaging/usr/share/bash-completion/completions/leyen.bash -t $out/share/bash-completion/completions
              install -Dm644 packaging/usr/share/fish/vendor_completions.d/leyen.fish -t $out/share/fish/vendor_completions.d
              install -Dm644 packaging/usr/share/zsh/site-functions/_leyen -t $out/share/zsh/site-functions
            '';

            # Use gappsWrapperArgs for a more "pure" Nix integration
            preFixup = ''
              gappsWrapperArgs+=(
                --prefix PATH : ${pkgs.lib.makeBinPath [
                  pkgs.curl
                  pkgs.gnutar
                  pkgs.umu-launcher
                  pkgs.which
                  pkgs.zstd
                  pkgs.winetricks
                ]}
              )
            '';

            meta = with pkgs.lib; {
              description = "A GTK4/libadwaita launcher for games using Proton and umu-launcher";
              homepage = "https://github.com/sachesi/leyen";
              license = licenses.gpl3Plus;
              maintainers = [ "sachesi" ];
              mainProgram = "leyen";
            };
          };

          devShells.default = pkgs.mkShell {
            inherit buildInputs;
            nativeBuildInputs = [
              pkgs.pkg-config
              rustToolchain
            ];

            shellHook = ''
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath buildInputs}:$LD_LIBRARY_PATH"
              export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}:${pkgs.libadwaita}/share/gsettings-schemas/${pkgs.libadwaita.name}:$XDG_DATA_DIRS"
            '';
          };
        }
      ) // {
      # Export the overlay
      overlays.default = overlay;
    };
}
