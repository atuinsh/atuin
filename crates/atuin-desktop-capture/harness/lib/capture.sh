# Runs desktop capture through a private Atuin daemon, for the Linux and the macOS harness.
#
# Needs W, the work directory, ATUIN, the `atuin` binary, PY, Python 3, and T, the harness's tools.
# HELPER, when set, names the helper that the daemon runs. Otherwise the daemon runs the helper
# next to $ATUIN. The daemon's settings, data, and logs live in $W. Its socket is $SOCK, which
# lies in $XDG_RUNTIME_DIR when that is set, because the path of a socket must stay short.
#
#   capture_start [lines]           starts the daemon with desktop capture, adds the lines to
#                                   its [desktop_capture] settings, such as 'apps = "all"', and
#                                   waits until capture is running. When capture doesn't start, it
#                                   prints a FAIL line and returns 1, so scenarios call it as
#                                   `capture_start ... || return`.
#   capture_stop                    stops capture and the daemon, and saves what the daemon
#                                   stored for `records`
#   mark <text>                     notes the time of a mark for `records`
#   records [app]                   after capture_stop, prints the segments, snapshots,
#                                   and marks in time order, only of the applications
#                                   whose names match the regular expression app when it
#                                   is given. tools/records.py describes the lines.
#   segment_id <regex>              after capture_stop, prints the id of the first segment
#                                   whose "<app> — <title>" matches
#   find_segment <search arguments>  prints the segment of the best hit of
#                                   `atuin desktop-capture search`, also when capture is running
#   daemon_config                   prints the [daemon] and [logs] settings that keep the daemon
#                                   in $W, for scenarios that write a configuration of their own
#   wait_socket                     waits until the daemon $DAEMON answers on $SOCK, and returns 1
#                                   when the daemon exits or doesn't answer in time
#   status_is <regex>               waits until a line of `atuin desktop-capture status` matches,
#                                   and returns 1 when no line does in time

export ATUIN_CONFIG_DIR=$W/atuin-config ATUIN_DATA_DIR=$W/atuin-data
mkdir -p "$ATUIN_CONFIG_DIR" "$ATUIN_DATA_DIR"
SOCK=${XDG_RUNTIME_DIR:-$W}/atuin.sock
# The waits end as soon as their condition holds. Their bound is long, because a loaded machine
# can take tens of seconds to start the daemon and the helper.
WAIT_SECONDS=90
daemon_config() {
    printf '[daemon]\nenabled = true\nautostart = false\nsocket_path = "%s"\n' "$SOCK"
    printf 'pidfile_path = "%s"\n\n[logs]\ndir = "%s"\n\n' "$W/atuin.pid" "$W/logs"
}
wait_socket() {
    local deadline=$((SECONDS + WAIT_SECONDS))
    while [ $SECONDS -lt $deadline ]; do
        # The socket's file appears before the daemon listens on it, so the wait asks the
        # daemon for an answer.
        "$ATUIN" desktop-capture status > /dev/null 2>&1 && return 0
        kill -0 "$DAEMON" 2> /dev/null || return 1
        sleep 0.1
    done
    return 1
}
status_is() {
    local deadline=$((SECONDS + WAIT_SECONDS))
    while [ $SECONDS -lt $deadline ]; do
        "$ATUIN" desktop-capture status 2>&1 | grep -qE "$1" && return 0
        sleep 0.1
    done
    return 1
}
capture_start() {
    {
        daemon_config
        printf '[desktop_capture]\nenabled = true\n'
        [ -n "${HELPER:-}" ] && printf 'helper_path = "%s"\n' "$HELPER"
        printf '%s\n' "$@"
    } > "$ATUIN_CONFIG_DIR/config.toml"
    "$ATUIN" daemon start >> "$W/daemon.log" 2>&1 &
    DAEMON=$!
    if ! wait_socket; then
        echo "FAIL  the daemon did not start; see $W/daemon.log"
        return 1
    fi
    # An earlier capture_stop leaves capture stopped across daemon restarts.
    "$ATUIN" desktop-capture start > /dev/null 2>&1
    if ! status_is '^Desktop capture: running'; then
        echo "FAIL  desktop capture did not start; see $W/daemon.log and $W/logs"
        return 1
    fi
}
capture_stop() {
    "$ATUIN" desktop-capture stop >> "$W/daemon.log" 2>&1
    "$ATUIN" desktop-capture log --limit 1000000 --style ndjson > "$W/segments.ndjson"
    $PY -c 'import json, sys; [print(json.loads(line)["segment"]) for line in sys.stdin]' \
        < "$W/segments.ndjson" | while read -r id; do
        "$ATUIN" desktop-capture snapshots "$id" --style ndjson
    done > "$W/snapshots.ndjson"
    "$ATUIN" daemon stop >> "$W/daemon.log" 2>&1
    wait "$DAEMON" 2> /dev/null
    DAEMON=
}
mark() { echo "$($PY -c 'import time; print(time.time_ns() // 1_000_000)') $*" >> "$W/marks"; }
records() { $PY "$T/records.py" "$W" "$@"; }
segment_id() { records | sed -nE "s/^[a-z]+ ([0-9a-f-]{36}): ($1).*/\1/p" | head -1; }
find_segment() {
    "$ATUIN" desktop-capture search --limit 1 --style ndjson "$@" 2> /dev/null \
        | $PY -c 'import json, sys; line = sys.stdin.readline(); print(json.loads(line)["segment"] if line else "")'
}
