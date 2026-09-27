#!/usr/bin/env bash
# Compiles the tests once into a nextest archive and plans how the test
# step's parallel jobs split them (test-plan.py), then uploads both for
# run-tests.sh. Runs after toolchain.sh and the target cache restore.
#
# Usage: build-tests.sh <os> <cargo target selection...>
# TEST_JOBS must match the test step's parallelism.
set -euo pipefail

# shellcheck source=lib.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

os=$1
shift
mkdir -p test-archive

section ":package: cargo nextest archive" \
  cargo nextest archive --archive-file "test-archive/tests-${os}.tar.zst" "$@"

# Listing reuses the build above; nothing recompiles.
echo "--- :straight_ruler: Plan the test jobs"
cargo nextest list "$@" --message-format json >"test-archive/list-${os}.json"
python3 .buildkite/scripts/test-plan.py plan "test-archive/list-${os}.json" \
  test-timings/timings.json "$os" "$TEST_JOBS" "test-archive/plan-${os}.json"

section ":arrow_up: Upload the archive and plan" \
  buildkite-agent artifact upload "test-archive/tests-${os}.tar.zst;test-archive/plan-${os}.json"
