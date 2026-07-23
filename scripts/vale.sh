#!/usr/bin/env bash
# Lint the Atuin docs with Vale (https://vale.sh).
#
# Installs a pinned, checksum-verified Vale into .vale/bin on first use, syncs
# the pinned style packages whenever .vale.ini changes, then lints. Arguments
# are passed through; with no path argument it lints docs/docs.
set -euo pipefail

VALE_VERSION="3.13.0"
VALE_BASE_URL="${VALE_BASE_URL:-https://github.com/errata-ai/vale/releases/download/v${VALE_VERSION}}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
vale_dir="${ATUIN_VALE_DIR:-$repo_root/.vale}"
vale_bin="$vale_dir/bin/vale"
stamp="$vale_dir/.sync-stamp"

case "$(uname -s)/$(uname -m)" in
Linux/x86_64)
  asset="vale_${VALE_VERSION}_Linux_64-bit.tar.gz"
  want_sha="4378ee4bc7c2493760826270e55d5569cda35d7c89582e9fdc3070e2a1089193"
  ;;
Linux/aarch64 | Linux/arm64)
  asset="vale_${VALE_VERSION}_Linux_arm64.tar.gz"
  want_sha="2134f23e7afbdf70b44272e6d3b5f26e85972340faa1e2a2b194358cf2892d84"
  ;;
Darwin/x86_64)
  asset="vale_${VALE_VERSION}_macOS_64-bit.tar.gz"
  want_sha="9f2991092579e85dd5be082c691b7b14ddbcd7c65477a6ff44b5f5e8dc3a9079"
  ;;
Darwin/arm64)
  asset="vale_${VALE_VERSION}_macOS_arm64.tar.gz"
  want_sha="2e89bd82cadfffa6abebda80a141529db2799df5d4197e6aa0489a4d711d8a3b"
  ;;
*)
  echo "vale.sh: unsupported platform $(uname -s)/$(uname -m)" >&2
  exit 1
  ;;
esac

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

install_vale() {
  local tmp got
  tmp="$(mktemp -d)"
  echo "vale.sh: downloading Vale $VALE_VERSION" >&2
  if ! curl -fsSL "$VALE_BASE_URL/$asset" -o "$tmp/$asset"; then
    rm -rf "$tmp"
    echo "vale.sh: download failed: $VALE_BASE_URL/$asset" >&2
    exit 1
  fi
  got="$(sha256_of "$tmp/$asset")"
  if [ "$got" != "$want_sha" ]; then
    rm -rf "$tmp"
    echo "vale.sh: checksum mismatch for $asset" >&2
    echo "  expected $want_sha" >&2
    echo "  got      $got" >&2
    exit 1
  fi
  tar -xzf "$tmp/$asset" -C "$tmp" vale
  mkdir -p "$vale_dir/bin"
  mv "$tmp/vale" "$vale_bin"
  chmod +x "$vale_bin"
  rm -rf "$tmp"
}

if [ ! -x "$vale_bin" ] || ! "$vale_bin" --version 2>/dev/null | grep -q "$VALE_VERSION"; then
  install_vale
fi

for arg in "$@"; do
  case "$arg" in
  -v | --version | -h | --help) exec "$vale_bin" "$arg" ;;
  esac
done

cd "$repo_root"

want_stamp="$(sha256_of "$repo_root/.vale.ini")"
if [ ! -f "$stamp" ] || [ "$(cat "$stamp")" != "$want_stamp" ]; then
  "$vale_bin" --no-global sync
  mkdir -p "$vale_dir"
  printf '%s\n' "$want_stamp" >"$stamp"
fi

has_path=0
for arg in "$@"; do
  case "$arg" in
  -*) ;;
  *) has_path=1 ;;
  esac
done
if [ "$has_path" -eq 0 ]; then
  set -- "$@" docs/docs
fi

exec "$vale_bin" --no-global "$@"
