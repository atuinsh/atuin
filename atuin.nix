# Atuin package definition
#
# This file will be similar to the package definition in nixpkgs:
#     https://github.com/NixOS/nixpkgs/blob/master/pkgs/by-name/at/atuin/package.nix
#
# Helpful documentation: https://github.com/NixOS/nixpkgs/blob/master/doc/languages-frameworks/rust.section.md
{
  lib,
  stdenv,
  installShellFiles,
  rustPlatform,
  libiconv,
  pkg-config,
  openssl,
}:
rustPlatform.buildRustPackage {
  name = "atuin";

  src = lib.cleanSource ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
    # Allow dependencies to be fetched from git and avoid having to set the outputHashes manually
    allowBuiltinFetchGit = true;
  };

  nativeBuildInputs = [
    installShellFiles
    pkg-config
  ];

  buildInputs = [ openssl ] ++ lib.optionals stdenv.isDarwin [ libiconv ];

  OPENSSL_NO_VENDOR = 1;

  # native-tls pulls OpenSSL into the sqlx-macros proc-macro, which rustc dlopens at
  # build time. With OPENSSL_NO_VENDOR it links libssl.so.3 dynamically, so the loader
  # needs it on LD_LIBRARY_PATH while the proc-macro is loaded.
  preBuild = ''
    export LD_LIBRARY_PATH="${lib.makeLibraryPath [ openssl ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  '';

  # The linked binary records no RPATH for OpenSSL, so once it leaves the build
  # sandbox it fails with "libssl.so.3: cannot open shared object file". The
  # build never notices, because LD_LIBRARY_PATH above is still exported when
  # postInstall runs the binary to generate completions.
  postFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
    patchelf --add-rpath ${lib.makeLibraryPath [ openssl ]} $out/bin/atuin
  '';

  postInstall = ''
    installShellCompletion --cmd atuin \
      --bash <($out/bin/atuin gen-completions -s bash) \
      --fish <($out/bin/atuin gen-completions -s fish) \
      --zsh <($out/bin/atuin gen-completions -s zsh)
  '';

  doCheck = false;

  meta = with lib; {
    description = "Replacement for a shell history which records additional commands context with optional encrypted synchronization between machines";
    homepage = "https://github.com/atuinsh/atuin";
    license = licenses.mit;
    mainProgram = "atuin";
  };
}
