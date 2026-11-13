# shellcheck shell=bash
# Sourced by render.sh for threads.tape: two machines, a laptop (the HOME the
# tape starts in) and a devbox (ssh devbox), syncing through a local
# atuin-server, each with its own daemon. The devbox is a second HOME behind
# an unprivileged sshd on loopback, so the hop in the tape is a real ssh.
#
# The agents are real: claude on the laptop, codex on the devbox, on
# ANTHROPIC_API_KEY and OPENAI_API_KEY.
#   CLAUDE_BIN, CODEX_BIN  (default: claude and codex on PATH)
#   SYNC_PORT, SSH_PORT    (default: 18888 and 2222)

# shellcheck source=common.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

rig_up() {
  local work=$1 theme=$2
  local server_bin=${ATUIN_SERVER_BIN:-$demos/../target/debug/atuin-server}
  local claude_bin codex_bin key
  local sync_port=${SYNC_PORT:-18888} ssh_port=${SSH_PORT:-2222}
  local laptop=$work/laptop devbox=$work/devbox

  : "${ANTHROPIC_API_KEY:?the threads demo runs claude, which needs it}"
  : "${OPENAI_API_KEY:?the threads demo runs codex, which needs it}"
  claude_bin=$(command -v "${CLAUDE_BIN:-claude}")
  codex_bin=$(command -v "${CODEX_BIN:-codex}")
  server_bin=$(absolute "$server_bin")

  cp -R fixtures/laptop/. "$laptop/"
  make_home "$devbox" devbox

  # The checkout both machines work in, at the same path on each, as a
  # resumed thread goes back to its directory. It's the devbox's own, so
  # Codex, which shortens paths under HOME, shows it as ~/src/atuin.
  local repo=$devbox/src/atuin
  make_repo "$repo"
  ln -s "$devbox/src" "$laptop/src"

  ln -s "$claude_bin" "$laptop/bin/claude"
  ln -s "$claude_bin" "$devbox/bin/claude"
  ln -s "$codex_bin" "$devbox/bin/codex"
  # codex is a node script. The node itself, not a version manager's shim,
  # which needs the environment the devbox doesn't have.
  ln -s "$(node -p process.execPath)" "$devbox/bin/node"

  local home
  for home in "$laptop" "$devbox"; do
    # The daemons sync every second, so the devbox has the laptop's thread by
    # the time the tape gets there.
    sed -i.bak 's/^auto_sync = false$/auto_sync = true/' "$home/.config/atuin/config.toml"
    rm "$home/.config/atuin/config.toml.bak"
    cat >>"$home/.config/atuin/config.toml" <<EOF
sync_address = "http://127.0.0.1:$sync_port"

[sync]
records = true
EOF
    echo "sync_frequency = 1" | configure_daemon "$home"
  done

  configure_claude "$laptop" "$repo" "$theme"
  mkdir -p "$devbox/.codex"
  cat >"$devbox/.codex/config.toml" <<EOF
check_for_update_on_startup = false

[projects."$repo"]
trust_level = "trusted"
EOF
  (umask 077 && printf '{"auth_mode":"apikey","OPENAI_API_KEY":"%s"}\n' \
    "$OPENAI_API_KEY" >"$devbox/.codex/auth.json")

  ATUIN_DB_URI="sqlite://$work/server.db?mode=rwc" ATUIN_OPEN_REGISTRATION=true \
    ATUIN_HOST=127.0.0.1 ATUIN_PORT=$sync_port ATUIN_CONFIG_DIR=$work \
    "$server_bin" start >"$work/server.log" 2>&1 &
  echo $! >>"$work/pids"
  local tries=0
  until curl -sf "http://127.0.0.1:$sync_port" >/dev/null; do
    tries=$((tries + 1))
    if [ "$tries" -gt 50 ]; then
      cat "$work/server.log"
      return 1
    fi
    sleep 0.1
  done

  laptop_env=(HOME="$laptop" PATH="$laptop/bin:/usr/bin:/bin" ATUIN_HOST_NAME=laptop)
  devbox_env=(HOME="$devbox" PATH="$devbox/bin:/usr/bin:/bin" ATUIN_HOST_NAME=devbox)
  env -i "${laptop_env[@]}" atuin register -u demo -e demo@example.com \
    -p demo-password >/dev/null 2>&1
  key=$(env -i "${laptop_env[@]}" atuin key)
  env -i "${devbox_env[@]}" atuin login -u demo -p demo-password -k "$key" >/dev/null 2>&1

  start_daemon "$work/laptop-daemon.log" "${laptop_env[@]}"
  start_daemon "$work/devbox-daemon.log" "${devbox_env[@]}"

  # The devbox's sshd, as whoever runs this, so it needs no root. Its one key
  # lands in the devbox's HOME.
  mkdir -p "$work/sshd" "$laptop/.ssh"
  ssh-keygen -q -t ed25519 -N '' -f "$work/sshd/host_key"
  ssh-keygen -q -t ed25519 -N '' -f "$laptop/.ssh/id_ed25519"
  local env_opts=() var
  for var in "${devbox_env[@]}" TERM=xterm-256color LANG=C.UTF-8 TZ=UTC \
    HISTFILE="$devbox/.zsh_history"; do
    env_opts+=("environment=\"$var\"")
  done
  (
    IFS=,
    echo "${env_opts[*]} $(cat "$laptop/.ssh/id_ed25519.pub")"
  ) >"$work/sshd/authorized_keys"
  cat >"$work/sshd/config" <<EOF
Port $ssh_port
ListenAddress 127.0.0.1
HostKey $work/sshd/host_key
PidFile $work/sshd/pid
AuthorizedKeysFile $work/sshd/authorized_keys
PermitUserEnvironment yes
PermitRootLogin prohibit-password
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
PrintMotd no
PrintLastLog no
ForceCommand exec zsh -i
EOF
  # sshd as root wants its privilege separation dir, which only the service
  # scripts make.
  [ "$(id -u)" -ne 0 ] || mkdir -p /run/sshd
  "$(command -v sshd || echo /usr/sbin/sshd)" -f "$work/sshd/config" \
    -E "$work/sshd/log"
  cat >"$laptop/.ssh/config" <<EOF
Host devbox
  HostName 127.0.0.1
  Port $ssh_port
  User $(id -un)
  IdentityFile $laptop/.ssh/id_ed25519
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  LogLevel ERROR
EOF

  # Fast mode's "now available" notice takes a row of the demo.
  shell_env+=(ATUIN_HOST_NAME=laptop ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY"
    CLAUDE_CODE_DISABLE_FAST_MODE=1)
}
