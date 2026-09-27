#!/usr/bin/env bash
# Runs the published installer (setup.atuin.sh, served from this repo's
# install.sh on main) under the given shell and checks atuin runs, as
# .github/workflows/installer.yml did.
set -euo pipefail

shell=$1
# Single-quoted on purpose: the inner shell expands it, not this one.
# shellcheck disable=SC2016
"$shell" -c '
  /bin/bash -c "$(curl --proto "=https" --tlsv1.2 -sSf https://setup.atuin.sh)"
  [ -d "$HOME/.atuin" ] && . "$HOME/.atuin/bin/env"
  atuin --help
'
