#!/bin/bash
set -e

# NOTE: tailscale and atuin are temporarily removed. Tailscale blocked here
# waiting on tailnet device approval, and `atuin login` blocked prompting for a
# 2FA code — both hung the entrypoint before it reached sshd, so the pod came up
# Ready but with nothing running. Access is via `kubectl exec` / sshd meanwhile.

# ---- sshd: access path ----
/usr/sbin/sshd

# ---- optional: dotfiles ----
# If DOTFILES_REPO is set (via template env), clone + run install script once.
if [ -n "${DOTFILES_REPO:-}" ] && [ ! -d /root/.dotfiles ]; then
  git clone --depth 1 "${DOTFILES_REPO}" /root/.dotfiles && \
    ( cd /root/.dotfiles && ( ./install.sh || ./setup.sh || true ) )
fi

# ---- main process ----
# Swap for the pty-proxy / `atuin lab share` invocation when testing share.
exec sleep infinity
