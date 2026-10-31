{
  pkgs,
  lib,
  config,
  inputs,
  ...
}:

# One shell for Atuin work, with what CI runs (https://devenv.sh).
#
# devenv shell     # CI's Rust toolchain and tools, and the sandbox below
# devenv up -d     # Postgres, for atuin-server's integration tests
# ci-clippy        # each ci-* script is a .buildkite/pipeline.yml step's commands
#
# The shell is a sandbox: ATUIN_HOME, ATUIN_CONFIG_DIR, the XDG directories and
# the daemon socket point under .devenv/state/atuin/, with a config.toml that
# never syncs. A `cargo run -- …`, a dev daemon or the e2e suite cannot reach
# your own history, config, sync account or launchd agent. `atuin-dev-reset`
# wipes it; `atuin-real <command>` runs one command against your real profile.
#
# Every version here is CI's, read from where CI pins it, so a bump there is a
# bump here. Where nixpkgs ships a different version, evaluation fails and says
# so rather than drift.

let
  inherit (pkgs.stdenv.hostPlatform) system;

  # `NAME=value` from a shell script CI sources.
  shellVar =
    file: name:
    let
      found = builtins.match ".*\n${name}=([^\n]*)\n.*" (builtins.readFile file);
    in
    if found == null then throw "${name} is not set in ${toString file}" else builtins.head found;
  ciLib = ./.buildkite/scripts/lib.sh;
  ciToolchain = ./.buildkite/scripts/toolchain.sh;

  # The format step's toolchain: `RUST_TOOLCHAIN: nightly-<date>` in the pipeline.
  nightlyDate =
    let
      found = builtins.match ".*RUST_TOOLCHAIN: nightly-([0-9-]+)\n.*" (
        builtins.readFile ./.buildkite/pipeline.yml
      );
    in
    if found == null then
      throw "no nightly RUST_TOOLCHAIN in .buildkite/pipeline.yml"
    else
      builtins.head found;
  msrv = (lib.importTOML ./Cargo.toml).workspace.package.rust-version;

  pinned =
    pkg: version:
    if pkg.version == version then
      pkg
    else
      throw ''
        nixpkgs' ${pkg.pname} is ${pkg.version}; CI pins ${version}.
        Move the nixpkgs input to a revision that has ${version}, or package
        CI's download as devenv.nix does for Vale.
      '';

  rustBin = inputs.rust-overlay.lib.mkRustBin { } pkgs.buildPackages;
  nightlyRustfmt = rustBin.nightly.${nightlyDate}.minimal.override { extensions = [ "rustfmt" ]; };
  msrvToolchain = rustBin.stable.${msrv}.minimal;
  # Nightly rustfmt as the shell's rustfmt, so `cargo fmt` and editors format
  # as CI checks. Only these two binaries: the nightly cargo and rustc stay off
  # PATH.
  rustfmt = pkgs.runCommand "rustfmt-nightly-${nightlyDate}" { } ''
    mkdir -p $out/bin
    for bin in rustfmt cargo-fmt; do
      printf '#!/bin/sh\nexec ${nightlyRustfmt}/bin/%s "$@"\n' "$bin" >$out/bin/$bin
      chmod +x $out/bin/$bin
    done
  '';

  # CI's Vale release (nixpkgs is ahead of the pin), with lib.sh's hash for
  # each platform.
  vale =
    let
      version = shellVar ciLib "VALE_VERSION";
      asset =
        {
          x86_64-linux = "Linux_64-bit";
          aarch64-linux = "Linux_arm64";
          x86_64-darwin = "macOS_64-bit";
          aarch64-darwin = "macOS_arm64";
        }
        .${system};
      shaVar =
        {
          x86_64-linux = "VALE_SHA256";
          aarch64-linux = "VALE_LINUX_ARM64_SHA256";
          x86_64-darwin = "VALE_MACOS_X86_64_SHA256";
          aarch64-darwin = "VALE_MACOS_ARM64_SHA256";
        }
        .${system};
    in
    pkgs.stdenvNoCC.mkDerivation {
      pname = "vale";
      inherit version;
      src = pkgs.fetchurl {
        url = "https://github.com/errata-ai/vale/releases/download/v${version}/vale_${version}_${asset}.tar.gz";
        sha256 = shellVar ciLib shaVar;
      };
      sourceRoot = ".";
      dontBuild = true;
      installPhase = "install -Dm755 vale $out/bin/vale";
      meta.mainProgram = "vale";
    };

  sandbox = "${config.devenv.state}/atuin";
  profile = "${sandbox}/home";

  # Creates what is missing; a reset deletes first.
  seedSandbox = pkgs.writeShellScript "atuin-dev-seed" ''
    set -euo pipefail
    mkdir -p "$ATUIN_HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"
    mkdir -p -m 700 "$(dirname "$ATUIN_DAEMON__SOCKET_PATH")"
    if [ ! -e "$ATUIN_HOME/config.toml" ]; then
      cat >"$ATUIN_HOME/config.toml" <<'EOF'
    # Atuin's devenv sandbox profile (devenv.nix); `atuin-dev-reset` recreates it.
    # Nothing here reaches your account: sync is off, and the address is a local
    # `atuin server`.
    auto_sync = false
    update_check = false
    sync_address = "http://127.0.0.1:8888"
    EOF
    fi
  '';

  # Every ci-* script runs from the repo root and stops at the first failure,
  # printing the pipeline's section titles.
  ciScript = body: {
    exec = ''
      set -euo pipefail
      cd "$DEVENV_ROOT"
      section() { echo "--- $1"; shift; "$@"; }
      ${body}
    '';
  };
