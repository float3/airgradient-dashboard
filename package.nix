{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "airgradient-dashboard";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
      # The page is baked into the binary with include_str!.
      ./index.html
    ];
  };

  # The lock file is the source of truth, so there is no cargoHash to keep in
  # step with it.
  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "Local history, charts and an out-of-range alarm for an AirGradient monitor";
    homepage = "https://github.com/float3/airgradient-dashboard";
    license = lib.licenses.mit;
    mainProgram = "airgradient-dashboard";
    platforms = lib.platforms.linux;
  };
}
