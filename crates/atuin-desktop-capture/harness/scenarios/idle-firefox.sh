# Counts how often a static page in Firefox is read when nothing on it is changing. Firefox keeps
# a preloaded New Tab page in every window, whose rotating stories send events all the time.
keep_a11y
sleep 1
firefox_ "file://$P/long.html"
sleep 10
capture_start 'apps = ["firefox"]' 'poll_interval = "0s"' || return
sleep 60
capture_stop
page=$(segment_id 'Firefox — ')
reads=$(records | grep -cE "^segment ${page:-none}: (restructured|edited|loaded) snapshot.*unchanged")
echo "unchanged reads of the idle page in 60 s: $reads"
[ "$reads" -eq 0 ] && echo "ok    an idle page is not read again" || echo "FAIL  an idle page was read again $reads times"
