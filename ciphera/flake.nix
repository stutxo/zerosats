{
  description = "Ciphera development shell";

  inputs = {
    nixpkgs.url      = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url  = "github:numtide/flake-utils";
    barretenberg-nix = {
      url = "git+ssh://git@github.com/zerosats/barretenberg-nix.git?ref=macos";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, barretenberg-nix, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        lib = pkgs.lib;
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        barretenberg-pkg = barretenberg-nix.packages.${system}.default;
        noir-pkg = barretenberg-nix.packages.${system}.noir;

        buildShell = import ./nix/build-shell.nix {
          inherit pkgs lib rustToolchain;
        };

        devShell = import ./nix/dev-shell.nix {
          inherit pkgs lib buildShell;
          barretenberg = barretenberg-pkg;
          noir = noir-pkg;
        };
      in
      {
        devShells = {
          default = devShell.shell;
          build = buildShell.shell;
        };
      }
    );
}
