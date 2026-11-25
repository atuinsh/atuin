# Reads that run out of time are tried again: with a read budget of 150 ms, no read of the
# 12,000-element page in Chromium finishes, so capture keeps a partial snapshot and tries again
# three times, each after a rest of 20 times as long as the read took.
capture_start 'apps = ["chromium"]' 'read_budget = "150ms"' 'poll_interval = "0s"' || return
chromium_ "file://$P/ci.html"
sleep 8
can_focus && focus chromium
sleep 60
capture_stop
records | grep -v unchanged
echo
expect "the page gets a partial snapshot" '^segment [0-9a-f-]+: [a-z]+ snapshot, [0-9]{1,3} nodes'
expect_none "no read of the page finishes" '^segment [0-9a-f-]+: [a-z]+ snapshot, 1[0-9]{4} nodes'
retried=$(records | grep -cE '^segment [0-9a-f-]+: retried snapshot')
[ "$retried" -eq 3 ] && echo "ok    the window is tried again three times" \
    || echo "FAIL  the window was tried again $retried times"
