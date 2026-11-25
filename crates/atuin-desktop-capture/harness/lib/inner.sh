# Runs inside the private D-Bus session that session.sh starts: starts the display server, then
# defines the helpers below and runs the scenario, with its output in $W/result.txt.
#
# Variables for scenarios:
#   W     the work directory          P     the directory of the test pages, for file:// URLs
#   KIND  the kind of session         ATUIN the atuin binary, with its helper next to it
#
# Helpers for scenarios:
#   capture_start, capture_stop, mark, records, segment_id, find_segment, daemon_config,
#   wait_socket, and status_is, from capture.sh
#   keep_a11y                       starts a client that listens for events, so that Qt and
#                                   Firefox join the accessibility bus before capture starts
#   expect <description> <regex>    prints "ok" or "FAIL" for whether a record matches
#   expect_none <description> <regex>  the same for whether no record matches
#   can_focus                       whether this kind of session can focus a window
#   focus <application>             focuses a window whose application name contains the text
#   windows                         lists the windows that the compositor knows
#   type_text <text>                types into the focused window
#   firefox_ <url...>               starts Firefox; firefox_more --new-tab <url> adds to it
#   chromium_ <url...>              starts Chromium, on Wayland where the session offers it
#   on_x11 <command...>             runs a GTK or Qt program through X11 (x11, kwin-xwayland)
#   select_tab <app> <title text>   selects a tab through its accessibility action
#   wait_window <app> [seconds]     waits until the application shows a window
#   can_type                        whether this kind of session can type text
set -u
unset GTK_A11Y GNOME_ACCESSIBILITY QT_ACCESSIBILITY \
    QT_LINUX_ACCESSIBILITY_ALWAYS_ON ACCESSIBILITY_ENABLED GDK_BACKEND QT_QPA_PLATFORM