in
{
  languages.rust = {
    enable = true;
    channel = "stable";
    version = (lib.importTOML ./rust-toolchain.toml).toolchain.channel;
    # No stable rustfmt: the nightly one above is the shell's.
    components = [
      "rustc"
      "cargo"
      "clippy"
      "rust-src"
      "rust-analyzer"
    ];
  };

  packages = [
    rustfmt
    (pinned pkgs.cargo-nextest (shellVar ciToolchain "NEXTEST_VERSION"))
    (pinned pkgs.cargo-deny (shellVar ciToolchain "CARGO_DENY_VERSION"))
    (pinned pkgs.shellcheck (shellVar ciLib "SHELLCHECK_VERSION"))
    (pinned pkgs.codespell (shellVar ciLib "CODESPELL_VERSION"))
    vale
    # OPENSSL_NO_VENDOR builds (clippy, as in CI) link the system OpenSSL;
    # the rest build the vendored one, which needs perl.
    pkgs.openssl
    pkgs.pkg-config
    pkgs.perl
    # The shells the e2e suite drives, and ble.sh for bash.
    pkgs.bash
    pkgs.zsh
    pkgs.fish
    pkgs.blesh
  ];

  env = {
    ATUIN_HOME = profile;
    # Redundant with ATUIN_HOME for config.toml, but it is what the launchd
    # backend's serves_this_profile() tests: with it set, a dev daemon never
    # adopts or installs the user's agent.
    ATUIN_CONFIG_DIR = profile;
    XDG_CONFIG_HOME = "${sandbox}/xdg/config";
    XDG_DATA_HOME = "${sandbox}/xdg/data";
    XDG_CACHE_HOME = "${sandbox}/xdg/cache";
    # Short, and apart from the real daemon's default socket.
    ATUIN_DAEMON__SOCKET_PATH = "${config.env.DEVENV_RUNTIME}/atuin.sock";
    # devenv's Postgres by its socket, which only that server can answer: with it stopped, the
    # tests find no Postgres rather than whichever one has the port.
    ATUIN_DB_URI = "postgres://atuin:pass@localhost:${toString config.env.PGPORT}/atuin?host=${config.env.PGHOST}";
    ATUIN_E2E_BLESH = "${pkgs.blesh}/share/blesh/ble.sh";
  };

  services.postgres = {
    enable = true;
    package = pkgs.postgresql_18;
    # No TCP: a socket in devenv's runtime directory can't collide with, or be mistaken for, a
    # Postgres of the contributor's own.
    listen_addresses = "";
    initialDatabases = [
      {
        name = "atuin";
        user = "atuin";
        pass = "pass";
      }
    ];
    # The atuin role needs its password, as in CI's Postgres image: a test of `db_password_file`
    # fails on a server that would let it in without one. Everyone else is trusted, for devenv's
    # own readiness probe and for psql.
    hbaConf = ''
      local all atuin scram-sha-256
      local all all   trust
    '';
  };

  scripts = {
    atuin-dev-reset = {
      description = "Wipe the Atuin sandbox under .devenv/state/atuin (stopping a dev daemon in it) and recreate it";
      exec = ''
        set -euo pipefail
        # atuin stops the daemon, not a kill of the pidfile's PID: after a crash that PID may be
        # another process's, and atuin signals only a process that still holds the pidfile.
        if [ -e "$ATUIN_HOME/data/atuin-daemon.pid" ]; then
          for atuin in "$DEVENV_ROOT/target/debug/atuin" "$DEVENV_ROOT/target/release/atuin" "$(command -v atuin || true)"; do
            if [ -x "$atuin" ]; then
              "$atuin" daemon stop || echo "atuin daemon stop failed; removing the sandbox anyway" >&2
              break
            fi
          done
        fi
        rm -rf ${lib.escapeShellArg sandbox} "$ATUIN_DAEMON__SOCKET_PATH"
        ${seedSandbox}
        echo "sandbox reset: ${sandbox}"
      '';
    };
    atuin-real = {
      description = "Run a command against your real Atuin profile instead of the sandbox";
      exec = ''
        if [ $# -eq 0 ]; then
          echo "usage: atuin-real <command> [args…]   e.g. atuin-real target/debug/atuin stats" >&2
          exit 2
        fi
        exec env -u ATUIN_HOME -u ATUIN_CONFIG_DIR -u ATUIN_DAEMON__SOCKET_PATH \
          -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME "$@"
      '';
    };

    ci-check = ciScript ''
      section ":package: cargo fetch" cargo fetch --locked
      section ":mag: check (no features)" cargo check --no-default-features --workspace
      section ":mag: check (sync)" cargo check --no-default-features --features sync --workspace
      section ":mag: check (server)" cargo check -p atuin-server
      section ":mag: check (client only)" cargo check --no-default-features --features client --workspace
      section ":mag: check (release profile)" cargo check --workspace --locked --release
    '';
    ci-clippy = ciScript ''
      export OPENSSL_NO_VENDOR=1
      section ":paperclip: clippy (all features; lib, bins, tests)" cargo clippy --all-features --lib --bins --tests -- -D warnings
    '';
    ci-test = ciScript ''
      # Extra arguments go to nextest: `ci-test -p atuin-common`.
      # The tests run as on a CI agent: no ATUIN_* variables but CI's, in a fresh HOME. Some
      # tests read the real environment, and fail under the sandbox's ATUIN_HOME or socket path
      # (or one of your own). The fresh HOME and TMPDIR stand in for the sandbox: a test that
      # reaches for ~/.atuin or the default socket ($TMPDIR/atuin-$UID) finds neither your
      # profile nor your daemon. ATUIN_CONFIG_DIR stays set, so no test touches your launchd
      # agent.
      for var in $(env | sed -n 's/^\(ATUIN_[A-Z0-9_]*\)=.*/\1/p'); do
        case $var in
          ATUIN_CONFIG_DIR | ATUIN_DB_URI | ATUIN_E2E_*) ;;
          *) unset "$var" ;;
        esac
      done
      export CARGO_HOME="''${CARGO_HOME:-$HOME/.cargo}"
      export HOME="$DEVENV_STATE/atuin/test-home" TMPDIR="$DEVENV_RUNTIME/tmp"
      rm -rf "$HOME" "$TMPDIR"
      mkdir -p "$HOME" "$TMPDIR"
      export NEXTEST_PROFILE=ci ATUIN_E2E_REQUIRE_SHELLS=1

      # Whether the nextest arguments select any of atuin-server's integration tests, the ones
      # that need Postgres. A package selection settles it; a filterset is left to nextest, which
      # alone knows what `not package(atuin-server)` or `rdeps(…)` match.
      needs_postgres() {
        local packages=() excludes=() filters=()
        while [ $# -gt 0 ]; do
          case $1 in
            -p | --package) packages+=("$2") && shift ;;
            -p*) packages+=("''${1#-p}") ;;
            --package=*) packages+=("''${1#*=}") ;;
            --exclude) excludes+=("$2") && shift ;;
            --exclude=*) excludes+=("''${1#*=}") ;;
            -E | --filterset | --filter-expr) filters+=(-E "$2") && shift ;;
            -E*) filters+=(-E "''${1#-E}") ;;
            --filterset=* | --filter-expr=*) filters+=(-E "''${1#*=}") ;;
          esac
          shift
        done
        if [ ''${#packages[@]} -gt 0 ]; then
          [[ " ''${packages[*]} " == *" atuin-server "* ]] || return 1
        elif [[ " ''${excludes[*]} " == *" atuin-server "* ]]; then
          return 1
        fi
        [ ''${#filters[@]} -eq 0 ] ||
          [ -n "$(cargo nextest list -p atuin-server --tests "''${filters[@]}" --message-format oneline)" ]
      }
      if ! pg_isready -q -h "$PGHOST" -p "$PGPORT"; then
        if needs_postgres "$@"; then
          echo "Postgres is not running: \`devenv up -d\` starts it. Without it, skip atuin-server's" >&2
          echo "integration tests with \`ci-test -E 'not package(atuin-server)'\`." >&2
          exit 1
        fi
        echo "Postgres is not running; going on, since this run has none of atuin-server's integration tests." >&2
      fi
      section ":test_tube: cargo nextest" cargo nextest run --lib --bins --tests "$@"
    '';
    ci-fmt = ciScript ''
      export PATH="${nightlyRustfmt}/bin:$PATH"
      section ":art: cargo fmt --check" cargo fmt -- --check
    '';
    ci-deny = ciScript ''
      section ":no_entry: cargo deny check" cargo deny --log-level warn --all-features check
      for crate in atuin-common atuin-history atuin-domain atuin-pty-proxy; do
        section ":no_entry: bans: $crate" cargo deny --log-level warn --manifest-path crates/$crate/Cargo.toml --all-features check bans
      done
    '';
    ci-docs = ciScript ''
      export RUSTDOCFLAGS="-D warnings"
      section ":books: cargo doc" cargo doc --document-private-items --no-deps --workspace
    '';
    ci-vale = ciScript ''
      section ":memo: vale" .buildkite/scripts/vale.sh
    '';
    ci-codespell = ciScript ''
      section ":pencil2: codespell" codespell --count --exclude-file CHANGELOG.md
    '';
    ci-shellcheck = ciScript ''
      export SHELLCHECK_OPTS="-e SC2148"
      section ":shell: shellcheck" .buildkite/scripts/shellcheck.sh
    '';
    ci-msrv = ciScript ''
      export PATH="${msrvToolchain}/bin:$PATH"
      section ":crab: check on MSRV (all features)" cargo check --locked --workspace --all-features
    '';
  };

  enterShell = ''
    ${seedSandbox}
    ${lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
      # OPENSSL_NO_VENDOR builds dlopen libssl from sqlx's proc macro; see atuin.nix.
      export LD_LIBRARY_PATH="${
        lib.makeLibraryPath [ pkgs.openssl ]
      }''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    ''}
    # The styles .vale.ini pins, fetched once per change to it (.vale/styles/ is not in git).
    if [ ! .vale/styles/.synced -nt .vale.ini ]; then
      if vale sync >/dev/null; then
        touch .vale/styles/.synced
      else
        echo "vale sync failed (offline?); run it before ci-vale" >&2
      fi
    fi
    echo ""
    echo "atuin devenv: Rust $(rustc --version | cut -d' ' -f2), rustfmt nightly-${nightlyDate}, MSRV ${msrv}"
    echo "  sandbox  : $ATUIN_HOME (atuin-dev-reset wipes it; atuin-real <cmd> uses your real profile)"
    echo "  daemon   : $ATUIN_DAEMON__SOCKET_PATH"
    echo "  postgres : $ATUIN_DB_URI (devenv up -d)"
    echo "  CI steps : ci-check ci-clippy ci-test ci-fmt ci-deny ci-docs ci-vale ci-codespell ci-shellcheck ci-msrv"
    echo ""
  '';
}
