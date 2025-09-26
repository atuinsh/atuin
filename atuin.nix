# Atuin package definition
#
# This file will be similar to the package definition in nixpkgs:
# https://github.com/NixOS/nixpkgs/blob/master/pkgs/by-name/at/atuin/package.nix
#
# Helpful documentation: https://github.com/NixOS/nixpkgs/blob/master/doc/languages-frameworks/rust.section.md
{
  lib,
  stdenv,
  installShellFiles,
  rustPlatform,
  libiconv,
  gitRev,
}:
let
  fs = lib.fileset;
  src = fs.difference (fs.gitTracked ./.) (
    fs.unions [
      ./demo.gif
      ./flake.lock
      (fs.fileFilter (file: lib.strings.hasInfix ".git" file.name) ./.)
      (fs.fileFilter (file: file.hasExt "svg") ./.)
      (fs.fileFilter (file: file.hasExt "md") ./.)
      (fs.fileFilter (file: file.hasExt "nix") ./.)
    ]
  );
  packageVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

in
rustPlatform.buildRustPackage {
  pname = "atuin";
  version = "${packageVersion}-unstable-${gitRev}";

  src = fs.toSource {
    root = ./.;
    fileset = src;
  };

  cargoLock = {
    lockFile = ./Cargo.lock;
    # Allow dependencies to be fetched from git and avoid having to set the outputHashes manually
    allowBuiltinFetchGit = true;
  };

  nativeBuildInputs = [ installShellFiles ];

  buildInputs = lib.optionals stdenv.isDarwin [
    libiconv
  ];

  postInstall = ''
    installShellCompletion --cmd atuin \
      --bash <($out/bin/atuin gen-completions -s bash) \
      --fish <($out/bin/atuin gen-completions -s fish) \
      --zsh <($out/bin/atuin gen-completions -s zsh)
  '';

  doCheck = false;

  meta = {
    description = "Replacement for a shell history which records additional commands context with optional encrypted synchronization between machines";
    homepage = "https://github.com/atuinsh/atuin";
    license = lib.licenses.mit;
    mainProgram = "atuin";
  };
}
