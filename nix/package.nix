{
  lib,
  rustPlatform,
  stdenv,
  pkg-config,
  blueprint-compiler,
  just,
  gettext,
  glib,
  wrapGAppsHook4,
  gtk4,
  libadwaita,
  bubblewrap,
  util-linux,
  coreutils,
  curl,
  gnutar,
  runtimeShell,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = "leyen";
  inherit (cargoToml.workspace.package) version;

  src = lib.fileset.toSource {
    root = ./..;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../data
      ../po
      ../justfile
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  # The install recipe runs under #!/usr/bin/env bash, which the build sandbox lacks.
  postPatch = ''
    substituteInPlace justfile --replace-fail '#!/usr/bin/env bash' '#!${runtimeShell}'
  '';

  nativeBuildInputs = [
    pkg-config
    blueprint-compiler
    just
    gettext
    glib
    wrapGAppsHook4
  ];

  # just's setup hook would take over the build and check phases; cargo's do them.
  dontUseJustBuild = true;
  dontUseJustCheck = true;
  dontUseJustInstall = true;

  buildInputs = [
    gtk4
    libadwaita
  ];

  # bwrap builds the sandbox every game runs in; nsenter puts a game in the process
  # namespace its prefix is served from, which an idle sleep holds open; curl and
  # gnutar fetch umu-launcher and winetricks when they are not in PATH. systemd-run
  # and systemctl come from the system.
  preFixup = ''
    gappsWrapperArgs+=(--suffix PATH : ${
      lib.makeBinPath [
        bubblewrap
        util-linux
        coreutils
        curl
        gnutar
      ]
    })
  '';

  # The justfile installs everything the build does not: desktop entry, metainfo, D-Bus
  # service, icons, completions and catalogues. A destdir of / keeps it from refreshing
  # desktop caches and restarting a running daemon.
  installPhase = ''
    runHook preInstall
    DESTDIR=/ just release=target/${stdenv.hostPlatform.rust.cargoShortTarget}/release prefix=$out install
    runHook postInstall
  '';

  meta = {
    description = "Keep a library of Windows games and run them with Proton through umu-launcher";
    homepage = "https://github.com/sachesi/leyen";
    license = lib.licenses.gpl3Plus;
    mainProgram = "leyen-gtk";
    platforms = lib.platforms.linux;
  };
}
