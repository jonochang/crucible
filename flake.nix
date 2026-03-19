{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        untanglePkg = pkgs.rustPlatform.buildRustPackage {
          pname = "untangle";
          version = "0.5.4";
          src = pkgs.fetchFromGitHub {
            owner = "jonochang";
            repo = "untangle";
            rev = "v0.5.4";
            hash = "sha256-W8isoQgO7ZExemnNNxXfdb4GGUJ6mWJ4LMPXxWnbKRk=";
          };
          cargoHash = "sha256-NmbYb9gcjvTsDeOImbYzNwJB+NhL3fW+HiABqy/G1c8=";
          nativeBuildInputs = [ pkgs.pkg-config pkgs.cmake ];
          buildInputs = [ pkgs.openssl pkgs.libgit2 pkgs.zlib ];
          OPENSSL_NO_VENDOR = "1";
          LIBGIT2_NO_VENDOR = "1";
          doCheck = false;
        };
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "clippy" "rustfmt" "rust-src" "llvm-tools-preview" ];
        };
      in {
        packages.default = pkgs.callPackage ./package.nix { };
        apps.default = flake-utils.lib.mkApp {
          drv = pkgs.callPackage ./package.nix { };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = [
            rustToolchain
            pkgs.pkg-config pkgs.cmake
            pkgs.openssl pkgs.libgit2
            pkgs.cargo-nextest pkgs.cargo-deny
            pkgs.cargo-llvm-cov pkgs.cargo-insta
            pkgs.mdbook pkgs.git pkgs.gh
            untanglePkg
          ];
          LIBGIT2_NO_VENDOR = "1";
          OPENSSL_DIR = "${pkgs.openssl.dev}";
          OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
          OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
          shellHook = ''git config core.hooksPath .githooks'';
        };
      });
}
