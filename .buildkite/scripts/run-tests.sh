#!/usr/bin/env bash
# Runs this parallel job's share of the tests build-tests.sh archived and
# planned for <os>.
#
# Usage: run-tests.sh <os>
set -euo pipefail

# shellcheck source=lib.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

os=$1
archive=test-archive/tests-${os}.tar.zst
plan=test-archive/plan-${os}.json

section ":arrow_down: Download the tests" \
  buildkite-agent artifact download "test-archive/*-${os}.*" .

# Compile-time paths in the tests (env!("CARGO_BIN_EXE_atuin"),
# env!("CARGO_MANIFEST_DIR"), rstest's #[files]) point into the build job's
# checkout, which is at a different path on every agent. Make that path lead
# here, where the archive is extracted to (target/) next to the sources.
built=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["checkout"])' "$plan")
if [ "$built" != "$PWD" ]; then
  if [ -e "$built" ] && [ ! -L "$built" ]; then
    echo "+++ :rotating_light: $built exists on this agent; can't point it at $PWD"
    exit 1
  fi
  mkdir -p "$(dirname "$built")"
  ln -sfn "$PWD" "$built"
fi

# TEST_TMPDIR: where the tests' tempdirs go, without moving the downloads
# above there too.
export TMPDIR="${TEST_TMPDIR:-${TMPDIR:-/tmp}}"

echo "+++ :test_tube: cargo nextest (job $((BUILDKITE_PARALLEL_JOB + 1)) of ${BUILDKITE_PARALLEL_JOB_COUNT})"
python3 .buildkite/scripts/test-plan.py run "$plan" -- \
  cargo-nextest nextest run --archive-file "$archive" \
  --workspace-remap "$PWD" --extract-to "$PWD" --extract-overwrite ||
  { .buildkite/scripts/proptest-regressions.sh; exit 1; }
