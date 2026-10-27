#!/usr/bin/env bash
# Run after a failed test step: a proptest failure appends a `cc <seed>` line
# to a regression file, so show which ones changed and upload them as build
# artifacts to reproduce locally.
set -uo pipefail

echo "+++ :warning: proptest regression files changed by this run"
git add -N -- '*proptest-regressions*' 2>/dev/null || true
if git status --porcelain -- '*proptest-regressions*' | grep -q .; then
  git status --porcelain -- '*proptest-regressions*'
  git --no-pager diff -- '*proptest-regressions*'
  buildkite-agent artifact upload '**/proptest-regressions/**;**/*.proptest-regressions'
else
  echo "None."
fi
