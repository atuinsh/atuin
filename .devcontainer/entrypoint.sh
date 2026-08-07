#!/bin/bash
set -e

# ---- tailscale: userspace networking, ephemeral node, tailscale ssh ----
tailscaled --tun=userspace-networking --statedir=/var/lib/ts-state &
tailscale up \
  --authkey="${TS_AUTHKEY}" \
  --ssh \
  --hostname="sbx-${SANDBOX_NAME:-$(hostname)}" \
  || echo "WARN: tailscale up failed; falling back to sshd/exec"

# ---- sshd: editor/fallback path alongside tailscale ssh ----
/usr/sbin/sshd

# ---- atuin: login + daemon on the fixed socket ----
mkdir -p /root/.local/run
if ! atuin status >/dev/null 2>&1; then
  atuin login -u "${ATUIN_USER}" -p "${ATUIN_PASS}" -k "${ATUIN_KEY}" \
    || echo "WARN: atuin login failed; check atuin-creds secret"
fi
nohup atuin daemon > /root/.local/run/daemon.log 2>&1 &

# ---- optional: dotfiles ----
# If DOTFILES_REPO is set (via template env), clone + run install script once.
if [ -n "${DOTFILES_REPO:-}" ] && [ ! -d /root/.dotfiles ]; then
  git clone --depth 1 "${DOTFILES_REPO}" /root/.dotfiles && \
    ( cd /root/.dotfiles && ( ./install.sh || ./setup.sh || true ) )
fi

# ---- main process ----
# Swap for the pty-proxy / `atuin lab share` invocation when testing share.
exec sleep infinity
