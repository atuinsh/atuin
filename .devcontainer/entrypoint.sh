#!/bin/bash
set -e

# NOTE: tailscale and atuin are temporarily removed. Tailscale blocked here
# waiting on tailnet device approval, and `atuin login` blocked prompting for a
# 2FA code — both hung the entrypoint before it reached sshd, so the pod came up
# Ready but with nothing running. Access is via `kubectl exec` / sshd meanwhile.

# ---- sshd: access path ----
# This entrypoint now runs as `dev`, and sshd needs root to bind :22 and read
# the host keys — hence sudo (passwordless, see the Dockerfile). Non-fatal: a
# sandbox is still reachable via `kubectl exec` without it.
sudo /usr/sbin/sshd || echo "WARN: sshd failed to start; use kubectl exec"

# ---- repos: clone into the workspace ----
# SBX_REPOS is a space-separated list of owner/name (set via template env).
# Cloned in the background so the shell is usable immediately — watch progress
# with `tail -f /workspaces/.clone.log`. Private repos need GH_TOKEN; gh acts as
# the git credential helper so the token is never written to disk.
if [ -n "${SBX_REPOS:-}" ]; then
  if [ -n "${GH_TOKEN:-}" ]; then
    gh auth setup-git || echo "WARN: gh auth setup-git failed; private clones will fail"
  fi
  read -ra _repos <<<"$SBX_REPOS"
  (
    for repo in "${_repos[@]}"; do
      dest="/workspaces/${repo##*/}"
      if [ -e "$dest" ]; then
        echo "skip ${repo} (already present)"
        continue
      fi
      echo "cloning ${repo}..."
      git clone "https://github.com/${repo}.git" "$dest" ||
        echo "WARN: clone ${repo} failed"
    done
    echo "clone pass complete"
  ) >/workspaces/.clone.log 2>&1 &
fi

# ---- optional: dotfiles ----
# If DOTFILES_REPO is set (via template env), clone + run install script once.
if [ -n "${DOTFILES_REPO:-}" ] && [ ! -d "$HOME/.dotfiles" ]; then
  git clone --depth 1 "${DOTFILES_REPO}" "$HOME/.dotfiles" && \
    ( cd "$HOME/.dotfiles" && ( ./install.sh || ./setup.sh || true ) )
fi

# ---- main process ----
# Swap for the pty-proxy / `atuin lab share` invocation when testing share.
exec sleep infinity
