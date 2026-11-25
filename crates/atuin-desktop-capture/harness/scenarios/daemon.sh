# Runs desktop capture through Atuin's daemon: enables capture, installs the Claude Code hooks,
# starts the daemon, and focuses gedit, Kate, and a WG14 proposal in Firefox. It shows an error
# dialog for 2 s and sends a notification with a login code. Then it checks the log, the status,
# the context that the prompt hook adds, the scrubbed code, and that a stop holds.
#
# Capture is enabled before the hooks are installed, so that the hook install has to add the
# prompt hook itself.
#
# The daemon, its configuration and data, the Claude Code settings, and the hook's state all live
# in the session's work directory. The helper is the atuin-desktop-capture next to $ATUIN. Turning
# capture on through setup also writes the files that start capture at login into the session's
# home, and turns the toolkit-accessibility setting on.
[ -x "$ATUIN" ] || { echo "FAIL  build atuin first: cargo build --release -p atuin"; return; }
[ -x "$(dirname "$ATUIN")/atuin-desktop-capture" ] \
    || { echo "FAIL  build the helper first: cargo build --release -p atuin --features desktop-capture"; return; }
export ATUIN_CONFIG_DIR=$W/atuin-config ATUIN_DATA_DIR=$W/atuin-data CLAUDE_CONFIG_DIR=$HOME/.claude \
    TMPDIR=$W/tmp
mkdir -p "$ATUIN_CONFIG_DIR" "$ATUIN_DATA_DIR" "$TMPDIR"
cat > "$ATUIN_CONFIG_DIR/config.toml" << EOF
[daemon]
enabled = true
autostart = false
socket_path = "$SOCK"
pidfile_path = "$W/atuin.pid"

[logs]
dir = "$W/logs"

[desktop_capture]
apps = ["gedit", "kate", "Firefox", "zenity", "notify-send"]
EOF
cat > "$P/n3254.html" << 'EOF'
<html><head><title>N3254: Clarifying the rehearsal page (WG14 proposal)</title></head><body>
<h1>N3254: Clarifying the rehearsal page</h1>
<p>A local stand-in for a WG14 proposal, for the desktop capture harness.</p>
</body></html>
EOF
run() { echo "\$ atuin $*"; "$ATUIN" "$@" 2>&1; echo "(exit $?)"; }
# Prints the segments that `atuin desktop-capture log` lists, without its header.
segments() { "$ATUIN" desktop-capture log 2> /dev/null | tail -n +2; }
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fi; }
# Stops the daemon, and with it the helper, even when the session times out or a step aborts.
stop_daemon() {
    [ -n "${DAEMON:-}" ] || return 0
    "$ATUIN" daemon stop > /dev/null 2>&1
    for _ in $(seq 50); do kill -0 "$DAEMON" 2> /dev/null || break; sleep 0.1; done
    kill "$DAEMON" 2> /dev/null
    DAEMON=
}
trap stop_daemon EXIT
trap 'exit 143' TERM INT

[ "$KIND" = gnome ] || dunst > /dev/null 2>&1 &
# A first start of zenity fills the caches, so that the dialog below shows at once.
timeout 3 zenity --info --text=warming > /dev/null 2>&1
ENABLE=$(run setup --only --desktop-capture --daemon)
echo "$ENABLE"
run setup --only --agent-history --agents claude-code
"$ATUIN" daemon start > "$W/daemon.log" 2>&1 &
DAEMON=$!
wait_socket || echo "FAIL  the daemon did not start; see $W/daemon.log"
sleep 2
gedit "$P/rich.html" > /dev/null 2>&1 &
sleep 5
kate "$P/tab2.html" > /dev/null 2>&1 &
sleep 6
firefox_ "file://$P/n3254.html"
sleep 10
if can_focus; then
    for app in gedit kate firefox; do
        focus "$app"
        sleep 5
    done
