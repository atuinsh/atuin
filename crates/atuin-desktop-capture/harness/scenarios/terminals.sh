# Reads web terminals in the shape of xterm.js in Chromium and Firefox, the real xterm.js when
# $XTERM names a directory with its lib/xterm.js and css/xterm.css, and VS Code's integrated
# terminal when $VSCODE names its `code` binary. Capture must keep the text around each terminal,
# show the terminal as a line of its own, and not store any of the marker that every terminal
# holds.
capture_start 'apps = "all"' || return
chromium_ "file://$P/terminal.html"
sleep 8
can_focus && focus chromium
sleep 4
firefox_ "file://$P/terminal.html"
sleep 8
if [ -n "${XTERM:-}" ]; then
    cp -r "$XTERM" "$P/xterm"
    # Firefox reads the tab in the background. A second Chromium is an application of its own,
    # which the sweep reads.
    firefox_more --new-tab "file://$P/xterm.html"
    sleep 4
    chromium_ "file://$P/xterm.html"
    sleep 8
fi
can_focus && focus firefox
sleep 4
if [ -n "${VSCODE:-}" ]; then
    # A task that runs when the folder opens shows the marker in the terminal panel.
    mkdir -p "$HOME/vscode/User" "$HOME/ws/.vscode"
    cat > "$HOME/vscode/User/settings.json" << 'EOF'
{ "editor.accessibilitySupport": "on", "security.workspace.trust.enabled": false,
  "task.allowAutomaticTasks": "on", "workbench.startupEditor": "none" }
EOF
    cat > "$HOME/ws/.vscode/tasks.json" << 'EOF'
{ "version": "2.0.0", "tasks": [{ "label": "probe", "type": "shell",
  "command": "printf 'TERMPROBE-12345 row\\n'; exec sh", "isBackground": true,
  "problemMatcher": [], "runOptions": { "runOn": "folderOpen" } }] }
EOF
    "$VSCODE" --no-sandbox --ozone-platform=$CHROMIUM_PLATFORM --password-store=basic \
        --user-data-dir "$HOME/vscode" --extensions-dir "$HOME/vscode-extensions" \
        "$HOME/ws" > /dev/null 2>&1 &
    sleep 15
    can_focus && focus code
    sleep 4
fi
can_focus && focus chromium
sleep 4
texts=$("$ATUIN" desktop-capture log --limit 1000 --style ndjson 2> /dev/null \
    | $PY -c 'import json, sys; [print(json.loads(line)["segment"]) for line in sys.stdin]' \
    | while read -r id; do "$ATUIN" desktop-capture show "$id" 2> /dev/null; done)
hits=$("$ATUIN" desktop-capture search --style ndjson TERMPROBE 2> /dev/null)
capture_stop
echo "$texts" > "$W/texts.txt"
records | grep -v unchanged
echo "$texts" | grep -E 'KEEPME|terminal|TERMPROBE' | sort | uniq -c
echo
holds() {
    if grep -qF "$2" <<< "$texts"; then echo "ok    $1"; else echo "FAIL  $1"; fi
}
holds "the text beside the terminal is kept" "KEEPME-777"
holds "the text after the terminal is kept" "KEEPME-888"
holds "the terminal shows as a line of its own" "[terminal]"
if grep -qF TERMPROBE <<< "$texts"; then echo "FAIL  no stored text holds the terminal's marker"
else echo "ok    no stored text holds the terminal's marker"; fi
[ -z "$hits" ] && echo "ok    a search for the marker finds nothing" \
    || echo "FAIL  a search for the marker finds nothing"
grep -rqaF TERMPROBE "$ATUIN_DATA_DIR" && echo "FAIL  the data directory holds no marker" \
    || echo "ok    the data directory holds no marker"
expect "Chromium gets a segment" '^segment [0-9a-f-]+: Chromium — Cloud shell'
expect "Firefox gets a segment" '^segment [0-9a-f-]+: Firefox — Cloud shell'
if [ -n "${XTERM:-}" ]; then
    expect "the xterm.js page is read in Firefox" '^[a-z]+ [0-9a-f-]+: Firefox — xterm.js terminal'
    expect "the xterm.js page is read in Chromium" '^[a-z]+ [0-9a-f-]+: Chromium — xterm.js terminal'
fi
[ -n "${VSCODE:-}" ] && expect "VS Code gets a segment" '^segment [0-9a-f-]+: [Cc]ode — '
true
