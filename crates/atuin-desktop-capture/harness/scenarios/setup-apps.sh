# Checks what `atuin setup` writes to the apps setting, and that the result captures.
#
# A config without a [desktop_capture] section gets apps = "all" from
# `atuin setup --only --desktop-capture --daemon`, and the daemon then captures gedit without
# another command. `--desktop-apps listed` writes apps = [], which status reports as no
# application selected. A hand-written list survives a run that keeps the listed choice, and
# `--desktop-apps all` replaces it. `atuin desktop-capture apps --write` adds gedit to an empty
# list, which a running daemon applies, and a second run doesn't add anything. Without a
# terminal, a run on apps = "all" leaves the configuration as it is, and `--write` refuses to
# replace "all". A list that contains "*" fails to load with a hint that names apps = "all".
# `--no-desktop-capture` takes out every login file that setup wrote.
export ATUIN_CONFIG_DIR=$W/atuin-config ATUIN_DATA_DIR=$W/atuin-data TMPDIR=$W/tmp
mkdir -p "$ATUIN_CONFIG_DIR" "$ATUIN_DATA_DIR" "$TMPDIR"
CONFIG=$ATUIN_CONFIG_DIR/config.toml
cat > "$CONFIG" << EOT
[daemon]
enabled = true
autostart = false
socket_path = "$SOCK"
pidfile_path = "$W/atuin.pid"

[logs]
dir = "$W/logs"
EOT
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fi; }
apps_line() { sed -n '/^\[desktop_capture\]/,/^\[/{/^apps *=/p}' "$CONFIG"; }
stop_daemon() {
    [ -n "${DAEMON:-}" ] || return 0
    "$ATUIN" daemon stop > /dev/null 2>&1
    for _ in $(seq 50); do kill -0 "$DAEMON" 2> /dev/null || break; sleep 0.1; done
    kill "$DAEMON" 2> /dev/null
    DAEMON=
}
trap stop_daemon EXIT
trap 'exit 143' TERM INT

echo "\$ atuin setup --only --desktop-capture --daemon   (fresh [desktop_capture])"
"$ATUIN" setup --only --desktop-capture --daemon 2>&1
FRESH=$(apps_line)
echo "apps after a fresh setup: $FRESH"
LOGIN_FILES=$(cd "$HOME" && ls .profile .xsessionrc .config/autostart/atuin-desktop-capture.desktop \
    .config/environment.d/60-atuin-desktop-capture.conf 2> /dev/null | wc -l)

"$ATUIN" daemon start > "$W/daemon.log" 2>&1 &
DAEMON=$!
wait_socket || echo "FAIL  the daemon did not start; see $W/daemon.log"
echo "Fresh setup phrase quokka 5521" > "$P/quokka.txt"
gedit "$P/quokka.txt" > /dev/null 2>&1 &
wait_window gedit
sleep 5
can_focus && focus gedit && sleep 3
STATUS_ALL=$("$ATUIN" desktop-capture status 2>&1)
echo "$STATUS_ALL"
HIT=$(find_segment quokka 5521)
echo "segment of the phrase: ${HIT:-none}"

echo "\$ atuin setup --only --desktop-capture --desktop-apps listed"
"$ATUIN" setup --only --desktop-capture --desktop-apps listed 2>&1
LISTED=$(apps_line)
echo "apps after --desktop-apps listed: $LISTED"
status_is "no application is selected"
NOAPPS=$?
"$ATUIN" desktop-capture status 2>&1

sed -i 's/^apps = \[\]$/apps = ["gedit"]/' "$CONFIG"
"$ATUIN" setup --only --desktop-capture --daemon > /dev/null 2>&1
KEPT=$(apps_line)
"$ATUIN" setup --only --desktop-capture --desktop-apps listed > /dev/null 2>&1
KEPT_LISTED=$(apps_line)
"$ATUIN" setup --only --desktop-capture --desktop-apps all > /dev/null 2>&1
REPLACED=$(apps_line)
echo "hand-written list: kept=$KEPT kept_listed=$KEPT_LISTED replaced=$REPLACED"