PY=/usr/bin/python3
T=$HARNESS/tools
P=$W/pages
$PY "$HARNESS/pages/make.py" "$P"
cp "$HARNESS"/pages/*.html "$P/"
CHROMIUM_PLATFORM=wayland

wayland_env() {
    export XDG_SESSION_TYPE=wayland GDK_BACKEND=wayland QT_QPA_PLATFORM=wayland MOZ_ENABLE_WAYLAND=1
}

wait_for() {
    for _ in $(seq 100); do
        [ -e "$1" ] && return 0
        sleep 0.1
    done
    echo "timed out waiting for $1" >&2
    return 1
}

# A client that starts before the bus launcher owns org.a11y.Bus activates a second launcher, so
# the launcher has to own the name before any client starts.
start_a11y_bus() {
    /usr/libexec/at-spi-bus-launcher --launch-immediately >> "$W/display.log" 2>&1 &
    for _ in $(seq 100); do
        gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
            --method org.freedesktop.DBus.NameHasOwner org.a11y.Bus 2> /dev/null | grep -q true \
            && return 0
        sleep 0.1
    done
    echo "timed out waiting for org.a11y.Bus" >&2
    return 1
}

case $KIND in
x11)
    n=90
    while [ -e /tmp/.X11-unix/X$n ] || [ -e /tmp/.X$n-lock ]; do n=$((n + 1)); done
    export DISPLAY=:$n XDG_SESSION_TYPE=x11
    Xvfb "$DISPLAY" -screen 0 1600x1000x24 -nolisten tcp > "$W/display.log" 2>&1 &
    wait_for /tmp/.X11-unix/X$n
    # The bus launcher marks the X display with the address of the bus, so it starts after the
    # X server and before every client of the bus.
    start_a11y_bus
    xfwm4 >> "$W/display.log" 2>&1 &
    sleep 1
    CHROMIUM_PLATFORM=x11
    focus() { xdotool search --class "$1" | tail -1 | xargs -r -I{} xdotool windowactivate --sync {}; }
    windows() { xdotool search --onlyvisible --name . getwindowname %@ 2>/dev/null; }
    type_text() { xdotool type --delay 50 "$1"; }
    ;;
kwin | kwin-xwayland)
    start_a11y_bus
    wayland_env
    export XDG_CURRENT_DESKTOP=KDE
    if [ "$KIND" = kwin-xwayland ]; then
        # KWin hands the DISPLAY of its Xwayland only to the programs that it starts.
        kwin_wayland --xwayland --virtual --width 1600 --height 1000 --socket wayland-h \
            --exit-with-session "$HARNESS/lib/kwin-display.sh" > "$W/display.log" 2>&1 &
        wait_for "$XDG_RUNTIME_DIR/display"
        export DISPLAY=$(cat "$XDG_RUNTIME_DIR/display")
        CHROMIUM_PLATFORM=x11
    else
        kwin_wayland --virtual --width 1600 --height 1000 --socket wayland-h > "$W/display.log" 2>&1 &
        wait_for "$XDG_RUNTIME_DIR/wayland-h"
    fi
    export WAYLAND_DISPLAY=wayland-h
    focus() {
        sed "s/@APP@/$1/g" "$T/kwin-focus.js.in" > "$W/focus.js"
        local id
        id=$(gdbus call --session --dest org.kde.KWin --object-path /Scripting \
            --method org.kde.kwin.Scripting.loadScript "$W/focus.js" "focus-$RANDOM" | tr -dc 0-9)
        gdbus call --session --dest org.kde.KWin --object-path "/Scripting/Script$id" \
            --method org.kde.kwin.Script.run > /dev/null
        sleep 0.3
        gdbus call --session --dest org.kde.KWin --object-path "/Scripting/Script$id" \
            --method org.kde.kwin.Script.stop > /dev/null 2>&1
    }
    windows() { echo "listing windows is not available in KWin" >&2; }
    # KWin doesn't offer a virtual keyboard to clients.
    type_text() { echo "typing is not available in KWin" >&2; return 1; }
    ;;
mutter | gnome)
    start_a11y_bus
    wayland_env
    export XDG_CURRENT_DESKTOP=GNOME
    if [ "$KIND" = gnome ]; then
        mkdir -p "$HOME/.local/share/gnome-shell/extensions"
        cp -r "$HARNESS/gnome/desktop-capture-test@atuin.sh" "$HOME/.local/share/gnome-shell/extensions/"
        gsettings set org.gnome.shell enabled-extensions "['desktop-capture-test@atuin.sh']"
        gsettings set org.gnome.shell disable-extension-version-validation true
        gsettings set org.gnome.shell welcome-dialog-last-shown-version '999'
        gnome-shell --headless --wayland --no-x11 --virtual-monitor 1600x1000 \
            --wayland-display wayland-h --sm-disable > "$W/display.log" 2>&1 &
    else
        mutter --headless --wayland --no-x11 --virtual-monitor 1600x1000 \
            --wayland-display wayland-h --sm-disable > "$W/display.log" 2>&1 &
    fi
    wait_for "$XDG_RUNTIME_DIR/wayland-h"
    export WAYLAND_DISPLAY=wayland-h
    sleep 2
    # GTK 3 counts a window as active only when it has the keyboard focus, and a headless
    # mutter doesn't have a keyboard until a remote-desktop session adds one.
    $PY "$T/mutter-keyboard.py" hold 100000 > "$W/keyboard.log" 2>&1 &
    if [ "$KIND" = gnome ]; then
        focus() {
            gdbus call --session --dest sh.atuin.DesktopCaptureTest --object-path /sh/atuin/DesktopCaptureTest \
                --method sh.atuin.DesktopCaptureTest.Focus "$1" > /dev/null
        }
        windows() {
            gdbus call --session --dest sh.atuin.DesktopCaptureTest --object-path /sh/atuin/DesktopCaptureTest \
                --method sh.atuin.DesktopCaptureTest.Windows | $PY -c "import ast,sys; print(ast.literal_eval(sys.stdin.read().strip())[0])"
        }
    else
        focus() { echo "focusing a window is not available in mutter" >&2; return 1; }
        windows() { echo "listing windows is not available in mutter" >&2; }
    fi
    type_text() { $PY "$T/mutter-keyboard.py" type "$1"; }
    ;;
sway)
    start_a11y_bus
    wayland_env
    export XDG_CURRENT_DESKTOP=sway
    WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
        sway -c "$HARNESS/lib/sway.conf" > "$W/display.log" 2>&1 &
    wait_for "$XDG_RUNTIME_DIR/wayland-1"
    for _ in $(seq 100); do
        SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.* 2> /dev/null | head -1)
        [ -S "$SWAYSOCK" ] && break
        sleep 0.1
    done
    export WAYLAND_DISPLAY=wayland-1 SWAYSOCK
    # GTK 3 counts a window as active only when it has the keyboard focus, and a headless
    # sway doesn't have a keyboard until a client adds a virtual one.
    wtype -s 100000000 > "$W/keyboard.log" 2>&1 &
    focus() { swaymsg -q "[app_id=\"(?i)$1\"] focus"; }
    windows() {
        swaymsg -t get_tree | $PY -c '
import json, sys
def walk(node):
    if node.get("pid"):
        print("*" if node.get("focused") else " ", node.get("app_id"), json.dumps(node.get("name")))
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        walk(child)
walk(json.load(sys.stdin))'
    }
    type_text() { wtype "$1"; }
    ;;
*)
    echo "unknown kind of session: $KIND" >&2
    exit 2
    ;;
esac
# The session bus starts services such as gnome-terminal-server with the environment that it
# had at its own start, from before the display existed. Desktop sessions update that
# environment the same way.
dbus-update-activation-environment DISPLAY WAYLAND_DISPLAY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP \
    GDK_BACKEND QT_QPA_PLATFORM MOZ_ENABLE_WAYLAND SWAYSOCK 2> /dev/null

source "$HARNESS/lib/capture.sh"
keep_a11y() { $PY "$T/listen.py" > /dev/null 2>&1 & }
# Debian ships Firefox as firefox-esr, and other distributions as firefox.
FIREFOX=${FIREFOX:-$(command -v firefox-esr || command -v firefox)}
firefox_() {
    mkdir -p "$HOME/firefox"
    "$FIREFOX" --profile "$HOME/firefox" "$@" > /dev/null 2>&1 &
}
firefox_more() { timeout 10 "$FIREFOX" --profile "$HOME/firefox" "$@" > /dev/null 2>&1; }
# Chromium and Electron keep passwords in the Secret Service by default, and asking it to
# create a keyring opens a prompt that holds the keyboard in GNOME Shell.
chromium_() {
    chromium --no-sandbox --ozone-platform=$CHROMIUM_PLATFORM --user-data-dir="$HOME/chromium-$RANDOM" \
        --no-first-run --password-store=basic "$@" > /dev/null 2>&1 &
}
on_x11() { GDK_BACKEND=x11 QT_QPA_PLATFORM=xcb "$@"; }
expect() {
    if records | grep -qE "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fi
}
expect_none() {
    if records | grep -qE "$2"; then echo "FAIL  $1"; else echo "ok    $1"; fi
}
can_focus() { [ "$KIND" != mutter ]; }
can_type() { case $KIND in kwin | kwin-xwayland) false ;; *) true ;; esac; }
select_tab() { $PY "$T/select-tab.py" "$1" "$2"; }
wait_window() { $PY "$T/wait-window.py" "$1" "${2:-10}"; }

source "$SCENARIO" "$@" > "$W/result.txt" 2>&1
pkill -P $$ 2> /dev/null
