# Reads Chromium, and VS Code when $VSCODE names its `code` binary: a 12,000-element CI page,
# then a chat page in front, and VS Code in the background.
# A window that a script opens may stay behind, as GNOME prevents focus stealing, so each
# window is focused once it shows.
capture_start 'apps = "all"' || return
chromium_ "file://$P/ci.html"
sleep 8
can_focus && focus chromium
sleep 3
if [ -n "${VSCODE:-}" ]; then
    "$VSCODE" --no-sandbox --ozone-platform=$CHROMIUM_PLATFORM --password-store=basic \
        --user-data-dir "$HOME/vscode" \
        --extensions-dir "$HOME/vscode-extensions" "$P/rich.html" > /dev/null 2>&1 &
    sleep 12
    can_focus && focus code
    sleep 3
fi
chromium_ "file://$P/chat.html"
sleep 5
can_focus && focus chromium
sleep 12
capture_stop
records | grep -v unchanged
echo
expect "the CI page is read whole" '^segment [0-9a-f-]+: (loaded|activated|restructured) snapshot, 1[0-9]{4} nodes'
expect "the chat in Chromium gets a segment" '^segment [0-9a-f-]+: Chromium — #infra - Chat'
[ -n "${VSCODE:-}" ] && expect "VS Code gets a segment" '^segment [0-9a-f-]+: code — '
true
