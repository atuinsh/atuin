#!/usr/bin/env bash
# Lints the docs prose as .github/workflows/vale.yml did: vale-action with
# `--minAlertLevel=warning` and reviewdog's fail_on_error, which fails on any
# reported alert. Vale alone only exits non-zero on errors, so fail on any
# output instead.
set -euo pipefail

vale sync
out=$(vale --output=line --minAlertLevel=warning docs/docs || true)
if [ -n "$out" ]; then
  echo "$out"
  echo "+++ :rotating_light: Vale reported $(printf '%s\n' "$out" | wc -l | tr -d ' ') warning(s) or error(s)"
  exit 1
fi
echo "Vale: no warnings or errors."
