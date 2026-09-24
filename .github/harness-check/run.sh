#!/usr/bin/env bash
# run.sh <id> <npm package>@<version>, e.g. run.sh codex @openai/codex@0.156.1: install that
# harness release into a throwaway HOME, drive two prompts (plain, and one shell tool call)
# against mock.py with no network but loopback, then check what it wrote: new record shapes vs
# <id>.shapes, and the parser via the harness_check example.
# Needs npm, python3, unshare (user namespaces) and a built harness_check example.
# SEED=1 rewrites <id>.shapes from this run (after a human has checked the parser handles it).
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd)
id=$1 release=$2
check=$(realpath "${CHECK_BIN:-target/debug/examples/harness_check}")
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT

npm install --silent --no-audit --no-fund --prefix "$w/npm" "$release"
export HOME=$w/home PATH=$w/npm/node_modules/.bin:$PATH
unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME CLAUDE_CONFIG_DIR CODEX_HOME \
  ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN OPENAI_API_KEY OPENCODE_API_KEY PI_CODING_AGENT_DIR
mkdir -p "$HOME/work"
cd "$HOME/work"
url=http://127.0.0.1:18555

case $id in
claude-code)
  harness=claude-code data=$HOME/.claude/projects
  export ANTHROPIC_BASE_URL=$url ANTHROPIC_API_KEY=dummy CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
  cmd="claude --allowedTools=Bash -p"
  ;;
codex)
  harness=codex data=$HOME/.codex/sessions
  mkdir -p "$HOME/.codex"
  cat >"$HOME/.codex/config.toml" <<EOF
model = "mock-model"
model_provider = "mock"
approval_policy = "never"
sandbox_mode = "danger-full-access"
[model_providers.mock]
name = "Mock"
base_url = "$url/v1"
env_key = "MOCK_API_KEY"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
EOF
  export MOCK_API_KEY=dummy
  cmd="codex exec --skip-git-repo-check"
  ;;
opencode)
  harness=opencode data=$HOME/.local/share/opencode
  mkdir -p "$HOME/.config/opencode"
  cat >"$HOME/.config/opencode/opencode.json" <<EOF
{"model": "mock/mock-model", "small_model": "mock/mock-model", "share": "disabled", "autoupdate": false,
 "permission": {"bash": "allow"}, "disabled_providers": ["opencode"],
 "provider": {"mock": {"npm": "@ai-sdk/openai-compatible", "name": "Mock",
   "options": {"baseURL": "$url/v1", "apiKey": "dummy"}, "models": {"mock-model": {"name": "Mock"}}}}}
EOF
  export OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1 OPENCODE_DISABLE_LSP_DOWNLOAD=1
  cmd="opencode run"
  ;;
opencode-v2)
  harness=opencode data=$HOME/.local/share/opencode
  mkdir -p "$HOME/.config/opencode"
  echo '{}' >"$HOME/models.json"
  cat >"$HOME/.config/opencode/opencode.json" <<EOF
{"update": "disable", "model": "mock/mock-model",
 "providers": {"mock": {"name": "Mock", "package": "aisdk:@ai-sdk/openai-compatible",
   "settings": {"apiKey": "dummy", "baseURL": "$url/v1"},
   "models": {"mock-model": {"name": "Mock", "capabilities": {"tools": true, "input": ["text"], "output": ["text"]},
     "limit": {"context": 100000, "output": 4096}}}}}}
EOF
  export OPENCODE_DISABLE_AUTOUPDATE=true OPENCODE_DISABLE_MODELS_FETCH=true OPENCODE_MODELS_PATH=$HOME/models.json
  cmd="opencode run"
  ;;
pi)
  harness=pi data=$HOME/.pi/agent/sessions
  mkdir -p "$HOME/.pi/agent"
  cat >"$HOME/.pi/agent/models.json" <<EOF
{"providers": {"mock": {"baseUrl": "$url/v1", "api": "openai-completions", "apiKey": "dummy",
  "models": [{"id": "mock-model", "contextWindow": 100000, "maxTokens": 4096}]}}}
EOF
  echo '{"defaultProvider": "mock", "defaultModel": "mock-model"}' >"$HOME/.pi/agent/settings.json"
  export PI_OFFLINE=1 PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0
  cmd="pi -p"
  ;;
*) echo "unknown harness id: $id" >&2; exit 2 ;;
esac

# Loopback only: the harness can reach the mock and nothing else. $2 is the unquoted command.
# shellcheck disable=SC2016
unshare -rn bash -euc '
  ip link set lo up
  python3 "$1" 18555 & mock=$!
  trap "kill $mock" EXIT
  sleep 1
  timeout 120 $2 "say hello" || echo "harness exited $?"
  timeout 120 $2 "SHELL: run a command" || echo "harness exited $?"
' _ "$here/mock.py" "$cmd" </dev/null 2>&1 | tail -n 20

echo "--- shapes not in $id.shapes (fail) / missing from this run (+/-):"
python3 "$here/shapes.py" "$data" >"$w/shapes"
if [ -n "${SEED:-}" ]; then cp "$w/shapes" "$here/$id.shapes"; fi
new=$(comm -13 "$here/$id.shapes" "$w/shapes")
comm -3 "$here/$id.shapes" "$w/shapes" | sed -e 's/^\t/+ /' -e 't' -e 's/^/- /'
echo "--- parser:"
status=0
"$check" "$harness" || status=1
if [ -n "$new" ]; then status=1; fi
exit $status
