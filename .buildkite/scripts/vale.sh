#!/usr/bin/env bash
# Lints the docs prose as .github/workflows/vale.yml did: vale-action with
# `--minAlertLevel=warning` and reviewdog's fail_on_error, which fails on any
# reported alert. Vale alone only exits non-zero on errors, so fail on any
# output as well as on a non-zero exit.
set -euo pipefail

vale sync

# Vale exits 1 when it reports error-level alerts and 2 when it couldn't lint
# at all (bad config, runtime errors; details on stderr, which passes
# through). Keep the status so a crash can't pass as "no warnings".
status=0
out=$(vale --output=line --minAlertLevel=warning docs/docs) || status=$?
if [ -n "$out" ]; then
  echo "$out"
  echo "+++ :rotating_light: Vale reported $(printf '%s\n' "$out" | wc -l | tr -d ' ') warning(s) or error(s)"
  exit 1
fi
if [ "$status" -ne 0 ]; then
  echo "+++ :rotating_light: Vale exited with status $status without reporting any alerts (see its errors above)"
  exit "$status"
fi
echo "Vale: no warnings or errors."
