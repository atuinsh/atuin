#!/bin/bash
# Runs a capture scenario on the Mac through a private Atuin daemon. Run it on the Mac, in the
# account that has the Accessibility permission for the helper, after build.sh.
#
# Usage: run.sh <scenario> [arguments]
#
#   launch-after             TwoWin starts after capture; its edits must be recorded
#   launch-before            TwoWin starts before capture
#   background front|behind  TwoWin's background window changes every 3 s, with TwoWin in
#                            front or behind TextEdit
#   spaces <mode> <front>    TwoWin with mode normal, minimize, minimize-all, or fullscreen, and
#                            twowin or textedit in front
#   dialogs                  alerts from a process without other windows, open 300 ms and 1.5 s
#   safari                   Safari with a long page and a second tab: the first snapshot must
#                            hold the whole page
#   idle <app> <url...>      a static page in a browser for 60 s, without polling
#   private                  a normal and a private window in Safari, Firefox, Chrome, Edge,
#                            and Brave, which runs in German: capture records the private
#                            windows only with `private_windows = "capture"`
#   terminals                a web terminal in the shape of xterm.js in Chrome and Safari, and
#                            the real xterm.js when XTERM names a directory with its
#                            lib/xterm.js and css/xterm.css: capture keeps the page's text and
#                            doesn't store any of the terminal's marker
#
# ATUIN names the `atuin` binary, which is the `atuin` on PATH by default. HELPER names the
# helper that the daemon runs. HELPER must hold the grant itself, because the daemon runs it as
# it is. By default, HELPER is the signed copy that `atuin setup` made in
# ~/.atuin/desktop-capture, which holds the grant, and otherwise the atuin-desktop-capture next
# to ATUIN. axdrive must be on PATH for `spaces` and `private`. The test pages go to
# /tmp/desktop-capture-pages. The daemon's Atuin home, settings, data, and logs, and what the
# daemon stored, go to /tmp/desktop-capture-<scenario>.
#
# The scenarios quit Safari and TwoWin. `private` also sets Brave's language for the run, and
# puts the old language back when the run ends.
set -u
[ $# -ge 1 ] || { sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
SCENARIO=$1
shift
HARNESS=$(cd "$(dirname "$0")/.." && pwd)
ATUIN=${ATUIN:-$(command -v atuin)}
[ -n "$ATUIN" ] || { echo "no atuin on PATH: set ATUIN" >&2; exit 2; }
SIGNED=$HOME/.atuin/desktop-capture
if [ -z "${HELPER:-}" ]; then
    if [ -f "$SIGNED/signing.name" ]; then
        HELPER=$SIGNED/$(cat "$SIGNED/signing.name")
    elif [ -x "$SIGNED/atuin-desktop-capture" ]; then
        HELPER=$SIGNED/atuin-desktop-capture
    else
        HELPER=$(dirname "$ATUIN")/atuin-desktop-capture
    fi
fi
PY=/usr/bin/python3
T=$HARNESS/tools
W=/tmp/desktop-capture-$SCENARIO
PAGES=/tmp/desktop-capture-pages
NOTE=/tmp/desktop-capture-note.txt
rm -rf "$W" "$PAGES"
mkdir -p "$W" "$PAGES"
$PY "$HARNESS/pages/make.py" "$PAGES"
cp "$HARNESS"/pages/*.html "$PAGES/"
printf 'A note in TextEdit\n' > "$NOTE"
# The daemon's own Atuin home keeps the daemon away from the account's signing certificate, which
# the daemon would otherwise use to sign a copy of HELPER, and from the account's logs.
export ATUIN_HOME=$W/atuin-home
source "$HARNESS/lib/capture.sh"
# Each scenario adds the commands that undo what it changed outside $W.
undo=()
finish() {
    [ -n "${DAEMON:-}" ] && "$ATUIN" daemon stop > /dev/null 2>&1
    local command
    for command in ${undo[@]+"${undo[@]}"}; do eval "$command"; done
    undo=()
}
trap finish EXIT
trap 'exit 130' INT TERM HUP

case $SCENARIO in
launch-after)
    pkill -x TwoWin; sleep 1
    capture_start 'apps = ["TwoWin"]' 'poll_interval = "0s"' || exit 1; sleep 2
    open ~/TwoWin.app; sleep 30
    capture_stop; pkill -x TwoWin
    records
    echo
    records | grep -qE '^segment [0-9a-f-]+: (edited|restructured) snapshot' \
        && echo "ok    TwoWin's edits are recorded" || echo "FAIL  TwoWin's edits are recorded"
    ;;
launch-before)
    pkill -x TwoWin; sleep 1
    open ~/TwoWin.app; sleep 3
    capture_start 'apps = ["TwoWin"]' 'poll_interval = "0s"' || exit 1; sleep 30
    capture_stop; pkill -x TwoWin
    records
    ;;
background)
    pkill -x TwoWin; sleep 1
    open ~/TwoWin.app; sleep 3
    [ "${1:-front}" = behind ] && { open -a TextEdit "$NOTE"; sleep 2; }
    capture_start 'apps = "all"' || exit 1; sleep 40
    capture_stop; pkill -x TwoWin
    records TwoWin
    ;;
spaces)
    pkill -x TwoWin; sleep 2
    open ~/TwoWin.app --args "${1:-normal}"; sleep 7
    [ "${2:-twowin}" = textedit ] && { open -a TextEdit "$NOTE"; sleep 2; }
    for window in Front Background; do
        axdrive attrs TwoWin "$window" 2>&1 | grep -E '^(AXTitle|AXMinimized|AXFullScreen) =|no element'
    done
    capture_start 'apps = "all"' 'poll_interval = "0s"' || exit 1; sleep 35
    capture_stop; pkill -x TwoWin
    records TwoWin
    ;;
dialogs)
    capture_start 'apps = "all"' 'poll_interval = "0s"' || exit 1; sleep 3
    open -n ~/Alone.app --args 300; sleep 4
    open -n ~/Alone.app --args 1500; sleep 5
    capture_stop
    records Alone
    ;;
safari)
    pkill -x Safari; sleep 2
    open -a Safari "file://$PAGES/tab2.html"; sleep 3
    open -a Safari "file://$PAGES/long.html"; sleep 10
    capture_start 'apps = "all"' 'poll_interval = "0s"' || exit 1; sleep 20
    capture_stop
    records Safari | grep -v '^background '
    ;;
idle)
    app=$1
    shift
    for url in "$@"; do open -a "$app" "$url"; sleep 3; done
    sleep 5
    capture_start 'apps = "all"' 'poll_interval = "0s"' || exit 1; sleep 60
    capture_stop
    records "$app" | grep -v '^background ' | head -20
    echo "unchanged reads: $(records "$app" | grep -c ', unchanged$')"
    ;;
private)
    # The browsers other than Safari run with profiles of their own, so that they leave the
    # account's own browser windows alone. Brave takes its language from its defaults, which
    # the scenario sets for the run. A fresh Firefox profile opens an onboarding modal that
    # hides the window's toolbars, and capture can't check a window without them, so the
    # profile's user.js turns onboarding off.
    profiles=$(mktemp -d /tmp/desktop-capture-private.XXXXXX)
    undo+=('pkill -f -- "$profiles"; pkill -x Safari; sleep 2; rm -rf "$profiles"')
    # The browsers live in /Applications or in the account's own ~/Applications.
    app_exe() { # application executable
        local dir
        for dir in /Applications "$HOME/Applications"; do
            [ -x "$dir/$1.app/Contents/MacOS/$2" ] && { echo "$dir/$1.app/Contents/MacOS/$2"; return; }
        done
        echo "$1 isn't installed in /Applications or ~/Applications" >&2
        return 1
    }
    chromium_both() { # application private-flag
        local exe profile="$profiles/$1"
        exe=$(app_exe "$1" "$1") || exit 1
        "$exe" --user-data-dir="$profile" --no-first-run --no-default-browser-check \
            "file://$PAGES/long.html" > /dev/null 2>&1 &
        sleep 10
        "$exe" --user-data-dir="$profile" "$2" "file://$PAGES/rich.html" > /dev/null 2>&1 &
        sleep 8
    }
    pkill -x Safari; sleep 2
    open -a Safari "file://$PAGES/long.html"; sleep 4
    axdrive key Safari shift+cmd+n > /dev/null; sleep 2
    open -a Safari "file://$PAGES/rich.html"; sleep 4
    firefox=$(app_exe Firefox firefox) || exit 1
    mkdir -p "$profiles/firefox"
    cp "$HARNESS/macos/firefox-user.js" "$profiles/firefox/user.js"
    "$firefox" -profile "$profiles/firefox" -no-remote "file://$PAGES/long.html" > /dev/null 2>&1 &
    sleep 8
    "$firefox" -profile "$profiles/firefox" -private-window "file://$PAGES/rich.html" \
        > /dev/null 2>&1 &
    sleep 6
    chromium_both "Google Chrome" --incognito
    chromium_both "Microsoft Edge" --inprivate
    brave_languages=$(defaults read com.brave.Browser AppleLanguages 2> /dev/null)
    if [ -n "$brave_languages" ]; then
        undo+=('defaults write com.brave.Browser AppleLanguages "$brave_languages"')
    else
        undo+=('defaults delete com.brave.Browser AppleLanguages 2> /dev/null')
    fi
    defaults write com.brave.Browser AppleLanguages '(de)'
    chromium_both "Brave Browser" --incognito
    capture_start 'apps = "all"' 'poll_interval = "0s"' || exit 1; sleep 40
    capture_stop
    records > "$W/excluded.txt"
    # The mark comes before the start, because the sweep can start a segment before
    # capture_start returns.
    mark "private windows are captured"
    capture_start 'apps = "all"' 'poll_interval = "0s"' 'private_windows = "capture"' || exit 1
    sleep 60
    capture_stop
    records | sed -n '/^== private windows are captured/,$p' > "$W/captured.txt"
    finish
    grep -v snapshot "$W/excluded.txt"
    echo "== the second round"
    grep -v snapshot "$W/captured.txt"
    echo
    # Capture names the applications as they name themselves.
    for app in Safari Firefox Chrome Edge Brave; do
        grep -qE ": $app — Reference manual" "$W/excluded.txt" \
            && echo "ok    the normal $app window is captured" \
            || echo "FAIL  the normal $app window is captured"
        grep -qE ": $app — Rich page" "$W/excluded.txt" \
            && echo "FAIL  the private $app window isn't recorded" \
            || echo "ok    the private $app window isn't recorded"
        grep -qE ": $app — Rich page" "$W/captured.txt" \
            && echo "ok    the private $app window is captured on request" \
            || echo "FAIL  the private $app window is captured on request"
    done
    grep -qE 'Rich page|Privat' "$W/excluded.txt" \
        && echo "FAIL  no record names a private window" || echo "ok    no record names a private window"
    grep -q 'Brave (Privat)' "$W/captured.txt" \
        && echo "ok    Brave titles its private window in German" \
        || echo "FAIL  Brave titles its private window in German"
    ;;
terminals)
    # Chrome runs with a profile of its own, so that it leaves the account's own windows alone.
    profile=$(mktemp -d /tmp/desktop-capture-terminals.XXXXXX)
    undo+=('pkill -f -- "$profile"; pkill -x Safari; sleep 2; rm -rf "$profile"')
    chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    [ -x "$chrome" ] || chrome="$HOME/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    pkill -x Safari; sleep 2
    capture_start 'apps = "all"' 'poll_interval = "0s"' || exit 1; sleep 2
    "$chrome" --user-data-dir="$profile" --no-first-run --no-default-browser-check \
        "file://$PAGES/terminal.html" > /dev/null 2>&1 &
    sleep 12
    open -a Safari "file://$PAGES/terminal.html"; sleep 12
    if [ -n "${XTERM:-}" ]; then
        cp -r "$XTERM" "$PAGES/xterm"
        "$chrome" --user-data-dir="$profile" --new-window "file://$PAGES/xterm.html" \
            > /dev/null 2>&1 &
        sleep 12
        open -a Safari "file://$PAGES/xterm.html"; sleep 12
    fi
    texts=$("$ATUIN" desktop-capture log --limit 1000 --style ndjson 2> /dev/null \
        | $PY -c 'import json, sys; [print(json.loads(line)["segment"]) for line in sys.stdin]' \
        | while read -r id; do "$ATUIN" desktop-capture show "$id" 2> /dev/null; done)
    hits=$("$ATUIN" desktop-capture search --style ndjson TERMPROBE 2> /dev/null)
    capture_stop
    finish
    echo "$texts" > "$W/texts.txt"
    records | grep -v unchanged
    echo "$texts" | grep -E 'KEEPME|terminal' | sort | uniq -c
    echo
    for app in Chrome Safari; do
        grep -qE ": $app — Cloud shell" <<< "$(records)" \
            && echo "ok    $app gets a segment" || echo "FAIL  $app gets a segment"
        if [ -n "${XTERM:-}" ]; then
            grep -qE ": $app — xterm.js terminal" <<< "$(records)" \
                && echo "ok    $app reads the xterm.js page" \
                || echo "FAIL  $app reads the xterm.js page"
        fi
    done
    for text in KEEPME-777 KEEPME-888 '[terminal]'; do
        grep -qF "$text" <<< "$texts" && echo "ok    the texts hold $text" \
            || echo "FAIL  the texts hold $text"
    done
    grep -qF TERMPROBE <<< "$texts" && echo "FAIL  no stored text holds the terminal's marker" \
        || echo "ok    no stored text holds the terminal's marker"
    [ -z "$hits" ] && echo "ok    a search for the marker finds nothing" \
        || echo "FAIL  a search for the marker finds nothing"
    grep -rqaF TERMPROBE "$ATUIN_DATA_DIR" && echo "FAIL  the data directory holds no marker" \
        || echo "ok    the data directory holds no marker"
    ;;
*)
    echo "unknown scenario: $SCENARIO" >&2
    exit 2
    ;;
esac
