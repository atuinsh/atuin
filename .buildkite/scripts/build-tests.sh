#!/usr/bin/env bash
# Compiles the tests once into a nextest archive, lists them as bktec
# selectors (nextest-bktec.py), and uploads both for run-tests.sh. Runs after
# toolchain.sh and the target cache restore.
#
# Usage: build-tests.sh <os> <cargo target selection...>
set -euo pipefail

# shellcheck source=lib.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

os=$1
shift
mkdir -p test-archive

section ":package: cargo nextest archive" \
  cargo nextest archive --archive-file "test-archive/tests-${os}.tar.zst" "$@"

# Listing reuses the build above; nothing recompiles.
echo "--- :straight_ruler: List the tests"
cargo nextest list "$@" --message-format json >"test-archive/list-${os}.json"
python3 .buildkite/scripts/nextest-bktec.py selectors \
  "test-archive/list-${os}.json" "test-archive/selectors-${os}.txt"
# The test jobs need the build's checkout path: the test binaries have it
# compiled in (run-tests.sh).
echo "$PWD" >"test-archive/checkout-${os}.txt"

section ":arrow_up: Upload the archive and test list" \
  buildkite-agent artifact upload "test-archive/tests-${os}.tar.zst;test-archive/selectors-${os}.txt;test-archive/checkout-${os}.txt"
