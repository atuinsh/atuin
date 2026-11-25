#!/bin/bash
# Runs a scenario in a private desktop session with its own D-Bus session bus.
#
# Usage: crates/atuin-desktop-capture/harness/session.sh <kind> <scenario> [arguments...]
#
# <kind> is one of x11, kwin, kwin-xwayland, mutter, gnome, and sway. <scenario> is the name of
# a file in harness/scenarios, or a path. The scenario's own output is printed at the end, and
# everything else stays in target/harness-run/<kind>: the daemon's settings, data, and log, what
# the daemon stored, and the output of the session. The sockets of the session live in a short
# runtime directory in /tmp, which the run removes at its end.
set -u
HARNESS=$(cd "$(dirname "$0")" && pwd)
# The workspace root, which holds target/.
REPO=$(cd "$HARNESS/../../.." && pwd)
[ $# -ge 2 ] || { sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
KIND=$1
SCENARIO=$2
shift 2
case $SCENARIO in
*/*) ;;
*) SCENARIO=$HARNESS/scenarios/$SCENARIO.sh ;;
esac
[ -f "$SCENARIO" ] || { echo "no scenario $SCENARIO" >&2; exit 2; }
ATUIN=${ATUIN:-$REPO/target/release/atuin}
[ -x "$ATUIN" ] && [ -x "$(dirname "$ATUIN")/atuin-desktop-capture" ] || {
    echo "build atuin and its helper first: cargo build --release -p atuin --features desktop-capture" >&2
    exit 2
}
W=${HARNESS_WORK:-$REPO/target/harness-run}/$KIND
rm -rf "$W"
mkdir -p "$W/home" "$W/pages"
# A socket's path holds at most 107 bytes, so the sockets of sway, VS Code, and the daemon go into
# a runtime directory with a short path, regardless of the path of the work directory.
RUNTIME=$(mktemp -d /tmp/dch.XXXXXX)
ln -s "$RUNTIME" "$W/run"
export HARNESS KIND SCENARIO ATUIN W
# Apps installed with `flatpak --user` live in the account's own home.
export HARNESS_USER_HOME=$HOME
# The session bus starts every service that it activates, such as dconf, with its own
# environment, so the private home has to be in place before the bus starts.
# The cache outlives the session, because a program that rebuilds its font cache takes seconds
# to show its first window.
export HOME=$W/home XDG_RUNTIME_DIR=$RUNTIME XDG_CONFIG_HOME=$W/home/.config \
    XDG_DATA_HOME=$W/home/.local/share XDG_STATE_HOME=$W/home/.local/state \
    XDG_CACHE_HOME=${HARNESS_CACHE:-$REPO/target/harness-cache}
mkdir -p "$XDG_CACHE_HOME"
unset DISPLAY WAYLAND_DISPLAY SWAYSOCK DBUS_SESSION_BUS_ADDRESS AT_SPI_BUS_ADDRESS XDG_SESSION_TYPE \
    XDG_CURRENT_DESKTOP SESSION_MANAGER
# setsid gives the run a session of its own, whose id is the pid of the background job. Programs
# of the run that outlive the scenario, such as the services that the buses started, stay in
# that session, so the run ends by signalling the whole session, even when the run is
# interrupted.
end_session() {
    [ -n "${session:-}" ] || return 0
    pkill -s "$session" 2> /dev/null
    for _ in $(seq 50); do pgrep -s "$session" > /dev/null || break; sleep 0.1; done
    pkill -KILL -s "$session" 2> /dev/null
    session=
}
trap 'end_session; rm -rf "$RUNTIME"' EXIT
trap 'exit 130' INT TERM
setsid -w timeout "${TIMEOUT:-120}" dbus-run-session -- bash "$HARNESS/lib/inner.sh" "$@" \
    > "$W/session.out" 2>&1 &
session=$!
wait "$session"
status=$?
end_session
cat "$W/result.txt" 2>/dev/null
[ $status -eq 124 ] && echo "(the session timed out after ${TIMEOUT:-120} s)"
exit $status
