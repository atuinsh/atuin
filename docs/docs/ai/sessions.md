# Agent sessions

With `capture_sessions = true` under `[ai]`, the Atuin daemon records the
sessions of your AI coding agents (Claude Code, Codex, opencode and Pi) into the
encrypted record store, and syncs them like your history. `atuin ai session`
lists, shows, searches and follows them.

The daemon keeps a local search index of these sessions beside the record
store, and brings it up to date as sessions are captured, synced in from your
other machines, or imported.

## Searching

```shell
atuin ai session search "flaky test"
```

- The last word of the query matches as a prefix once it's at least two
  characters long, so `atuin ai session search "migr"` finds sessions
  mentioning "migration." A single character still matches only a whole word.
  Earlier words always match whole.
- An empty query (`atuin ai session search ""`) lists the newest sessions
  first, rather than returning nothing.
- `--harness` limits the search to one agent.

## Resuming

`atuin ai resume` (or a key you bind to it, see
[Binding `atuin ai resume`](../reference/init.md#binding-atuin-ai-resume)) opens a picker over
your sessions. It works like the history search: type to search, ++ctrl+o++
inspects the selected session, and ++esc++ leaves.

No key opens it by default. To open it with ++ctrl+bracket-right++ (which
replaces the shell's own character search on that key), pass `--bind-ai-resume`
to `atuin init`; or bind the widget to a key of your choice after the
`atuin init` line:

=== "zsh"

    ```shell
    bindkey -M emacs '^]' atuin-ai-resume
    bindkey -M viins '^]' atuin-ai-resume-viins
    bindkey -M vicmd '^]' atuin-ai-resume-vicmd
    ```

=== "bash"

    ```shell
    atuin-bind -m emacs      '\C-]' atuin-ai-resume-emacs
    atuin-bind -m vi-insert  '\C-]' atuin-ai-resume-viins
    atuin-bind -m vi-command '\C-]' atuin-ai-resume-vicmd
    ```

=== "fish"

    ```fish
    bind ctrl-] _atuin_ai_resume
    bind -M insert ctrl-] _atuin_ai_resume
    ```

Through the widget, the command it picks lands in your shell, as with the
history search.

Each row shows when the session was last active, its agent, its title and its
message count (when there's room). ++ctrl+r++ cycles which sessions are listed:
those in the current repository (where the picker opens), all of them, this
machine's, the current directory's, and the current branch's.

Words in the query of the form `filter:value` narrow the search further, and show
as chips in the input:

- `agent:<name>` (or `a:<name>` for short): only that agent's sessions, with
  `claude-code` (or `claude`, `cc`), `codex` (`cx`), `opencode` (`oc`) or `pi`.
  ++alt+a++ cycles through them in turn, and back to all agents.
- `m:<model>`: only sessions whose model contains this, as in `m:opus`.
- `b:<branch>`: only sessions on this git branch.

Put a `\` before a word to search for it as text instead (`\b:main`).

The preview under the list starts with where the session ran, its repository,
branch and, for a session recorded on another machine, that machine (by the end
of its host id), and how many forks it has:

```
       atuin · feat/ai-sessions · @3f9a12bc · 2 forks
first  …
last   …
```

At first the preview shows a line or two of the first prompt, the match and the
last reply. Scroll it to read them in full, one after another: with the mouse
wheel over it, or with ++shift+down++ and ++shift+up++ (a line) and
++shift+page-down++ and ++shift+page-up++ (a page). ++alt++ works in place of
++shift++, for terminals that keep shift and the arrows for themselves. Scroll
back to the top for the overview again. On a wide terminal the same goes for the
pane beside the list, and in Inspect for the conversation. The wheel over the
list moves the selection, as in the history search, and a scrollbar shows when
there's more than fits.

While the picker has the mouse, your terminal can't select text the usual way.
Most terminals still select with ++shift++ held while you drag (++option++ in
iTerm2, ++fn++ in macOS Terminal). To leave the mouse to the terminal (the
top-level `no_mouse = true` does the same for both the picker and the history
search):

```toml
[ai.sessions]
mouse = false
```

- Forks of a session (including Claude Code `--resume` copies, and
  continuations in another agent) are grouped under it. Inspect (++ctrl+o++)
  lists them, and ++c++ there expands the list.

- Subagents aren't listed at all, as they can't be resumed. A search that
  matches something a subagent said still finds the session it worked for, and
  `atuin ai resume <subagent-id>` resumes that session. Atuin records whether a
  child session is a subagent, a fork or a continuation as it captures it.
  Sessions captured by older versions don't say, and for Codex and opencode
  nearly all such children are subagents, so the picker treats them as such
  (they still resume by id).

Once you choose a session, with ++enter++ (or ++tab++ to edit the command
first), Atuin asks where to resume it:

```
╭ Resume in ──────────────────────────────────────────────────────────────────╮
│ > 1 CC Claude Code  original                                                │
│   2 CX Codex        continue, 42 tool calls become notes, reasoning dropped │
│   3 OC opencode     continue, 42 tool calls become notes, reasoning dropped │
│ <enter>: resume  <tab>: edit  <esc>: back                                   │
╰─────────────────────────────────────────────────────────────────────────────╯
```

- The session's own agent comes first and is already selected, so
  ++enter++ ++enter++ resumes it. If the session was recorded on another
  machine, or its transcript was deleted, Atuin writes the transcript back out
  from the synced messages before resuming it ("from sync").
- Every other agent installed on this machine follows. Picking one continues
  the session there as a new session: the conversation carries over, but that
  agent can't replay the original's tool calls, so they become notes in the
  text, and reasoning is dropped. The line says how much. Agents that aren't
  installed aren't listed, and with no other agent installed there is nothing
  to choose, so ++enter++ resumes straight away.
- If the session's own agent can't resume it here (a Copilot session, a
  directory that's gone, an agent that isn't installed), its line is
  dimmed with the reason, and the next one is selected instead.

In the chooser, ++up++ / ++down++ (or ++k++ / ++j++) move, a digit picks that
line, and ++esc++ goes back to the list. ++enter++ does what the key that
opened the chooser did: it resumes (or, without `enter_accept`, puts the
command on your command line), and after ++tab++ it edits. ++tab++ in the
chooser always edits. ++ctrl+y++ copies the command.

To resume in the session's own agent straight away, without the chooser:

```toml
[ai.sessions]
resume_chooser = false
```

The chooser still opens for a session its own agent can't resume.

From the command line, `atuin ai resume <id>` resumes a session in its own
agent directly (an id prefix works too), and `atuin ai resume <id> --in codex`
continues it in another (`claude`, `codex`, `opencode` or `pi`). `--print`
prints the command instead of running it.

## Settings

The picker follows the history search's `style`, `invert`, `show_preview`,
`max_preview_height`, `enter_accept`, `keymap_mode` and theme settings. Its own
go under `[ai.sessions]`:

```toml
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

To resume an agent's sessions with a command of your own in place of the
built-in one, give a template per agent under `[ai.sessions.resume]` (`claude`,
`codex`, `opencode` or `pi`). It runs after `cd -- <cwd> &&`, with these
substituted:

- `{id}`: the agent's own id for the session;
- `{path}`: the session's transcript on this machine;
- `{cwd}`: the directory the session worked in.

```toml
[ai.sessions.resume]
claude = "claude --resume {id} --model opus"
pi = "pi --session {path}"
```

Each value is substituted within its word, unquoted, so don't quote them, and
don't pass them through another shell (`sh -c "..."`), which would split and
expand them. The program is looked up on `PATH` only: shell aliases, functions
and `~/` aren't expanded, so give a full path for a program that isn't on
`PATH`.

## Rebuilding the index

The index is derived entirely from the record store. If it ever looks wrong or
incomplete, rebuild it:

```shell
atuin store rebuild ai-session
```

The daemon deletes the index and replays every session record into it in the
background (`atuin ai session` waits for it meanwhile), so this needs the daemon
enabled. `atuin ai resume` doesn't wait: it stays usable, and its status row says
how far the rebuild has got, as its results may be incomplete until it finishes. `atuin store purge` and `atuin store pull --force` do the same after
deleting records, and commands that re-encrypt the records (`atuin store rekey`,
`atuin login`) have the daemon replay them the next time it starts.

If `atuin ai resume` reports that the session database is at an older schema
version, the running daemon is an older Atuin than the command: restart it
(`atuin daemon restart`) so it upgrades the database.
If it reports that the session database was made by another build of Atuin (a
development build, say), the daemon deletes it and rebuilds it from the record
store when it starts: restart it (`atuin daemon restart`), and sessions reappear
as the rebuild replays them.
