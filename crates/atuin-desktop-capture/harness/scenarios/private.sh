# Opens a normal and a private window in Firefox and Chromium, and in Edge and Brave when EDGE
# and BRAVE name their executables. Brave runs in German, which titles its private window
# "(Privat)". By default, capture records the normal windows and doesn't record anything of the
# private ones, including their titles. With `private_windows = "capture"`, capture records
# the private windows too. The scenario takes longer than the default TIMEOUT.
#
# Each Chromium-based browser keeps one profile for both of its windows, so that the private
# window opens in the browser that is already running.
browser() { # executable profile language arguments...
    local executable=$1 profile=$2 language=$3
    shift 3
    LANGUAGE=$language "$executable" --no-sandbox --ozone-platform=$CHROMIUM_PLATFORM \
        --user-data-dir="$HOME/$profile" --no-first-run --no-default-browser-check \
        --password-store=basic --lang="$language" "$@" > /dev/null 2>&1 &
}
both_windows() { # executable profile language private-flag
    browser "$1" "$2" "$3" "file://$P/long.html"
    sleep 10
    browser "$1" "$2" "$3" "$4" "file://$P/rich.html"
    sleep 8
}

capture_start 'apps = "all"' || return
firefox_ "file://$P/chat.html"
sleep 8
firefox_more --private-window "file://$P/tab2.html"
sleep 5
both_windows chromium chromium en-US --incognito
[ -n "${EDGE:-}" ] && both_windows "$EDGE" edge en-US --inprivate
[ -n "${BRAVE:-}" ] && both_windows "$BRAVE" brave de --incognito
can_focus && focus firefox
sleep 6
capture_stop
records > "$W/excluded.txt"

# The mark comes before the start, because the sweep can start a segment before capture_start
# returns.
mark "private windows are captured"
capture_start 'apps = "all"' 'private_windows = "capture"' || return
# Capture reads every window in the background when it starts, one window at a time, and
# the large pages take a while.
sleep 45
capture_stop
records | sed -n '/^== private windows are captured/,$p' > "$W/captured.txt"

cat "$W/excluded.txt"
echo "== the second round"
cat "$W/captured.txt"
echo
in_file() { # description file pattern
    if grep -qE "$3" "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fi
}
not_in_file() {
    if grep -qE "$3" "$2"; then echo "FAIL  $1"; else echo "ok    $1"; fi
}
in_file "the normal Firefox window is captured" "$W/excluded.txt" ': Firefox — #infra - Chat'
in_file "the normal Chromium window is captured" "$W/excluded.txt" ': Chromium — Reference manual'
not_in_file "the private Firefox window isn't recorded" "$W/excluded.txt" 'Release notes'
not_in_file "the incognito Chromium window isn't recorded" "$W/excluded.txt" 'Rich page'
in_file "the private Firefox window is captured on request" "$W/captured.txt" 'Release notes'
in_file "the incognito Chromium window is captured on request" "$W/captured.txt" ': Chromium — Rich page'
if [ -n "${EDGE:-}" ]; then
    in_file "the normal Edge window is captured" "$W/excluded.txt" ': Microsoft Edge — Reference manual'
    in_file "the InPrivate window is captured on request" "$W/captured.txt" ': Microsoft Edge — Rich page'
fi
if [ -n "${BRAVE:-}" ]; then
    in_file "the normal Brave window is captured" "$W/excluded.txt" ': Brave[^—]* — Reference manual'
    in_file "the German private Brave window is captured on request" "$W/captured.txt" ': Brave[^—]* — Rich page'
fi
true
