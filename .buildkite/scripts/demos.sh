#!/usr/bin/env bash
# Renders the README demos (demos/render.sh) with the atuin from the Linux
# test archive, so this step needs no Rust build of its own. Runs as root in
# a bare Ubuntu 26.04 (see the step in pipeline.yml).
set -euo pipefail

# shellcheck source=lib.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# The agents the threads, mcp and history demos run.
CLAUDE_CODE_VERSION=2.1.323
CODEX_VERSION=0.167.2

# Uploaded from here, not with the step's artifact_paths: mono runs this
# pipeline from atuin/, and artifact_paths resolve from the checkout root.
# Also on failure, for betamax's failure bundles.
trap 'buildkite-agent artifact upload "demos/out/**/*" || true' EXIT

section ":shell: Install zsh, sshd and node" \
  apt_install ca-certificates curl git zstd zsh openssh-server sqlite3 nodejs npm
section ":movie_camera: Install betamax" install_betamax
# The agents the threads demo runs.
section ":robot_face: Install Claude Code and Codex" \
  npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
  "@openai/codex@${CODEX_VERSION}"

section ":arrow_down: Download atuin" \
  buildkite-agent artifact download "test-archive/*-linux.*" .
tar --zstd -xf test-archive/tests-linux.tar.zst target/debug/atuin \
  target/debug/atuin-server

# A debug build reads each crate's i18n/ catalogue from disk, at its path in
# the checkout that built it, and panics without the catalogue. This job's
# checkout is elsewhere, so copy the catalogues to that path. They must be
# real directories, since rust-embed rejects files that resolve outside its
# folder.
built=$(cat test-archive/checkout-linux.txt)
if [ "$built" != "$PWD" ]; then
  for dir in crates/*/i18n; do
    mkdir -p "$built/$dir"
    cp -R "$dir/." "$built/$dir"
  done
fi

# The threads demo runs real agents on the step's API keys, which only mono's
# pipeline gets: the public atuin pipeline renders the rest.
tapes=()
for tape in demos/*.tape; do
  if [ "$tape" = demos/threads.tape ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    echo "No ANTHROPIC_API_KEY, so leaving out $tape"
    continue
  fi
  tapes+=("$(basename "$tape")")
done
section ":movie_camera: Render" demos/render.sh "${tapes[@]}"
