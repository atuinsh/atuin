# Checks that a session works: gedit and Kate open, and capture follows the focus between them.
capture_start 'apps = "all"' || return
gedit "$P/rich.html" > /dev/null 2>&1 &
wait_window gedit 20
sleep 2
kate "$P/tab2.html" > /dev/null 2>&1 &
wait_window kate 20
sleep 2
if can_focus; then
    mark "focus gedit"
    focus gedit
    sleep 3
    mark "focus kate"
    focus kate
    sleep 3
fi
capture_stop
expect "gedit gets a segment" '^segment [0-9a-f-]+: gedit — rich.html'
expect "Kate gets a segment" '^segment [0-9a-f-]+: kate — tab2.html'
if can_focus; then
    # Background segments can start between the mark and the segment of the focused window.
    records | grep -E '^(segment|==) ' | grep -A1 -E '^== focus gedit' | grep -qE 'gedit —' \
        && echo "ok    focusing gedit starts its segment" || echo "FAIL  focusing gedit starts its segment"
fi
