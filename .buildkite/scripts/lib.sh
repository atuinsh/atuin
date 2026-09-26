# Shared helpers, sourced (not executed) by toolchain.sh and by the steps
# that don't need Rust. Runs on Linux and macOS hosted agents, so it sticks
# to bash 3.2 (macOS's /bin/bash).

# Runs a command under its own collapsible log group. Buildkite groups all
# output after a `--- title` line until the next one; on failure `^^^ +++`
# expands the group so the error is visible.
section() {
  local title=$1
  shift
  echo "--- $title"
  "$@" && return 0
  local status=$?
  echo "^^^ +++"
  return "$status"
}

verify_sha256() { # <sha256> <file>
  if command -v sha256sum >/dev/null; then
    echo "$1  $2" | sha256sum -c -
  else
    echo "$1  $2" | shasum -a 256 -c -
  fi
}

fetch_verified() { # <url> <sha256> <file>
  curl -fsSL -o "$3" "$1"
  verify_sha256 "$2" "$3"
}

# Downloaded .debs, kept by the `apt_debs` cache so later jobs can install
# them with dpkg instead of paying ~5s for `apt-get update` + download.
APT_DEBS_DIR="$HOME/.cache/apt-debs"
mkdir -p "$APT_DEBS_DIR/partial"

# Installs whichever of the given apt packages are missing (Linux only).
apt_install() {
  local sudo="" missing="" pkg
  [ "$(id -u)" -eq 0 ] || sudo=sudo
  for pkg in "$@"; do
    dpkg -s "$pkg" >/dev/null 2>&1 || missing="$missing $pkg"
  done
  [ -n "$missing" ] || return 0
  # Fast path: the .debs a previous job downloaded. --refuse-downgrade makes
  # a newer base image fall through to apt rather than downgrade its
  # packages to the cached versions.
  if ls "$APT_DEBS_DIR"/*.deb >/dev/null 2>&1 &&
    $sudo dpkg -i --refuse-downgrade "$APT_DEBS_DIR"/*.deb >/dev/null; then
    echo "Installed$missing from cached .debs"
    return 0
  fi
  # Download into APT_DEBS_DIR (not /var/cache/apt/archives, which images
  # often clean after every install) so they can be cached. -f repairs
  # anything a failed dpkg fast path left half-configured.
  $sudo apt-get update -qq
  # shellcheck disable=SC2086 # word-split the package list
  $sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq -f --no-install-recommends \
    -o Dir::Cache::archives="$APT_DEBS_DIR" -o APT::Keep-Downloaded-Packages=true $missing
}

# nextest's `--partition count:K/N` for a step split with Buildkite's
# `parallelism: N`, and nothing for a single job, so how many ways a test
# step is split is just that one number in the pipeline.
nextest_partition() {
  if [ -n "${BUILDKITE_PARALLEL_JOB_COUNT:-}" ]; then
    echo "--partition count:$((BUILDKITE_PARALLEL_JOB + 1))/${BUILDKITE_PARALLEL_JOB_COUNT}"
  fi
}

# Prebuilt tools not tied to Rust go here.
TOOLS_BIN="$HOME/.local/bin"
mkdir -p "$TOOLS_BIN"
export PATH="$TOOLS_BIN:$PATH"

SHELLCHECK_VERSION=0.11.0
SHELLCHECK_SHA256=b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6
VALE_VERSION=3.15.2
VALE_SHA256=fc72e64454d6bd7af91905d4faebbf411bae3eec17bb572f4101311212bc0d9e
CODESPELL_VERSION=2.4.3
NIX_INSTALLER_VERSION=3.22.5
NIX_INSTALLER_SHA256=9432d40ec1d0d4ebb6284848cc389bdb1bbf7aeb5c1cea11e7efc9daf0a1ec82

install_shellcheck() {
  fetch_verified "https://github.com/koalaman/shellcheck/releases/download/v${SHELLCHECK_VERSION}/shellcheck-v${SHELLCHECK_VERSION}.linux.x86_64.tar.gz" \
    "$SHELLCHECK_SHA256" /tmp/shellcheck.tgz
  tar -xzf /tmp/shellcheck.tgz -C /tmp
  mv "/tmp/shellcheck-v${SHELLCHECK_VERSION}/shellcheck" "$TOOLS_BIN/"
  shellcheck --version | sed -n 2p
}

install_vale() {
  fetch_verified "https://github.com/errata-ai/vale/releases/download/v${VALE_VERSION}/vale_${VALE_VERSION}_Linux_64-bit.tar.gz" \
    "$VALE_SHA256" /tmp/vale.tgz
  tar -xzf /tmp/vale.tgz -C "$TOOLS_BIN" vale
  vale --version
}

# In its own venv: Ubuntu's system Python refuses `pip install` (PEP 668).
install_codespell() {
  if ! python3 -m venv /tmp/codespell 2>/dev/null; then
    apt_install python3-venv
    python3 -m venv /tmp/codespell
  fi
  /tmp/codespell/bin/pip install --quiet "codespell==${CODESPELL_VERSION}"
  ln -sf /tmp/codespell/bin/codespell "$TOOLS_BIN/codespell"
  codespell --version
}

# Upstream Nix (what cachix/install-nix-action installs), root-only with no
# daemon (`--init none`), so it doesn't depend on the image running systemd.
install_nix() {
  local sudo=""
  [ "$(id -u)" -eq 0 ] || sudo=sudo
  fetch_verified "https://github.com/DeterminateSystems/nix-installer/releases/download/v${NIX_INSTALLER_VERSION}/nix-installer-x86_64-linux" \
    "$NIX_INSTALLER_SHA256" /tmp/nix-installer
  chmod +x /tmp/nix-installer
  $sudo /tmp/nix-installer install linux --no-confirm --init none --prefer-upstream-nix
  export PATH="/nix/var/nix/profiles/default/bin:$PATH"
  nix --version
}
