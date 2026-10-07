# Agent sessions

With `capture_sessions = true` under `[ai]`, the Atuin daemon records the sessions of your AI coding agents (Claude Code, Codex, opencode and Pi) into the encrypted record store, and syncs them like your history. `atuin ai session` lists, shows, searches and follows them.

The daemon keeps a local search index of these sessions beside the record store, and brings it up to date as sessions are captured, synced in from your other machines, or imported.

## What's captured

Capture keeps the conversation (what you and the agent said) and its tool calls: each call's input and output (the commands run and what they printed, the files read and written). A session resumed from them, on this machine or another, has its tool calls back as they were, and so does a session continued in another agent (see [Resuming](#resuming)). Reasoning is kept only as a marker that the model reasoned.

When an agent edits a file, capture also keeps the diff the agent recorded for the edit (Claude Code, Codex, opencode and pi each keep one), so a resumed session shows its edits as the agent showed them, and `atuin ai session show` prints them. A diff names its file and the lines changed, with a few lines around them, not the whole file. Search covers the names of the files a session changed.

Secrets Atuin recognises are replaced with `****`, in the conversation and the tool calls alike: the [same patterns](https://docs.atuin.sh/configuration/config/#secrets_filter) as for captured command output, and your own [`redact_patterns`](https://docs.atuin.sh/configuration/config/#redact_patterns). Images are left out, and the input or output of a single call is clipped to 64 KiB, keeping its start and its end. An edit's diff past 64 KiB keeps only the files it changed. Search covers a call's input, but not its output.

The rules that keep things out of your history and your captured output apply to your agents' tool calls too:

- A command [`history_filter`](https://docs.atuin.sh/configuration/config/#history_filter) excludes, or one [`secrets_filter`](https://docs.atuin.sh/configuration/config/#secrets_filter) recognises a credential in, is kept as the name of its tool only, without its input or output. Every call in a directory [`cwd_filter`](https://docs.atuin.sh/configuration/config/#cwd_filter) excludes is kept the same way.
- A command whose output is never stored is kept without its output: one [`command_filter`](https://docs.atuin.sh/configuration/config/#command_filter) matches, one of Atuin's own credential commands (such as `atuin key` and `atuin login`), or another tool's that prints a credential, such as `gh auth token` or `kubectl get secret`.
- A file holding credentials (`.env`, `~/.ssh/id_ed25519`, `~/.aws/credentials`, any of your [`sensitive_files`](https://docs.atuin.sh/configuration/config/#sensitive_files)) is never captured: a call that reads one, or a command naming one, is kept without its output, and a call that writes to one without what it wrote.

Text that can't be redacted quickly isn't kept. To keep more out, see [Keeping Secrets Out of Atuin](https://docs.atuin.sh/guide/excluding-commands/index.md).

Tool calls can be a lot of your codebase, not just your conversations. To keep only the name of each tool called (and whether the call failed), and no diffs, set `capture_tools = false` under `[ai]`:

```
[ai]
capture_sessions = true
capture_tools = false
```

These settings, and the filters above, apply to sessions captured or imported after the daemon restarts. Sessions already captured keep what they had.

## Searching

```
atuin ai session search "flaky test"
```

- Each word of the query matches a whole word: `atuin ai session search "migration"` finds sessions mentioning migration, but `"migr"` doesn't. The `atuin ai resume` picker searches as you type, so there the last word also matches as a prefix once it's at least two characters long.
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

- Forks of a session (including Claude Code `--resume` copies, and continuations in another agent) are grouped under it. Inspect (`Ctrl`+`O`) lists them, and `C` there expands the list.
- Subagents aren't listed at all, as they can't be resumed. A search that matches something a subagent said still finds the session it worked for, and `atuin ai resume <subagent-id>` resumes that session. Atuin records whether a child session is a subagent, a fork or a continuation as it captures it. Sessions captured by older versions don't say, and for Codex and opencode nearly all such children are subagents, so the picker treats them as such (they still resume by id).

Once you choose a session, with `Enter` (or `Tab` to edit the command first), Atuin asks where to resume it:

```
╭ Resume in ──────────────────────────────────────────────────────────────────╮
│ > 1 CC Claude Code  original                                                │
│   2 CC Claude Code  fork: new session, same history                         │
│   3 CX Codex        continue, 42 tool calls become notes, reasoning dropped │
│   4 OC opencode     continue, 42 tool calls become notes, reasoning dropped │
│ <enter>: resume  <tab>: edit  <esc>: back                                   │
╰─────────────────────────────────────────────────────────────────────────────╯
```

- The session's own agent comes first and is already selected, so `Enter` `Enter` resumes it. If the session was recorded on another machine, or its transcript was deleted, Atuin writes the transcript back out from the synced messages before resuming it ("from sync").
- Right under it, forking writes the session out as a new session of the same agent, with the same history, tool calls and all, and resumes that. The original is left as it is. The fork is linked to it the way the agent links its own forks (Claude Code's `forkedFrom`, Codex's `forked_from_id`, Pi's `parentSession`; opencode gets a line it keeps from the model), so it's grouped under the original, with an Atuin id of its own. It keeps the original's title. Pi names the original by its file, so a Pi session that isn't on this machine is written out first. Forking is selected instead of the original when the original can't resume here, or its agent has it open on this machine. Copilot sessions, and sessions with no messages, can't be forked.
- Every other agent installed on this machine follows. Picking one continues the session there as a new session: the conversation carries over, and so do the tool calls captured with their input and output (`capture_tools`), with what they returned. Each becomes the new agent's own tool where it has one that does the same (a shell command, reading, writing or editing a file, a search), and stays the tool it was otherwise: the agent reads it, it just can't call it again. Calls captured without their input, calls that never got a result, and web searches the model's provider ran itself become notes in the text, and reasoning is dropped. The line says how much. The new session opens with a note to the agent naming the session it continues, by its own agent's id and its Atuin id (in Pi, the note opens the first prompt, so you see it too). Agents that aren't installed aren't listed, and with nothing else to offer (no other agent installed, and a session that can't be forked) `Enter` resumes straight away.
- If the session's own agent can't resume it here (a Copilot session, an agent that isn't installed, or a session whose transcript is here but whose directory is gone, for an agent that needs it), its line is dimmed with the reason, and the next one is selected instead.

A session keeps one id on every machine. When you resume one in its own agent and its transcript is on this machine, Atuin first catches it up with the synced messages:

- If the session continued on another machine since, the messages this copy lacks are appended to it, and it resumes in place. The status line says `caught up: 12 messages from @3f9a12bc`.
- If this copy is up to date, or went on past the newest messages along the same branch, it resumes unchanged.
- Otherwise Atuin writes nothing and asks, with the chooser saying why: when the agent is running this session here (`Claude Code is running this session here`), when this copy went another way than the newest messages, when it has messages sync hasn't got, or when this copy can't be caught up (`couldn't catch up: ...`). Its first line resumes this copy unchanged, and a fork line for each branch of the session, newest first, forks from that branch (`fork @3f9a12bc's · +11 since they split · 20m ago`). With the agent running here, the newest fork is selected.
- When this copy went another way, sync holds every message of it, and the agent isn't running it here, a switch line for each other branch comes after the first line (`switch to @3f9a12bc's · replaces this copy, yours stays in atuin`). It moves this copy onto that branch, in place, under the same session id, and resumes it. The history this copy shares with that branch stays exactly as it was, and the messages this copy went on with are taken out. The branch's messages since the two split are added from sync, so, like a restore, they carry what Atuin keeps of them: the conversation and which tools were called, but not the other machine's tool input and output, or its images. Before anything changes, your copy is saved whole to `ai/switched/<agent>/` in Atuin's data directory, where no agent lists it, and the status line says where: `switched to @3f9a12bc's branch: 24 messages (your copy is at ...)`. Atuin never deletes these copies. The branch this copy was on stays in Atuin too, so forking it brings it back. Claude Code, Codex and Pi sessions can switch. These can only fork, and get no switch line: a Codex session that was reverted, so its history spans several files, or that Codex wrote in its older format; a Pi session file Pi hasn't opened since version 2 of its format; and opencode sessions, which live in a database.

Atuin never merges branches, and never writes to a session its agent has open. Claude Code, and Codex where it keeps session locks, say which session they have open. opencode, Pi and other Codex installs don't, so Atuin treats a session as possibly open, and doesn't catch it up, while one of that agent's processes works in the session's directory or below it: the directory this copy records, or else where the session works on this machine. A process elsewhere counts when its command line names the session (`opencode -s <id>`, `codex resume <id>`, `pi --session <file>`). While the session's transcript has changed in the last two minutes, any running process of that agent counts, wherever it works.

A session written back out from sync, or continued in another agent, resumes in the directory the session ran in when that exists here. Otherwise, when you're in a checkout of a repository with the same name, it resumes in the same place in that checkout (or at its root), and otherwise in the current directory. The picker says which.

In the chooser, `Up` / `Down` (or `K` / `J`) move, `F` moves to the fork, a digit picks that line, and `Esc` goes back to the list. `Enter` does what the key that opened the chooser did: it resumes (or, without `enter_accept`, puts the command on your command line), and after `Tab` it edits. `Tab` in the chooser always edits. `Ctrl`+`Y` copies the command.

`Alt`+`Enter` in the list or Inspect opens the chooser with the fork selected (`Shift`+`F` in vim's normal mode, `F` in Inspect). Some terminals keep `Alt`+`Enter` for themselves; `Enter` then `F` always works.

To resume in the session's own agent straight away, without the chooser:

```
[ai.sessions]
resume_chooser = false
```

The chooser still opens for a session its own agent can't resume.

From the command line, `atuin ai resume <id>` resumes a session in its own agent directly (an id prefix works too), writing it out from sync first when it isn't on this machine, and `atuin ai resume <id> --in codex` continues it in another (`claude`, `codex`, `opencode` or `pi`). `atuin ai resume <id> --fork` forks it. It catches the session up with sync as the picker does, and fails when that needs a choice, saying which: `--as-is` resumes this copy unchanged, `--switch` switches it to another branch, and `--fork` forks it from its newest branch. `--branch` picks the branch to catch up to, to switch to, or to fork from: `this`, `@<host id>`, or the start of the id the error lists. `--as-is`, `--switch` and `--branch` need an id that names a single session: when it names several, Atuin lists them and asks you to be more specific. `--print` prints the command instead of running it.

The id can be the agent's own, or the session's Atuin id, which Inspect and the MCP session tools show. An Atuin id is a UUIDv7, written as 32 hex digits like a history id. It's fixed when Atuin first records the session, and is the same for every agent and on every machine. Its first 12 digits are when the session started, so a prefix needs more digits than that to name a single session.

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
