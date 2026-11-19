# Claude Code recalls a decision made in a Codex thread last week, through
# atuin's MCP server. A question, not a task, so the answer is the payoff
# rather than a diff. The rig (rigs/mcp.sh) seeds the
# threads; Claude is real, so the waits match what any answer shows.
Source lib/window.tape
Set Height 900
Source lib/start.tape

Sleep 600ms
Type "claude"
Enter
Wait+Screen@180s /shift\+tab to cycle/
Sleep 800ms
Type@35ms "what did we decide about parse_duration's panics in codex last week? keep it short."
Enter
# Claude closes each turn with "✻ <verb> for 4s".
Wait+Screen@300s /✻ \w+ for \d/
Sleep 4s
