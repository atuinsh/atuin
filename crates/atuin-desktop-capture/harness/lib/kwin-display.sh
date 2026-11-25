#!/bin/bash
# Started by KWin, which gives it the DISPLAY of its Xwayland. Writes it out and stays.
echo "$DISPLAY" > "$XDG_RUNTIME_DIR/display"
exec sleep infinity
