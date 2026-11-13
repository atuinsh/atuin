# Claude Code finds a command in your shell history through atuin's MCP
# server. The rig (rigs/history.sh) sets Claude up; the history is the
# fixture's. Claude is real, so the waits match what any answer shows.
Source lib/window.tape
Set Height 900
Source lib/start.tape

Sleep 600ms
Type "claude"
Enter
Wait+Screen@180s /shift\+tab to cycle/
Sleep 800ms
Type@35ms "how did I check the hub's logs earlier? keep it short."
Enter
# Claude closes each turn with "✻ <verb> for 4s".
Wait+Screen@300s /✻ \w+ for \d/
Sleep 4s
