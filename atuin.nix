# Atuin package definition
#
# This file will be similar to the package definition in nixpkgs:
#     https://github.com/NixOS/nixpkgs/blob/master/pkgs/tools/misc/atuin/default.nix
#
# Helpful documentation: https://github.com/NixOS/nixpkgs/blob/master/doc/languages-frameworks/rust.section.md
{
  lib,
  stdenv,
  installShellFiles,
  rustPlatform,
  libiconv,
  Security,
  SystemConfiguration,
}:
rustPlatform.buildRustPackage {
  name = "atuin";

  src = lib.cleanSource ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
    # Allow dependencies to be fetched from git and avoid having to set the outputHashes manually
    allowBuiltinFetchGit = true;
  };

  nativeBuildInputs = [installShellFiles];

  buildInputs = lib.optionals stdenv.isDarwin [libiconv Security SystemConfiguration];

  postInstall = ''
    installShellCompletion --cmd atuin \
      --bash <($out/bin/atuin gen-completions -s bash) \
      --fish <($out/bin/atuin gen-completions -s fish) \
      --zsh <($out/bin/atuin gen-completions -s zsh)
  '';

  # Additional flags passed to the cargo test binary, see `cargo test -- --help`
  checkFlags = [
    # Registration tests require a postgres server
    "--skip=registration"
  ];

  meta = with lib; {
    description = "Replacement for a shell history which records additional commands context with optional encrypted synchronization between machines";
    homepage = "https://github.com/atuinsh/atuin";
    license = licenses.mit;
  };
}
