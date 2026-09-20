{
  lib,
  stdenvNoCC,
  python3,
  makeWrapper,
}:
stdenvNoCC.mkDerivation {
  pname = "airgradient-dashboard";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [./server.py ./index.html];
  };

  nativeBuildInputs = [makeWrapper];
  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall
    mkdir -p $out/share/airgradient-dashboard
    cp server.py index.html $out/share/airgradient-dashboard/
    makeWrapper ${python3}/bin/python3 $out/bin/airgradient-dashboard \
      --add-flags $out/share/airgradient-dashboard/server.py
    runHook postInstall
  '';

  meta = {
    description = "Local history and charts for an AirGradient monitor";
    homepage = "https://github.com/float3/airgradient-dashboard";
    license = lib.licenses.mit;
    mainProgram = "airgradient-dashboard";
    platforms = lib.platforms.linux;
  };
}
