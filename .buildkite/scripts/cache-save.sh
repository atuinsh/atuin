#!/usr/bin/env bash
# `buildkite-agent cache save "$@"`, on the default branch only (main,
# including its scheduled builds). Branch and PR builds restore main's
# entries (exact or fallback, per the cache registry policy) and never save:
# a branch's first build would otherwise spend ~20s uploading copies of
# entries identical to main's, and branches never write caches at all.
set -euo pipefail

if [ "${BUILDKITE_BRANCH:-}" != "${BUILDKITE_PIPELINE_DEFAULT_BRANCH:-main}" ]; then
  echo "Not saving caches ($*) on ${BUILDKITE_BRANCH:-?}: only ${BUILDKITE_PIPELINE_DEFAULT_BRANCH:-main} saves."
  exit 0
fi
buildkite-agent cache save "$@"
