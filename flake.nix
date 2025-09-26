{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    {
      self,
      nixpkgs,
      fenix,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      eachSystem = lib.genAttrs systems;
      pkgsFor = eachSystem (
        system:
        import nixpkgs {
          localSystem.system = system;
          overlays = [ self.overlays.atuin ];
        }
      );

    in
    {

      
        packages = eachSystem (system: {
        inherit (pkgsFor.${system}) atuin;

        default = self.packages.${system}.atuin;
      });

      devShells = lib.mapAttrs (system: pkgs: {

        default = self.packages.${system}.default.overrideAttrs (super: {
          nativeBuildInputs =
            with pkgs;
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
      }) pkgsFor;

      formatter = lib.mapAttrs (_: pkgs: pkgs.nixfmt) pkgsFor;

      overlays = {
        atuin =
          final: _:
          let
            toolchain = fenix.packages.${final.system}.fromToolchainFile {
              file = ./rust-toolchain.toml;
              sha256 = "sha256-SJwZ8g0zF2WrKDVmHrVG3pD2RGoQeo24MEXnNx5FyuI=";
            };
            rustPlatform = final.makeRustPlatform {
              cargo = toolchain;
              rustc = toolchain;
            };
          in
          {
            atuin = final.callPackage ./atuin.nix {
              inherit rustPlatform;
              gitRev = self.shortRev;
              inherit (final.darwin.apple_sdk.frameworks) Security SystemConfiguration AppKit;
            };

            default = self.overlays.atuin;
          };
      };
    };
}
