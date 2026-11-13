# ctrl-r, a few keystrokes, then the match goes back to the prompt.
Source lib/window.tape
Source lib/start.tape

Sleep 600ms
Ctrl+R
Wait+Screen /history count: \d+/
Sleep 800ms
Type@150ms "carg"
Wait+Screen /cargo clippy/
Sleep 1s
Up
Sleep 600ms
Up
Sleep 1s
Tab
Wait+Line /atuin \$ cargo/
Sleep 2s
