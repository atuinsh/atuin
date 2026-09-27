# Sourced (not executed) at the start of each Rust step: installs the Rust
# toolchain and CI tools. Shared helpers (`section` etc.) come from lib.sh.
#
# Per-step knobs, set as step env:
#   RUST_TOOLCHAIN   toolchain to install (default: rust-toolchain.toml)
#   RUST_COMPONENTS  extra rustup components, e.g. "clippy"
#   CARGO_TOOLS      prebuilt tools to install: "nextest", "deny"
#   APT_PACKAGES     extra apt packages on Linux (libssl-dev + pkg-config always)
#   BREW_PACKAGES    Homebrew packages on macOS
#
# Runs on Linux and macOS hosted agents, so it sticks to bash 3.2 (macOS's
# /bin/bash).

# lib.sh is checked on its own; shellcheck only follows sources with -x.
# shellcheck disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

NEXTEST_VERSION=0.9.146
NEXTEST_LINUX_SHA256=682c21b777c333e96fd532e114d3a5a894e0729ab88d94c0a9f20f8419695428
NEXTEST_MACOS_SHA256=39785160b3c2f6ed9a765049cf4fa79f3b39aa02eb7598a5a0e2a1a0b9ffb9a8
CARGO_DENY_VERSION=0.20.2
CARGO_DENY_SHA256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f

echo "--- :rust: Toolchain"
export CARGO_HOME="$HOME/.cargo" RUSTUP_HOME="$HOME/.rustup"
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TERM_COLOR=always CARGO_INCREMENTAL=0

case "$(uname -s)" in
  Linux)
    # shellcheck disable=SC2086 # word-split the package list
    apt_install libssl-dev pkg-config ${APT_PACKAGES:-}
    nextest_asset=x86_64-unknown-linux-gnu nextest_sha256=$NEXTEST_LINUX_SHA256
    ;;
  Darwin)
    if [ -n "${BREW_PACKAGES:-}" ]; then
      # shellcheck disable=SC2086 # word-split the package list
      HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_INSTALL_CLEANUP=1 brew install $BREW_PACKAGES
    fi
    nextest_asset=universal-apple-darwin nextest_sha256=$NEXTEST_MACOS_SHA256
    ;;
  *)
    echo "Unsupported OS: $(uname -s)"
    return 1
    ;;
esac

if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --no-modify-path --profile minimal --default-toolchain none
fi
if [ -n "${RUST_TOOLCHAIN:-}" ]; then
  rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
  export RUSTUP_TOOLCHAIN="$RUST_TOOLCHAIN"
else
  # The channel pinned in rust-toolchain.toml. Named explicitly: a bare
  # `rustup toolchain install` needs rustup 1.28+, and hosted images may
  # ship an older one.
  channel=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
  rustup toolchain install "$channel" --profile minimal
fi
for component in ${RUST_COMPONENTS:-}; do
  rustup component add "$component"
done

# rustup may come preinstalled from elsewhere (a system package, the image),
# in which case nothing has created $CARGO_HOME/bin yet.
mkdir -p "$CARGO_HOME/bin"
for tool in ${CARGO_TOOLS:-}; do
  case "$tool" in
    nextest)
      fetch_verified "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${NEXTEST_VERSION}/cargo-nextest-${NEXTEST_VERSION}-${nextest_asset}.tar.gz" \
        "$nextest_sha256" /tmp/nextest.tgz
      tar -xzf /tmp/nextest.tgz -C "$CARGO_HOME/bin"
      ;;
    deny)
      fetch_verified "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
        "$CARGO_DENY_SHA256" /tmp/cargo-deny.tgz
      tar -xzf /tmp/cargo-deny.tgz -C /tmp
      mv "/tmp/cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl/cargo-deny" "$CARGO_HOME/bin/"
      ;;
    *)
      echo "Unknown CARGO_TOOLS entry: $tool"
      return 1
      ;;
  esac
done

rustc --version
cargo --version
