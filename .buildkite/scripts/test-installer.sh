#!/usr/bin/env bash
# Runs the published installer (setup.atuin.sh, served from this repo's
# install.sh on main) under the given shell and checks atuin runs, as
# .github/workflows/installer.yml did.
set -euo pipefail

shell=$1

# The installer reads its prompts from /dev/tty. GitHub Actions jobs have no
# controlling terminal, so it took its non-interactive path there; the
# Buildkite agent runs commands in a pty, where it waits forever at "import
# your existing shell history?". Run it in a new session with no controlling
# terminal (python3, since macOS has no setsid command) so it behaves as it
# did on GitHub Actions, detection included.
without_tty() {
  python3 -c '
import os, sys
pid = os.fork()
if pid == 0:
    os.setsid()
    os.execvp(sys.argv[1], sys.argv[1:])
_, status = os.waitpid(pid, 0)
sys.exit(os.waitstatus_to_exitcode(status))
' "$@" </dev/null
}

# Single-quoted on purpose: the inner shell expands it, not this one.
# shellcheck disable=SC2016
without_tty "$shell" -c '
  /bin/bash -c "$(curl --proto "=https" --tlsv1.2 -sSf https://setup.atuin.sh)"
  [ -d "$HOME/.atuin" ] && . "$HOME/.atuin/bin/env"
  atuin --help
'
