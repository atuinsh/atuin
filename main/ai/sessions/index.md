# Agent sessions

With `capture_sessions = true` under `[ai]`, the Atuin daemon records the sessions of your AI coding agents (Claude Code, Codex, opencode and Pi) into the encrypted record store, and syncs them like your history. `atuin ai session` lists, shows, searches and follows them.

The daemon keeps a local search index of these sessions beside the record store, and brings it up to date as sessions are captured, synced in from your other machines, or imported.

## Searching

```
atuin ai session search "flaky test"
```

- The last word of the query matches as a prefix once it's at least two characters long, so `atuin ai session search "migr"` finds sessions mentioning "migration." A single character still matches only a whole word. Earlier words always match whole.
- An empty query (`atuin ai session search ""`) lists the newest sessions first, rather than returning nothing.
- `--harness` limits the search to one agent.

## Resuming

`atuin ai resume` (or a key you bind to it, see [Binding `atuin ai resume`](https://docs.atuin.sh/reference/init/#binding-atuin-ai-resume)) opens a picker over your sessions. It works like the history search: type to search, `Ctrl`+`O` inspects the selected session, and `Esc` leaves.

No key opens it by default. To open it with `Ctrl`+`]` (which replaces the shell's own character search on that key), pass `--bind-ai-resume` to `atuin init`; or bind the widget to a key of your choice after the `atuin init` line:

```
bindkey -M emacs '^]' atuin-ai-resume
bindkey -M viins '^]' atuin-ai-resume-viins
bindkey -M vicmd '^]' atuin-ai-resume-vicmd
```

```
atuin-bind -m emacs      '\C-]' atuin-ai-resume-emacs
atuin-bind -m vi-insert  '\C-]' atuin-ai-resume-viins
atuin-bind -m vi-command '\C-]' atuin-ai-resume-vicmd
```

```
bind ctrl-] _atuin_ai_resume
bind -M insert ctrl-] _atuin_ai_resume
```

Through the widget, the command it picks lands in your shell, as with the history search.

Each row shows when the session was last active, its agent, its title and its message count (when there's room). `Ctrl`+`R` cycles which sessions are listed: those in the current repository (where the picker opens), all of them, this machine's, the current directory's, and the current branch's.

Words in the query of the form `filter:value` narrow the search further, and show as chips in the input:

- `agent:<name>` (or `a:<name>` for short): only that agent's sessions, with `claude-code` (or `claude`, `cc`), `codex` (`cx`), `opencode` (`oc`) or `pi`. `Alt`+`A` cycles through them in turn, and back to all agents.
- `m:<model>`: only sessions whose model contains this, as in `m:opus`.
- `b:<branch>`: only sessions on this git branch.

Put a `\` before a word to search for it as text instead (`\b:main`).

The preview under the list starts with where the session ran, its repository, branch and, for a session recorded on another machine, that machine (by the end of its host id), and how many forks it has:

```
       atuin · feat/ai-sessions · @3f9a12bc · 2 forks
first  …
last   …
```

At first the preview shows a line or two of the first prompt, the match and the last reply. Scroll it to read them in full, one after another: with the mouse wheel over it, or with `Shift`+`Down` and `Shift`+`Up` (a line) and `Shift`+`Page Down` and `Shift`+`Page Up` (a page). `Alt` works in place of `Shift`, for terminals that keep shift and the arrows for themselves. Scroll back to the top for the overview again. On a wide terminal the same goes for the pane beside the list, and in Inspect for the conversation. The wheel over the list moves the selection, as in the history search, and a scrollbar shows when there's more than fits.

While the picker has the mouse, your terminal can't select text the usual way. Most terminals still select with `Shift` held while you drag (`Option` in iTerm2, `Fn` in macOS Terminal). To leave the mouse to the terminal (the top-level `no_mouse = true` does the same for both the picker and the history search):

```
[ai.sessions]
mouse = false
```

- Forks of a session (including Claude Code `--resume` copies) are grouped under it. Inspect (`Ctrl`+`O`) lists them, and `C` there expands the list.
- Subagents aren't listed at all, as they can't be resumed. A search that matches something a subagent said still finds the session it worked for, and `atuin ai resume <subagent-id>` resumes that session. Atuin records whether a child session is a subagent, a fork or a continuation as it captures it. Sessions captured by older versions don't say, and for Codex and opencode nearly all such children are subagents, so the picker treats them as such (they still resume by id).

`Enter` resumes the selected session in its own agent (or, without `enter_accept`, puts the command on your command line), and `Tab` puts the command on your command line to edit first. `Ctrl`+`Y` copies it.

If the session was recorded on another machine, or its transcript was deleted, Atuin first writes the transcript back out from the synced messages (Inspect says "from sync"). It resumes in the directory the session ran in when that exists here. Otherwise, when you're in a checkout of a repository with the same name, it resumes in the same place in that checkout (or at its root), and otherwise in the current directory. The picker says which.

If the session's own agent can't resume it here (a Copilot session, an agent that isn't installed, or a session whose transcript is here but whose directory is gone, for an agent that needs it), the picker says why and stays open; Inspect says so too.

From the command line, `atuin ai resume <id>` resumes a session directly (an id prefix works too), writing it out from sync first when it isn't on this machine. `--print` prints the command instead of running it.

## Settings

The picker follows the history search's `style`, `invert`, `show_preview`, `max_preview_height`, `enter_accept`, `keymap_mode` and theme settings. Its own go under `[ai.sessions]`:

```
[ai.sessions]
## The filter the picker opens in: "workspace" (the default, widening to "global"
## outside a git repository or when the workspace has no sessions), "global",
## "host", "directory" or "branch". `--filter-mode` overrides it.
filter_mode = "workspace"

## Height of the inline picker; 0 for fullscreen. Defaults to the top-level
## inline_height. `--inline-height` overrides it.
inline_height = 40

## Take the mouse (see above). Defaults to the top-level no_mouse.
mouse = true
```

To resume an agent's sessions with a command of your own in place of the built-in one, give a template per agent under `[ai.sessions.resume]` (`claude`, `codex`, `opencode` or `pi`). It runs after `cd -- <cwd> &&`, with these substituted:

- `{id}`: the agent's own id for the session;
- `{path}`: the session's transcript on this machine (for a session written back out from sync, the one Atuin wrote);
- `{cwd}`: the directory the session worked in.

```
[ai.sessions.resume]
claude = "claude --resume {id} --model opus"
pi = "pi --session {path}"
```

Each value is substituted within its word, unquoted, so don't quote them, and don't pass them through another shell (`sh -c "..."`), which would split and expand them. The program is looked up on `PATH` only: shell aliases, functions and `~/` aren't expanded, so give a full path for a program that isn't on `PATH`.

## Rebuilding the index

The index is derived entirely from the record store. If it ever looks wrong or incomplete, rebuild it:

```
atuin store rebuild ai-session
```

The daemon deletes the index and replays every session record into it in the background (`atuin ai session` waits for it meanwhile), so this needs the daemon enabled. `atuin ai resume` doesn't wait: it stays usable, and its status row says how far the rebuild has got, as its results may be incomplete until it finishes. `atuin store purge` and `atuin store pull --force` do the same after deleting records, and commands that re-encrypt the records (`atuin store rekey`, `atuin login`) have the daemon replay them the next time it starts.

If `atuin ai resume` reports that the session database is at an older schema version, the running daemon is an older Atuin than the command: restart it (`atuin daemon restart`) so it upgrades the database. If it reports that the session database was made by another build of Atuin (a development build, say), the daemon deletes it and rebuilds it from the record store when it starts: restart it (`atuin daemon restart`), and sessions reappear as the rebuild replays them.
