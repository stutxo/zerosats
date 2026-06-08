{ pkgs, lib, system }:

let
  archives = {
    aarch64-darwin = ../web/binaries/darwin/barretenberg-arm64-darwin.tar.gz;
    x86_64-darwin = ../web/binaries/darwin/barretenberg-amd64-darwin.tar.gz;
    aarch64-linux = ../web/binaries/linux64/barretenberg-arm64-linux.tar.gz;
    x86_64-linux = ../web/binaries/linux64/barretenberg-amd64-linux.tar.gz;
  };

  src = archives.${system} or (throw "No checked-in Barretenberg binary for ${system}");
in
pkgs.stdenvNoCC.mkDerivation {
  pname = "barretenberg-bin";
  version = "3.0.0-nightly.20251030-2";
  inherit src;

  dontConfigure = true;
  dontBuild = true;

  unpackPhase = ''
    runHook preUnpack
    tar -xzf "$src"
    runHook postUnpack
  '';

  installPhase = ''
    runHook preInstall
    install -Dm755 bb "$out/bin/bb"
    runHook postInstall
  '';

  meta = {
    description = "Checked-in Barretenberg bb binary used by Ciphera";
    mainProgram = "bb";
    platforms = lib.attrNames archives;
  };
}