fi
echo "#### an error dialog for 2 s, then a notification with a login code"
zenity --error --text "Copy failed: no space left on device" > /dev/null 2>&1 &
ALERT=$!
wait_window zenity
sleep 2
kill $ALERT
sleep 2
notify-send "Sign-in" "Your login code is 482913"
sleep 3
echo "\$ atuin desktop-capture status"
STATUS=$("$ATUIN" desktop-capture status 2>&1)
echo "$STATUS"
run desktop-capture log

# The payload that Claude Code sends when the user submits a prompt, piped into the command that
# Claude Code's settings now hold for that event.
HOOK=$($PY -c 'import json, sys; print(json.load(open(sys.argv[1]))["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"])' \
    "$CLAUDE_CONFIG_DIR/settings.json")
echo "UserPromptSubmit hook: $HOOK"
cat > "$W/prompt.json" << EOF
{"session_id": "harness-$KIND-$$", "transcript_path": "$W/transcript.jsonl", "cwd": "$W",
 "permission_mode": "default", "hook_event_name": "UserPromptSubmit",
 "prompt": "does my compiler have any issues related to n3254?"}
EOF
sh -c "$HOOK" < "$W/prompt.json" > "$W/hook.out" 2> "$W/hook.err"
echo "hook exit $?"
CONTEXT=$($PY -c 'import json, sys; print(json.load(open(sys.argv[1]))["hookSpecificOutput"]["additionalContext"])' \
    "$W/hook.out" 2> /dev/null)
echo "additionalContext:"
echo "$CONTEXT"
echo "\$ atuin desktop-capture log --kind notification"
LISTED=$("$ATUIN" desktop-capture log --kind notification 2>&1)
echo "$LISTED"
NOTIFICATION_ID=$(sed -nE 's/^([0-9a-f-]{36}) .*/\1/p' <<< "$LISTED" | head -1)
echo "\$ atuin desktop-capture show $NOTIFICATION_ID"
NOTIFICATION_TEXT=$("$ATUIN" desktop-capture show "${NOTIFICATION_ID:-none}" 2>&1)
echo "$NOTIFICATION_TEXT"

echo "#### stop, then change the focus"
run desktop-capture stop
BEFORE=$(segments | grep -c .)
if can_focus; then
    focus gedit
    sleep 2
    focus kate
fi
sleep 5
SEGMENTS=$(segments)
AFTER=$(grep -c . <<< "$SEGMENTS")
echo "segments before: $BEFORE, after: $AFTER"
run daemon stop
wait "$DAEMON" 2> /dev/null
DAEMON=

echo
check "enabling capture asks for no restart" '! grep -q Restart <<< "$ENABLE"'
check "the prompt hook is installed" '[ -n "$HOOK" ]'
check "gedit gets a segment" 'grep -qE "^[0-9a-f-]{36} +[0-9.]+ active +gedit +[0-9]+ +rich.html" <<< "$SEGMENTS"'
check "Kate gets a segment" 'grep -qE "^[0-9a-f-]{36} +[0-9.]+ active +kate +[0-9]+ +tab2.html" <<< "$SEGMENTS"'
check "Firefox gets a segment" 'grep -qE "^[0-9a-f-]{36} +[0-9.]+ active +Firefox +[0-9]+ +N3254: " <<< "$SEGMENTS"'
check "status is running" 'grep -q "^Desktop capture: running" <<< "$STATUS"'
check "recent activity names the N3254 page" 'grep -q "Firefox — N3254: Clarifying the rehearsal page (WG14 proposal)" <<< "$CONTEXT"'
check "recent activity names Kate" 'grep -q "kate — tab2.html" <<< "$CONTEXT"'
check "the dialog reaches the hook" 'grep -q "dialog in zenity" <<< "$CONTEXT"'
if [ "$KIND" = mutter ]; then
    echo "skip  the login code is scrubbed: mutter has no notification service"
else
    check "the login code is scrubbed" \
        'grep -q "\[one-time code\]" <<< "$NOTIFICATION_TEXT" && ! grep -q 482913 <<< "$NOTIFICATION_TEXT"'
fi
if can_focus; then
    check "stop holds" '[ "$BEFORE" -gt 0 ] && [ "$BEFORE" -eq "$AFTER" ]'
else
    echo "skip  stop holds: this session cannot change the focus"
fi
