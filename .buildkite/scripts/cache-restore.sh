#!/usr/bin/env bash
# `buildkite-agent cache restore "$@"`, noting which caches didn't restore
# main's (or this branch's) exact entry: a miss, or a fallback to an older
# entry, as when a branch changes Cargo.lock, the toolchain or a cache
# version. cache-save.sh saves those on branches.
set -euo pipefail

log=$(mktemp)
status=0
buildkite-agent cache restore "$@" 2>&1 | tee "$log" || status=$?

# The agent logs one outcome per cache:
#   Cache restored cache_id=target ... fallback_used=false   (exact entry)
#   Cache restored cache_id=target ... fallback_used=true    (older entry)
#   Cache not restored (not found) cache_id=target ...
# If this format ever changes, nothing is recorded and branches just don't
# save, as before.
sed -n -E \
  -e 's/.*Cache restored cache_id=([^ ]+) .*fallback_used=true.*/\1/p' \
  -e 's/.*Cache not restored .*cache_id=([^ ]+).*/\1/p' \
  "$log" >>"${TMPDIR:-/tmp}/cache-inexact"
rm -f "$log"
exit "$status"
