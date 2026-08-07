#!/bin/sh
# Feature install script — runs at image build time as root.
set -e

USER_HOME="/root"

# setup.atuin.sh is a bash script; Debian's /bin/sh is dash and chokes on its
# bash-isms ("Bad substitution"). Pipe to bash explicitly.
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | bash

# shell hooks. The $(atuin init …) must stay literal so it runs when the shell
# starts, not here; only $USER_HOME is spliced in.
# shellcheck disable=SC2016
echo 'eval "$('"$USER_HOME"'/.atuin/bin/atuin init zsh)"' >>"$USER_HOME/.zshrc"
# shellcheck disable=SC2016
echo 'eval "$('"$USER_HOME"'/.atuin/bin/atuin init bash)"' >>"$USER_HOME/.bashrc"

# PATH for non-login shells (kubectl exec)
ln -sf "$USER_HOME/.atuin/bin/atuin" /usr/local/bin/atuin

# sandbox-safe config: fixed daemon socket, since pods have no logind
# session and therefore no XDG_RUNTIME_DIR.
mkdir -p "$USER_HOME/.config/atuin"
cat >"$USER_HOME/.config/atuin/config.toml" <<TOML
$([ -n "${SYNCADDRESS}" ] && echo "sync_address = \"${SYNCADDRESS}\"")

[daemon]
enabled = true
socket_path = "/root/.local/run/atuin.sock"
TOML
