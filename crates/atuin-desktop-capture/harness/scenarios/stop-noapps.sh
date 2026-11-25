# Checks `atuin desktop-capture stop` and `start` with apps = [], and the "Stopped" line of status.
#
# With no application selected, status says so, and a stop adds the "Stopped" line, which a
# start takes away. An application that is added while capture is stopped stays uncaptured, and
# the stop survives a restart of the daemon. A start then captures the application.
export ATUIN_CONFIG_DIR=$W/atuin-config ATUIN_DATA_DIR=$W/atuin-data
mkdir -p "$ATUIN_CONFIG_DIR" "$ATUIN_DATA_DIR"
CONFIG=$ATUIN_CONFIG_DIR/config.toml
write_config() {
    daemon_config > "$CONFIG.new"
    printf '[desktop_capture]\nenabled = true\n%s\n' "$1" >> "$CONFIG.new"
    \mv -f "$CONFIG.new" "$CONFIG"
}
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fi; }
start_daemon() {
    "$ATUIN" daemon start >> "$W/daemon.log" 2>&1 &
    DAEMON=$!
    wait_socket || echo "FAIL  the daemon did not start; see $W/daemon.log"
}
stop_daemon() {
    [ -n "${DAEMON:-}" ] || return 0
    "$ATUIN" daemon stop > /dev/null 2>&1
    for _ in $(seq 50); do kill -0 "$DAEMON" 2> /dev/null || break; sleep 0.1; done
    kill "$DAEMON" 2> /dev/null
    DAEMON=
}
trap stop_daemon EXIT
trap 'exit 143' TERM INT
status() { "$ATUIN" desktop-capture status 2>&1; }
STOPPED_LINE='^  Stopped: '

write_config 'apps = []'
start_daemon
# The supervisor reports Starting for a moment after the daemon starts and after each change.
status_is 'no application is selected'
S_NOAPPS=$(status); echo "$S_NOAPPS"
"$ATUIN" desktop-capture stop
status_is "$STOPPED_LINE"
S_NOAPPS_STOPPED=$(status); echo "$S_NOAPPS_STOPPED"
"$ATUIN" desktop-capture start > /dev/null
status_is 'no application is selected'
S_NOAPPS_STARTED=$(status); echo "$S_NOAPPS_STARTED"

echo "#### add gedit while stopped"
"$ATUIN" desktop-capture stop > /dev/null
write_config 'apps = ["gedit"]'
# Status says "stopped" in place of "no application is selected" once the daemon applied the list.
status_is '^Desktop capture: stopped'
echo "Stopped phrase wombat 3317" > "$P/wombat.txt"
gedit "$P/wombat.txt" > /dev/null 2>&1 &
wait_window gedit
sleep 5
can_focus && focus gedit && sleep 3
S_ADDED=$(status); echo "$S_ADDED"
WHILE_STOPPED=$(find_segment wombat 3317)

echo "#### restart the daemon while stopped"
stop_daemon
start_daemon
status_is '^Desktop capture: stopped'
S_RESTARTED=$(status); echo "$S_RESTARTED"
sleep 3
AFTER_RESTART=$(find_segment wombat 3317)

echo "#### start"
"$ATUIN" desktop-capture start > /dev/null
status_is '^Desktop capture: running'
sleep 5
can_focus && focus gedit && sleep 3
S_STARTED=$(status); echo "$S_STARTED"
AFTER_START=$(find_segment wombat 3317)
stop_daemon

echo
check "apps = [] says that no application is selected" 'grep -q "no application is selected" <<< "$S_NOAPPS"'
check "apps = [] without a stop has no Stopped line" '! grep -q "$STOPPED_LINE" <<< "$S_NOAPPS"'
check "a stop with apps = [] adds the Stopped line" 'grep -q "$STOPPED_LINE" <<< "$S_NOAPPS_STOPPED"'
check "a start takes the Stopped line away" '! grep -q "$STOPPED_LINE" <<< "$S_NOAPPS_STARTED"'
check "an app added while stopped leaves capture stopped" 'grep -q "^Desktop capture: stopped" <<< "$S_ADDED"'
check "an app added while stopped is not captured" '[ -z "$WHILE_STOPPED" ]'
check "the stop survives a daemon restart" 'grep -q "^Desktop capture: stopped" <<< "$S_RESTARTED"'
check "nothing is captured after the restart while stopped" '[ -z "$AFTER_RESTART" ]'
check "a start runs capture" 'grep -q "^Desktop capture: running" <<< "$S_STARTED"'
check "a start captures the added app" '[ -n "$AFTER_START" ]'
