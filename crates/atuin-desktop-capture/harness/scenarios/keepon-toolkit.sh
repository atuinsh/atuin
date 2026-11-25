# Checks the toolkit-accessibility setting after `atuin setup --only --desktop-capture --daemon`
# in a home whose config already has [desktop_capture] enabled = true and no login files, as a
# config from a dotfiles repository has. A config without the section is the comparison. Both runs
# must write the login files and turn the key on, and a second run must not mention desktop
# capture. The scenario needs a GSettings backend that keeps the key, as the gnome kind has, and
# skips otherwise.
export ATUIN_CONFIG_DIR=$W/atuin-config ATUIN_DATA_DIR=$W/atuin-data TMPDIR=$W/tmp
mkdir -p "$ATUIN_CONFIG_DIR" "$ATUIN_DATA_DIR" "$TMPDIR"
CONFIG=$ATUIN_CONFIG_DIR/config.toml
base() { cat > "$CONFIG" << EOT
[daemon]
enabled = true
autostart = false
socket_path = "$SOCK"
pidfile_path = "$W/atuin.pid"

[logs]
dir = "$W/logs"
EOT
}
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fi; }
key() { gsettings get org.gnome.desktop.interface toolkit-accessibility 2>&1; }
if ! gsettings set org.gnome.desktop.interface toolkit-accessibility false 2> /dev/null \
    || [ "$(key)" != false ]; then
    echo "skip  toolkit-accessibility: this session has no GSettings backend that keeps it"
    return 0
fi
base; printf '\n[desktop_capture]\nenabled = true\napps = "all"\n' >> "$CONFIG"
echo "backend: ${GSETTINGS_BACKEND:-default}; before: $(key)"
echo '$ atuin setup --only --desktop-capture --daemon   (config already enabled)'
"$ATUIN" setup --only --desktop-capture --daemon 2>&1
PRE=$(key); echo "after (preset): $PRE"
echo '$ atuin setup --only --desktop-capture --daemon   (again)'
AGAIN=$("$ATUIN" setup --only --desktop-capture --daemon 2>&1); echo "$AGAIN"
FILES=$(cd "$HOME" && ls .profile .xsessionrc .config/autostart/atuin-desktop-capture.desktop .config/environment.d/60-atuin-desktop-capture.conf 2>/dev/null | wc -l)
"$ATUIN" daemon stop >/dev/null 2>&1; sleep 1
"$ATUIN" setup --only --no-desktop-capture 2>&1
gsettings set org.gnome.desktop.interface toolkit-accessibility false
base
echo '$ atuin setup --only --desktop-capture --daemon   (fresh section)'
"$ATUIN" setup --only --desktop-capture --daemon 2>&1
FRESH=$(key); echo "after (fresh): $FRESH"
"$ATUIN" daemon stop >/dev/null 2>&1; sleep 1
echo
check "the preset run writes the login files" '[ "$FILES" -eq 4 ]'
check "the fresh run turns toolkit-accessibility on" '[ "$FRESH" = true ]'
check "the preset run turns toolkit-accessibility on" '[ "$PRE" = true ]'
check "a second preset run says nothing about desktop capture" \
    '! grep -qiE "login|toolkit-accessibility|desktop" <<< "$AGAIN"'
