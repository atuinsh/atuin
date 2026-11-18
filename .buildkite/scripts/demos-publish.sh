#!/usr/bin/env bash
# Deploys the demos the demos step rendered to the atuin-demos Cloudflare
# Pages project, served at atuin-demos.pages.dev. Each deploy replaces the
# last.
set -euo pipefail

# shellcheck source=lib.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# The version web/pnpm-lock.yaml pins for the hub's deploys.
WRANGLER_VERSION=4.142.0

site=$(mktemp -d)
section ":arrow_down: Download demos" \
  buildkite-agent artifact download "demos/out/*.webp" "$site" --step demos

section ":cloudflare: Deploy atuin-demos" \
  pnpm dlx "wrangler@${WRANGLER_VERSION}" pages deploy "$site/demos/out" \
  --project-name atuin-demos --branch main --commit-hash "$BUILDKITE_COMMIT"