echo "\$ atuin desktop-capture apps --write   (apps = [])"
sed -i 's/^apps = "all"$/apps = []/' "$CONFIG"
status_is "no application is selected"
"$ATUIN" desktop-capture apps --write > "$W/apps-write.log" 2>&1
sed -n '/^6\. /,$p' "$W/apps-write.log"
WRITTEN=$(apps_line)
echo "apps after apps --write: $WRITTEN"
status_is "^Desktop capture: running"
LIVE=$?
echo "Written list phrase wombat 7731" > "$P/wombat.txt"
gedit "$P/wombat.txt" > /dev/null 2>&1 &
sleep 5
can_focus && focus gedit && sleep 3
HIT_LISTED=$(find_segment wombat 7731)
echo "segment of the phrase after the write: ${HIT_LISTED:-none}"
"$ATUIN" desktop-capture apps --write > "$W/apps-again.log" 2>&1
AGAIN=$(apps_line)
sed -n '/^6\. /,$p' "$W/apps-again.log"
sed -i 's/^apps = \[.*\]$/apps = "all"/' "$CONFIG"
"$ATUIN" desktop-capture apps < /dev/null > "$W/apps-all.log" 2>&1
ALL_KEPT=$(apps_line)
sed -n '/^6\. /,$p' "$W/apps-all.log"
"$ATUIN" desktop-capture apps --write > "$W/apps-write-all.log" 2>&1
WRITE_ALL_EXIT=$?
WRITE_ALL=$(apps_line)
sed -n '/^6\. /,$p' "$W/apps-write-all.log"

cp "$CONFIG" "$W/config.good"
sed -i 's/^apps = "all"$/apps = ["*"]/' "$CONFIG"
STAR=$("$ATUIN" desktop-capture status 2>&1)
echo "$STAR"
cp "$W/config.good" "$CONFIG"
stop_daemon

echo "\$ atuin setup --only --no-desktop-capture"
"$ATUIN" setup --only --no-desktop-capture 2>&1
LEFT=$(cd "$HOME" && grep -rl "atuin setup" .profile .xsessionrc .config/autostart .config/environment.d \
    2> /dev/null)
echo "files with setup's marker after the removal: ${LEFT:-none}"

echo
check "a fresh setup writes apps = \"all\"" '[ "$FRESH" = "apps = \"all\"" ]'
check "a fresh setup writes the login files" '[ "$LOGIN_FILES" -eq 4 ]'
check "capture runs after setup alone" 'grep -q "^Desktop capture: running" <<< "$STATUS_ALL"'
check "gedit is captured after setup alone" '[ -n "$HIT" ]'
check "--desktop-apps listed writes apps = []" '[ "$LISTED" = "apps = []" ]'
check "status says that no application is selected" '[ "$NOAPPS" -eq 0 ]'
check "a hand-written list survives a run without --desktop-apps" '[ "$KEPT" = "apps = [\"gedit\"]" ]'
check "a hand-written list survives --desktop-apps listed" '[ "$KEPT_LISTED" = "apps = [\"gedit\"]" ]'
check "--desktop-apps all replaces a hand-written list" '[ "$REPLACED" = "apps = \"all\"" ]'
check "apps --write adds gedit to an empty list" 'grep -q "\"gedit\"" <<< "$WRITTEN"'
check "capture runs after the write without a restart" '[ "$LIVE" -eq 0 ]'
check "gedit is captured after the write" '[ -n "$HIT_LISTED" ]'
check "a second apps --write adds nothing" \
    '[ "$AGAIN" = "$WRITTEN" ] && grep -q "selects or excludes each" "$W/apps-again.log"'
check "apps without a terminal keeps apps = \"all\"" \
    '[ "$ALL_KEPT" = "apps = \"all\"" ] && grep -q "put this line" "$W/apps-all.log"'
check "apps --write refuses to replace apps = \"all\"" \
    '[ "$WRITE_ALL_EXIT" -ne 0 ] && [ "$WRITE_ALL" = "apps = \"all\"" ] && grep -q "only adds to a list" "$W/apps-write-all.log"'
check "a list with \"*\" fails with the hint" 'grep -q "use apps = \"all\"" <<< "$STAR"'
check "--no-desktop-capture removes the login files" '[ -z "$LEFT" ]'
