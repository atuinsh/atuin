{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = {
    self,
    nixpkgs,
    flake-utils,
    fenix,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = nixpkgs.outputs.legacyPackages.${system};
    in {
      packages.atuin = pkgs.callPackage ./atuin.nix {
        inherit (pkgs.darwin.apple_sdk.frameworks) Security SystemConfiguration AppKit;
        rustPlatform = let
          toolchain =
            fenix.packages.${system}.fromToolchainFile
            {
              file = ./rust-toolchain.toml;
              sha256 = "sha256-3jVIIf5XPnUU1CRaTyAiO0XHVbJl12MSx3eucTXCjtE=";
            };
        in
          pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };
      };
      packages.default = self.outputs.packages.${system}.atuin;

      devShells.default = self.packages.${system}.default.overrideAttrs (super: {
        nativeBuildInputs = with pkgs;
          super.nativeBuildInputs
          ++ [
            cargo-edit
            clippy
            rustfmt
          ];
        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

        shellHook = ''
          echo >&2 "Setting development database path"
          export ATUIN_DB_PATH="/tmp/atuin_dev.db"
          export ATUIN_RECORD_STORE_PATH="/tmp/atuin_records.db"

          if [ -e "''${ATUIN_DB_PATH}" ]; then
            echo >&2 "''${ATUIN_DB_PATH} already exists, you might want to double-check that"
          fi

          if [ -e "''${ATUIN_RECORD_STORE_PATH}" ]; then
            echo >&2 "''${ATUIN_RECORD_STORE_PATH} already exists, you might want to double-check that"
          fi
        '';
      });
    })
    // {
      overlays.default = final: prev: {
        inherit (self.packages.${final.system}) atuin;
      };
    };
}
