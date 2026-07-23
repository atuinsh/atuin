#!/usr/bin/env bash
# Verifies scripts/vale.sh refuses a tampered download and installs the pinned
# version from a clean state.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fail() {
  echo "FAIL: $1" >&2
  exit 1
}

# A well-formed tarball that is not the pinned Vale release, served over
# file:// in place of the GitHub release.
mkdir -p "$tmp/fake"
printf '#!/bin/sh\necho tampered\n' >"$tmp/fake/vale"
chmod +x "$tmp/fake/vale"
tar -czf "$tmp/vale_3.13.0_Linux_64-bit.tar.gz" -C "$tmp/fake" vale
for name in vale_3.13.0_Linux_arm64.tar.gz \
  vale_3.13.0_macOS_64-bit.tar.gz \
  vale_3.13.0_macOS_arm64.tar.gz; do
  cp "$tmp/vale_3.13.0_Linux_64-bit.tar.gz" "$tmp/$name"
done

set +e
out="$(ATUIN_VALE_DIR="$tmp/tampered" VALE_BASE_URL="file://$tmp" \
  "$repo_root/scripts/vale.sh" --version 2>&1)"
status=$?
set -e

[ "$status" -eq 0 ] && fail "tampered download was accepted (exit 0)"
case "$out" in
*"checksum mismatch"*) ;;
*) fail "expected 'checksum mismatch' in output, got: $out" ;;
esac
[ -x "$tmp/tampered/bin/vale" ] && fail "tampered binary was installed"

out="$(ATUIN_VALE_DIR="$tmp/clean" "$repo_root/scripts/vale.sh" --version 2>&1)"
case "$out" in
*"3.13.0"*) ;;
*) fail "expected version 3.13.0, got: $out" ;;
esac

echo "PASS"
