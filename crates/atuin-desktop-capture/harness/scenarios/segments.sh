# Follows the focus across GTK 3, Qt, and Firefox, and reads a dialog and a notification that
# disappear at once. GNOME Terminal and Konsole, which are on the accessibility bus, don't leave
# any records.
[ "$KIND" = gnome ] || dunst > /dev/null 2>&1 &
# A first start of zenity fills the caches, so that the alert below shows at once.
timeout 3 zenity --info --text=warming > /dev/null 2>&1
capture_start 'apps = "all"' || return
gedit "$P/rich.html" > /dev/null 2>&1 &
sleep 4
kate "$P/tab2.html" > /dev/null 2>&1 &
sleep 5
firefox_ "file://$P/chat.html"
sleep 8
gnome-terminal > /dev/null 2>&1 &
sleep 4
konsole > /dev/null 2>&1 &
sleep 5
if can_focus; then
    for app in gedit kate gnome-terminal konsole firefox; do
        mark "focus $app"
        focus "$app"
        sleep 4
    done
    mark "Firefox in front for 15 s"
    sleep 15
fi
mark "zenity alert for 300 ms"
zenity --error --text="Copy failed: no space left on device" > /dev/null 2>&1 &
alert=$!
wait_window zenity
sleep 0.3
kill $alert
sleep 2
mark "notification"
notify-send -a Backup "Backup failed" "The disk is full"
sleep 3
FOUND=$("$ATUIN" desktop-capture search --kind notification "disk is full" 2>&1)
capture_stop
records | grep -v unchanged
echo
expect "gedit gets a segment" '^segment [0-9a-f-]+: gedit — '
expect "Kate gets a segment" '^segment [0-9a-f-]+: kate — '
expect "Firefox gets a segment" '^segment [0-9a-f-]+: Firefox — #infra'
alert=$(segment_id 'zenity — ')
expect "the alert is read" "^dialog ${alert:-none}: snapshot, ([2-9]|[0-9]{2,}) nodes"
expect_none "terminals leave no records" '^[a-z]+ [0-9a-f-]+: (gnome-terminal|konsole|Terminal)'
expect_none "no segment names a terminal" '^[a-z]+ [0-9a-f-]+: .*(gnome-terminal|konsole)'
records | sed -n '/^== focus gnome-terminal/,/^== focus konsole/p' | grep -qE '^segment [0-9a-f-]+: (gedit|kate) —' \
    && echo "FAIL  focusing the terminal kept the last window's segment" || echo "ok    focusing the terminal ends the last window's segment"
if [ "$KIND" = mutter ]; then
    echo "skip  the notification: mutter has no notification service"
else
    expect "the notification is recorded" '^notification [0-9a-f-]*: Backup — Backup failed$'
    grep -qi 'disk is full' <<< "$FOUND" \
        && echo "ok    search finds the notification by its body" || echo "FAIL  search finds the notification by its body"
fi
if can_focus; then
    records | sed -n '/^== Firefox in front/,/^== zenity/p' | grep -qE '^segment [0-9a-f-]+: restructured snapshot' \
        && echo "ok    the chat in front gets snapshots" || echo "FAIL  the chat in front gets snapshots"
fi
