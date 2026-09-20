{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "wall-dashboards";
  version = "0.2.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
      # The pages are baked into the binary with include_str!.
      ./pages
    ];
  };

  # The lock file is the source of truth, so there is no cargoHash to keep in
  # step with it.
  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "Air quality, weather and departure panels for a wall display";
    homepage = "https://github.com/float3/wall-dashboards";
    license = lib.licenses.mit;
    mainProgram = "wall-dashboards";
    platforms = lib.platforms.linux;
  };
}
