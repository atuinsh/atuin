# Atuin package definition
#
# Built with crane rather than buildRustPackage (as nixpkgs does:
#     https://github.com/NixOS/nixpkgs/blob/master/pkgs/by-name/at/atuin/package.nix)
# so dependencies are cached separately from atuin itself.
#
# Helpful documentation: https://crane.dev
{
  lib,
  stdenv,
  installShellFiles,
  craneLib,
  libiconv,
  pkg-config,
  openssl,
}:
let
  # Shared by the dependency-only build and the final build, so the cached
  # dependency artifacts match what the final build expects.
  commonArgs = {
    pname = "atuin";
    version = (lib.importTOML ./Cargo.toml).workspace.package.version;

    src = lib.cleanSource ./.;

    nativeBuildInputs = [
      installShellFiles
      pkg-config
    ];

    buildInputs = [ openssl ] ++ lib.optionals stdenv.hostPlatform.isDarwin [ libiconv ];

    OPENSSL_NO_VENDOR = 1;

    # native-tls pulls OpenSSL into the sqlx-macros proc-macro, which rustc dlopens at
    # build time. With OPENSSL_NO_VENDOR it links libssl.so.3 dynamically, so the loader
    # needs it on LD_LIBRARY_PATH while the proc-macro is loaded.
    preBuild = ''
      export LD_LIBRARY_PATH="${lib.makeLibraryPath [ openssl ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    '';

    doCheck = false;
  };
in
craneLib.buildPackage (
  commonArgs
  // {
    # Dependencies build in their own derivation, keyed only on Cargo.toml and
    # Cargo.lock, so source changes reuse them from the binary cache.
    cargoArtifacts = craneLib.buildDepsOnly commonArgs;

    # The linked binary records no RPATH for the libraries it loads from outside
    # glibc: OpenSSL, and libstdc++ for the Luau VM. Without one, it fails with
    # "cannot open shared object file". postInstall runs the patched binary
    # without LD_LIBRARY_PATH, so a missing library fails the build instead.
    postInstall =
      lib.optionalString stdenv.hostPlatform.isLinux ''
        patchelf --add-rpath ${
          lib.makeLibraryPath [
            openssl
            stdenv.cc.cc.lib
          ]
        } $out/bin/atuin
      ''
      + ''
        unset LD_LIBRARY_PATH
        # A failure inside <(...) below doesn't reach `set -e`.
        $out/bin/atuin --version
        installShellCompletion --cmd atuin \
          --bash <($out/bin/atuin gen-completions -s bash) \
          --fish <($out/bin/atuin gen-completions -s fish) \
          --zsh <($out/bin/atuin gen-completions -s zsh)
      '';

    meta = with lib; {
      description = "Replacement for a shell history which records additional commands context with optional encrypted synchronization between machines";
      homepage = "https://github.com/atuinsh/atuin";
      license = licenses.mit;
      mainProgram = "atuin";
    };
  }
)
