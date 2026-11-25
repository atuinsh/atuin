# Reads libadwaita spin rows, which GTK 4 leaves out of the accessibility tree. Capture finds
# a spin row only once the row had the keyboard focus. The window's first row is a switch, so
# no spin row has the focus at first. Then the last spin row takes the focus, and the one
# between stays unfocused.
if ! $PY -c 'import gi; gi.require_version("Adw", "1")' 2> /dev/null; then
    echo "skip  libadwaita's typelib is missing: install gir1.2-adw-1"
    return 0
fi
if ! can_focus; then
    echo "skip  this kind of session cannot focus a window"
    return 0
fi
capture_start 'apps = ["spin-rows"]' || return
$PY "$T/spin-rows.py" > "$W/spin-rows.out" 2>&1 &
probe=$!
wait_window spin-rows
focus spin-rows
sleep 4
segment=$(find_segment --app spin-rows First switch)
before=$("$ATUIN" desktop-capture show "${segment:-00000000-0000-0000-0000-000000000000}" 2>&1)
mark "focus the third spin row"
kill -USR1 $probe
sleep 6
after=$("$ATUIN" desktop-capture show "${segment:-00000000-0000-0000-0000-000000000000}" 2>&1)
capture_stop
kill $probe 2> /dev/null
records
echo "-- before the focus moved:"
echo "$before"
echo "-- after:"
echo "$after"
echo
[ -n "$segment" ] && echo "ok    the window gets a segment" || echo "FAIL  the window gets a segment"
echo "$before" | grep -qx '\[x\] First switch' \
    && echo "ok    the switch row is read" || echo "FAIL  the switch row is read"
echo "$before" | grep -q 'spin' \
    && echo "FAIL  no spin row is read before one had the focus" \
    || echo "ok    no spin row is read before one had the focus"
echo "$after" | grep -A1 -x 'Third spin' | grep -qx '3' \
    && echo "ok    the focused spin row is read with its value" \
    || echo "FAIL  the focused spin row is read with its value"
echo "$after" | grep -q 'Second spin' \
    && echo "FAIL  the spin row that never had the focus stays missing" \
    || echo "ok    the spin row that never had the focus stays missing"
