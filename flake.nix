{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
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
    inputs@{ flake-parts, fenix, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      perSystem =
        { pkgs, system, ... }:
        let
          toolchain = fenix.packages.${system}.fromToolchainFile {
            file = ./rust-toolchain.toml;
            sha256 = "sha256-h+t2xTBz5yt2YIO+1VMIIGlCU7gyp2LYOFvaV1nwOXU=";
          };
          atuin = pkgs.callPackage ./atuin.nix {
            rustPlatform = pkgs.makeRustPlatform {
              cargo = toolchain;
              rustc = toolchain;
            };
          };
        in
        {
          packages = {
            inherit atuin;
            default = atuin;
          };

          formatter = pkgs.nixfmt;

          devShells.default = pkgs.mkShell {
            inputsFrom = [ atuin ];

            nativeBuildInputs =
              (with pkgs; [
                cargo-edit
                rust-analyzer
              ])
              ++ [ toolchain ];

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
          };
        };

      flake.overlays.default = final: _: {
        inherit (inputs.self.packages.${final.stdenv.hostPlatform.system}) atuin;
      };
    };
}
