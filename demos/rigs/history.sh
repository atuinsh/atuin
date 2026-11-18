# shellcheck shell=bash
# Sourced by render.sh for history.tape: the laptop, with Claude Code
# connected to atuin's MCP server, over the fixture history start.tape
# imports. Claude is real, on ANTHROPIC_API_KEY.

# shellcheck source=common.sh disable=SC1091
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

rig_up() {
  local home=$1/laptop
  setup_claude_with_mcp "$home" "$2"
  env -i HOME="$home" PATH="$home/bin:/usr/bin:/bin" \
    atuin mcp install claude-code >/dev/null 2>&1
}
