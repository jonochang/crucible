{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
    untangle.url = "github:jonochang/untangle";
    untangle.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, untangle }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        untanglePkg = untangle.packages.${system}.default.overrideAttrs (old: {
          src = pkgs.fetchFromGitHub {
            owner = "jonochang";
            repo = "untangle";
            rev = "v${old.version}";
            hash = "sha256-zHhk6f50bjiRi0PnY3YWcVvlzhj8q1NLroPxDVYOP7o=";
          };
          doCheck = false;
        });
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "clippy" "rustfmt" "rust-src" ];
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
            pkgs.mdbook pkgs.git
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
