# A Claude Code thread on the laptop, picked up in Codex on the devbox. The
# rig (rigs/threads.sh) makes the two machines; the agents are real, so the
# waits match what any answer shows rather than its words.
Source lib/window.tape
# Codex and Claude want the rows.
Set Height 900
Source lib/start.tape

Sleep 600ms
Type "claude"
Enter
Wait+Screen@180s /shift\+tab to cycle/
Sleep 800ms
Type@40ms "parse_duration in src/duration.rs: any input that panics? one sentence."
Enter
# Claude closes each turn with "✻ <verb> for 4s".
Wait+Screen@180s /✻ \w+ for \d/
Sleep 2s
Type "/exit"
Sleep 300ms
Enter
Wait+Line@60s /laptop .*\$/

Hide
Type "atuin sync >/dev/null && clear"
Enter
Wait+Line@60s /laptop .*\$/
Show

Sleep 600ms
Type "ssh devbox"
Enter
Wait+Line@60s /devbox .*\$/
Sleep 600ms
Type "atuin resume"
Enter
Wait+Screen@180s /1 thread/
Sleep 1.5s
Enter
Wait+Screen@180s /Continue in/
Sleep 1s
Down
Sleep 400ms
Down
Sleep 800ms
Enter
Wait+Screen@180s /OpenAI Codex/
Sleep 1.5s
Type@40ms "fix them, no new tests"
Enter
Wait+Screen@180s /esc to interrupt/
Wait+Screen@180s /Worked for/
Sleep 3s
