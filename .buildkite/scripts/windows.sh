#!/usr/bin/env bash
# The Windows steps. The agent runs step commands through cmd.exe, which
# carries on past a failing line, so each Windows step is one command that
# runs this script in Git Bash instead.
#
# Usage: windows.sh check|tests
set -euo pipefail

# shellcheck source=toolchain.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/toolchain.sh"

.buildkite/scripts/cache-restore.sh --name cargo_registry --name target

case "$1" in
  check)
    section ":package: cargo fetch" cargo fetch --locked
    # The registry key includes the OS, so Windows saves its own.
    .buildkite/scripts/cache-save.sh --name cargo_registry
    section ":mag: check (all features)" cargo check --all-features --workspace
    section ":mag: check (no features)" cargo check --no-default-features --workspace
    section ":mag: check (sync)" cargo check --no-default-features --features sync --workspace
    section ":mag: check (server)" cargo check -p atuin-server
    section ":mag: check (client only)" cargo check --no-default-features --features client --workspace
    section ":mag: check (release profile)" cargo check --workspace --locked --release
    ;;
  tests)
    section ":test_tube: cargo nextest" cargo nextest run --lib --bins ||
      { .buildkite/scripts/proptest-regressions.sh; exit 1; }
    ;;
  *)
    echo "Unknown step: $1"
    exit 1
    ;;
esac

section ":broom: Prune target/" .buildkite/scripts/prune-target.sh
.buildkite/scripts/cache-save.sh --name target
