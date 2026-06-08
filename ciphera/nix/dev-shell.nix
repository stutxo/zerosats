{ pkgs, lib, buildShell, barretenberg, noir }:
let
  optionalPackage = name:
    lib.optionals (lib.hasAttr name pkgs) [ pkgs.${name} ];

  dockerPackages =
    optionalPackage "docker"
    ++ optionalPackage "docker-compose";

  env = buildShell.env // {
    BB_PATH = "${barretenberg}/bin";
  };
in
{
  inherit env;

  shell = (pkgs.mkShell.override { stdenv = pkgs.llvmPackages_latest.stdenv; }) (env // {
    inputsFrom = [ buildShell.shell ];

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
