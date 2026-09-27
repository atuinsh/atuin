#!/usr/bin/env bash
# `buildkite-agent cache save --name <cache>...`. main (including its
# scheduled builds) saves every cache. Branch and PR builds restore main's
# entries, so they save only the caches cache-restore.sh found no exact
# entry for: a branch that changes Cargo.lock, the toolchain or a cache
# version is warm from its second build, and every other branch skips
# uploading copies of main's entries. The cache registry policy scopes a
# branch's entries to that branch, so main never restores them.
set -euo pipefail

if [ "${BUILDKITE_BRANCH:-}" = "${BUILDKITE_PIPELINE_DEFAULT_BRANCH:-main}" ]; then
  exec buildkite-agent cache save "$@"
fi

inexact="${TMPDIR:-/tmp}/cache-inexact"
args=()
skipped=()
saving=0
while [ $# -gt 0 ]; do
  if [ "$1" = --name ] && [ $# -ge 2 ]; then
    if [ -f "$inexact" ] && grep -qxF "$2" "$inexact"; then
      args+=(--name "$2")
      saving=$((saving + 1))
    else
      skipped+=("$2")
    fi
    shift 2
  else
    args+=("$1")
    shift
  fi
done

if [ ${#skipped[@]} -gt 0 ]; then
  echo "Not saving ${skipped[*]} on ${BUILDKITE_BRANCH:-?}: main's entry restored exactly."
fi
if [ "$saving" -gt 0 ]; then
  buildkite-agent cache save "${args[@]}"
fi
