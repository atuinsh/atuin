# shellcheck shell=bash
# What the rigs share. Sourced by them, inside render.sh.

# A git checkout of fixtures/repo at $1.
make_repo() {
  mkdir -p "$(dirname "$1")"
  cp -R fixtures/repo "$1"
  git -C "$1" init -q -b main
  git -C "$1" add -A
  git -C "$1" -c user.name=demo -c user.email=demo@example.com \
    commit -q -m 'Initial commit'
}

# Claude Code in HOME $1 without its first-run screens: onboarding, the auto
# mode notice, the API key prompt (it asks about the key's last 20
# characters), and trust for the folder $2. $3 is the theme.
configure_claude() {
  cat >"$1/.claude.json" <<JSON
{
  "hasCompletedOnboarding": true,
  "hasSeenAutoDefaultNotice": true,
  "theme": "$3",
  "customApiKeyResponses": { "approved": ["${ANTHROPIC_API_KEY: -20}"], "rejected": [] },
  "projects": { "$2": { "hasTrustDialogAccepted": true } }
}
JSON
}

# The laptop in HOME $1 (theme $2) with a checkout at ~/src/atuin, and
# Claude Code, which calls atuin's MCP tools without asking. atuin mcp
# install is left to the caller, as it must run before any daemon starts.
#   CLAUDE_BIN  (default: claude on PATH)
setup_claude_with_mcp() {
  local home=$1 claude_bin
  : "${ANTHROPIC_API_KEY:?this demo runs claude, which needs it}"
  claude_bin=$(command -v "${CLAUDE_BIN:-claude}")
  cp -R fixtures/laptop/. "$home/"
  make_repo "$home/src/atuin"
  ln -s "$claude_bin" "$home/bin/claude"
  configure_claude "$home" "$home/src/atuin" "$2"
  mkdir -p "$home/.claude"
  echo '{ "permissions": { "allow": ["mcp__atuin"] } }' >"$home/.claude/settings.json"
  shell_env+=(ATUIN_HOST_NAME=laptop ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY"
    CLAUDE_CODE_DISABLE_FAST_MODE=1)
}

# Turns on thread capture and the daemon in HOME $1, its socket in that HOME.
# Any more lines for the [daemon] table follow on stdin.
configure_daemon() {
  cat >>"$1/.config/atuin/config.toml" <<TOML

[threads]
capture = true

[daemon]
enabled = true
socket_path = "$1/d.sock"
TOML
  cat >>"$1/.config/atuin/config.toml"
}

# Starts the daemon under the environment in "$@", logging to $work/<host>-daemon.log.
start_daemon() {
  local log=$1
  shift
  env -i "$@" atuin daemon start >"$log" 2>&1 &
  echo $! >>"$work/pids"
}

# Polls the command in "$@" until it succeeds, for up to 10s, then prints the
# log $2 and fails, naming what it waited for ($1).
wait_for() {
  local what=$1 log=$2 tries=0
  shift 2
  until "$@"; do
    tries=$((tries + 1))
    if [ "$tries" -gt 100 ]; then
      echo "Timed out waiting for $what. $log:"
      cat "$log"
      return 1
    fi
    sleep 0.1
  done
}

rig_down() {
  local work=$1
  [ ! -f "$work/sshd/pid" ] || kill "$(cat "$work/sshd/pid")" 2>/dev/null || true
  # shellcheck disable=SC2046 # one pid per line
  [ ! -f "$work/pids" ] || kill $(cat "$work/pids") 2>/dev/null || true
  # And whatever the agents left running, as Codex's app-server outlives
  # codex.
  pkill -f "$work/" 2>/dev/null || true
  wait 2>/dev/null || true
}
