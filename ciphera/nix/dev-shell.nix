{ pkgs, lib, rustShell, barretenberg, noir }:
let
  optionalPackage = name:
    lib.optionals (lib.hasAttr name pkgs) [ pkgs.${name} ];

  dockerPackages =
    optionalPackage "docker"
    ++ optionalPackage "docker-compose";

  env = rustShell.env // {
    BB_PATH = "${barretenberg}/bin";
    CIPHERA_ZK_SHELL = "1";
  };
in
{
  inherit env;

  shell = (pkgs.mkShell.override { stdenv = pkgs.llvmPackages_latest.stdenv; }) (env // {
    inputsFrom = [ rustShell.shell ];

    packages = [
      barretenberg
      noir
      pkgs.nodejs_22
      pkgs.go
      pkgs.python310
      pkgs.jq
      pkgs.curl
      pkgs.git
    ] ++ dockerPackages;
  });
}
