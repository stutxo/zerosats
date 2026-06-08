{ pkgs, lib, rustToolchain }:
let
  llvm = pkgs.llvmPackages_latest;

  env = {
    RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
    LIBCLANG_PATH = lib.makeLibraryPath [ llvm.libclang.lib ];

    CC = "cc";
    CXX = "c++";
    CXXFLAGS = "-include cstdint";
    CMAKE_C_FLAGS = "-march=x86-64";
    CMAKE_CXX_FLAGS = "-march=x86-64";
  };
in
{
  inherit env rustToolchain;

  shell = (pkgs.mkShell.override { stdenv = llvm.stdenv; }) (env // {
    nativeBuildInputs = [
      pkgs.cmake
      pkgs.pkg-config
      pkgs.ninja
      llvm.bintools
      llvm.clang
      rustToolchain
    ];

    buildInputs = [
      pkgs.openssl
      pkgs.protobuf
      llvm.openmp
    ];
  });
}
