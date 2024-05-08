{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };
  outputs = {
    self,
    nixpkgs,
    flake-utils,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = nixpkgs.outputs.legacyPackages.${system};
    in {
      packages.atuin = pkgs.callPackage ./atuin.nix {
        inherit (pkgs.darwin.apple_sdk.frameworks) Security SystemConfiguration AppKit;
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
