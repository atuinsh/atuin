# Qt on Wayland activates and deactivates its window in one moment whenever another application
# takes the focus. Capture must not follow that activation: here Chromium opens while Kate
# is running, and Chromium's window must get its segment and its snapshots.
capture_start 'apps = "all"' || return
kate "$P/tab2.html" > /dev/null 2>&1 &
sleep 5
chromium_ "file://$P/chat.html"
sleep 6
can_focus && focus chromium
sleep 15
capture_stop
records | grep -v unchanged
echo
expect "Kate gets a segment" '^segment [0-9a-f-]+: kate — '
expect "Chromium gets a segment" '^segment [0-9a-f-]+: Chromium — #infra - Chat'
chat=$(segment_id 'Chromium — #infra - Chat')
records | grep -qE "^segment ${chat:-none}: (edited|restructured) snapshot" \
    && echo "ok    the chat in Chromium gets snapshots" || echo "FAIL  the chat in Chromium gets snapshots"
