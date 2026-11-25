# Shows an alert ten times, each for 300 ms after it appears, behind a window that stays
# active, and counts the alerts that capture read.
gedit "$P/rich.html" > /dev/null 2>&1 &
timeout 3 zenity --info --text=warming > /dev/null 2>&1
capture_start 'apps = "all"' || return
sleep 4
for i in $(seq 10); do
    zenity --error --text="Copy failed $i: no space left on device" > /dev/null 2>&1 &
    alert=$!
    wait_window zenity
    sleep 0.3
    kill $alert
    sleep 1.5
done
capture_stop
# A dialog that closes before capture reads it leaves a blank snapshot of one node.
read=$(records | grep -cE '^dialog [0-9a-f-]+: snapshot, ([2-9]|[0-9]{2,}) nodes')
echo "alerts read: $read of 10"
[ "$read" -eq 10 ] && echo "ok    every alert is read" || echo "FAIL  $((10 - read)) alerts were missed"
