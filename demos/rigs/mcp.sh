# shellcheck shell=bash
# Sourced by render.sh for mcp.tape: one laptop with three past threads, from
# Codex, Claude Code and opencode (fixtures/threads/), and Claude Code
# connected to atuin's MCP server. Claude is real, on ANTHROPIC_API_KEY.

# shellcheck source=common.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# Prints the ISO 8601 time $1 seconds ago, with GNU or BSD date.
ago() {
  local at=$(($(date +%s) - $1))
  date -u -d "@$at" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -r "$at" +%Y-%m-%dT%H:%M:%SZ
}

rig_up() {
  local work=$1 theme=$2 home=$1/laptop
  local repo=$home/src/atuin env tsv harness i=0

  setup_claude_with_mcp "$home" "$theme"
  configure_daemon "$home" </dev/null
  env=(HOME="$home" PATH="$home/bin:/usr/bin:/bin" ATUIN_HOST_NAME=laptop)
  # Before the daemon, which would race it to make the database.
  env -i "${env[@]}" atuin mcp install claude-code >/dev/null 2>&1
  start_daemon "$work/daemon.log" "${env[@]}"
  wait_for "the daemon" "$work/daemon.log" test -S "$home/d.sock"

  for tsv in fixtures/threads/*.tsv; do
    harness=$(basename "$tsv" .tsv)
    i=$((i + 1))
    while IFS=$'\t' read -r seconds message; do
      message=${message//TIME/$(ago "$seconds")}
      echo "${message//CWD/$repo}"
    done <"$tsv" | env -i "${env[@]}" atuin threads push --harness "$harness" \
      --session "00000000-0000-4000-8000-00000000000$i" >/dev/null
  done

  # The daemon indexes what was pushed; wait for all of it.
  # shellcheck disable=SC2016 # eval expands it, each try
  wait_for "the pushed threads" "$work/daemon.log" \
    eval '[ "$(env -i "${env[@]}" atuin threads list 2>/dev/null | wc -l)" -gt 3 ]'

}
