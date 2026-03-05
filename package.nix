{ pkgs ? import <nixpkgs> {} }:
let
  src = pkgs.lib.cleanSource ./.;
  rustPlatform = pkgs.makeRustPlatform {
    cargo = pkgs.cargo;
    rustc = pkgs.rustc;
  };
  openssl = pkgs.openssl;
  libgit2 = pkgs.libgit2;
  pkgconfig = pkgs.pkg-config;
  cmake = pkgs.cmake;
  buildInputs = [ openssl libgit2 ];
  nativeBuildInputs = [ pkgconfig cmake ];
  env = {
    OPENSSL_DIR = "${openssl.dev}";
    OPENSSL_LIB_DIR = "${openssl.out}/lib";
    OPENSSL_INCLUDE_DIR = "${openssl.dev}/include";
    LIBGIT2_NO_VENDOR = "0";
  };
  in rustPlatform.buildRustPackage ({
    pname = "crucible";
    version = "0.1.13";
    inherit src buildInputs nativeBuildInputs;
    cargoLock.lockFile = ./Cargo.lock;
    cargoBuildFlags = [ "-p" "crucible-cli" ];
    meta.mainProgram = "crucible";
  } // env)
