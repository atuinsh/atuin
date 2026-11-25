# Reads Firefox's background tabs: a window with a long page in front and three background
# tabs, one of them a chat that receives a message every 6 seconds. Then switches to the chat.
keep_a11y
sleep 1
firefox_ "file://$P/long.html"
sleep 8
firefox_more --new-tab "file://$P/tab2.html"
sleep 2
firefox_more --new-tab "file://$P/chat.html"
sleep 2
can_focus && focus firefox
select_tab firefox "Reference manual"
sleep 2
capture_start 'apps = ["firefox"]' || return
sleep 30
mark "switch to the chat tab"
select_tab firefox "#infra"
sleep 15
capture_stop
records | grep -v unchanged
echo
page=$(segment_id 'Firefox — ')
notes=$(segment_id 'Firefox — Release notes')
chat=$(segment_id 'Firefox — #infra - Chat')
records | grep -m1 -E '^segment [0-9a-f-]+: .* — ' | grep -q 'Firefox — Reference manual' \
    && echo "ok    the long page is the active segment" || echo "FAIL  the long page is the active segment"
expect "the release notes tab is swept" "^background ${notes:-none}: swept snapshot"
expect "the chat tab is read in the background" "^background ${chat:-none}: [a-z]+ snapshot"
expect_none "the New Tab page is never read" 'New Tab'
reads=$(records | sed -n '1,/^== switch/p' \
    | grep -cE "^segment ${page:-none}: (restructured|edited) snapshot.*unchanged")
[ "$reads" -eq 0 ] && echo "ok    the page in front is not read again for the tabs" \
    || echo "FAIL  the page in front was read again $reads times"
records | sed -n '/^== switch/,$p' | grep -qE '^segment [0-9a-f-]+: Firefox — #infra - Chat — Mozilla Firefox' \
    && echo "ok    switching to the chat tab starts its segment" || echo "FAIL  switching to the chat tab starts its segment"
