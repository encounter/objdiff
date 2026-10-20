{
  lib,
  rustPlatform,
  pkg-config,
  makeWrapper,
  makeDesktopItem,
  fontconfig,
  freetype,
  libGL,
  libx11,
  libxcursor,
  libxi,
  libxrandr,
  libxkbcommon,
  vulkan-loader,
  wayland,
  xdg-utils,
  version,
}:
{
  crateName,
  binaryName,
  withGui ? false,
}:
let
  desktopItem = makeDesktopItem {
    name = "objdiff";
    desktopName = "objdiff";
    comment = "Compare object files in decompilation projects";
    exec = "objdiff";
    icon = "objdiff";
    categories = [ "Development" ];
    startupWMClass = "objdiff";
  };
  runtimeLibraries = [
    libGL
    libxkbcommon
    vulkan-loader
    wayland
    libx11
    libxcursor
    libxi
    libxrandr
  ];
in
rustPlatform.buildRustPackage {
  pname = crateName;
  inherit version;

  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      let
        name = baseNameOf path;
      in
      !lib.elem name [
        ".git"
        ".worktrees"
        "result"
        "target"
      ];
  };

  # Import registry dependencies directly from Cargo.lock so lock-file updates
  # do not require refreshing a single vendor hash.
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "--package"
    crateName
  ];
  cargoTestFlags = [
    "--package"
    crateName
  ];

  nativeBuildInputs = [ pkg-config ] ++ lib.optionals withGui [ makeWrapper ];
  buildInputs = lib.optionals withGui [
    fontconfig
    freetype
    wayland
    libx11
  ];

  env = lib.optionalAttrs withGui {
    OBJDIFF_DISABLE_SELF_UPDATE = "1";
  };

  postInstall = lib.optionalString withGui ''
    wrapProgram "$out/bin/${binaryName}" \
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath runtimeLibraries} \
      --prefix PATH : ${lib.makeBinPath [ xdg-utils ]}
    install -Dm644 objdiff-gui/assets/icon.png \
      "$out/share/icons/hicolor/512x512/apps/objdiff.png"
    install -Dm644 ${desktopItem}/share/applications/objdiff.desktop \
      "$out/share/applications/objdiff.desktop"
  '';

  meta = {
    description = "A local diffing tool for decompilation projects";
    homepage = "https://github.com/encounter/objdiff";
    license = with lib.licenses; [
      asl20
      mit
    ];
    mainProgram = binaryName;
    platforms = lib.platforms.linux;
  };
}
