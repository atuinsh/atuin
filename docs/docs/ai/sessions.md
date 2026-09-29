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

- The last word of the query matches as a prefix once it is at least two
  characters long, so `atuin ai session search "migr"` finds sessions
  mentioning "migration". A single character still matches only a whole word.
  Earlier words always match whole.
- An empty query (`atuin ai session search ""`) lists the newest sessions
  first, rather than returning nothing.
- `--harness` limits the search to one agent.

## Resuming

`atuin ai resume` (or ++ctrl+bracket-right++ at the prompt) opens a picker over
your sessions. It works like the history search: type to search, ++ctrl+o++
inspects the selected session, and ++esc++ leaves.

Each row shows when the session was last active, its agent, its title and its
message count (when there's room). The preview under the list starts with where
the session ran, its repository, branch and, for a session recorded on
another machine, that machine, and how many forks it has:

```
       atuin · feat/ai-sessions · @MacBook-Pro-3 · 2 forks
first  …
last   …
```

- Forks of a session (including Claude Code `--resume` copies, and
  continuations in another agent) are grouped under it. Inspect
  (++ctrl+o++) lists them, and ++c++ there expands the list. To give each fork
  a row of its own instead:

  ```toml
  [ai.sessions]
  group_forks = false
  ```

- Subagents aren't listed at all, as they can't be resumed. A search that
  matches something a subagent said still finds the session it worked for, and
  `atuin ai resume <subagent-id>` resumes that session. Codex and opencode
  don't record whether a child session is a subagent or a fork; nearly all of
  them are subagents, so the picker treats them as such (they still resume by
  id).

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
chooser always edits.

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

### Across machines

A session keeps one id on every machine. Each machine's copy of the transcript
is a branch of it, and the synced messages are the shared history, the way
git has a remote. Resuming never merges branches, and never writes the history
out again under a new id.

- **Fast-forward.** If the copy on this machine is behind (the session went
  on elsewhere since), Atuin appends the messages it's missing and resumes it
  in place. The status line says so: `caught up 136 messages from
  @MacBook-Pro-3`. With no copy here, it writes one out from sync.
- **Branches.** If the session went on separately on two machines, it has two
  branches. The preview and Inspect say `2 branches`, and the chooser lists
  them, newest first and already selected:

  ```
  ╭ Resume in ──────────────────────────────────────────────────╮
  │ 2 branches: it went on separately on several machines       │
  │ > 1 CC Claude Code  @MacBook-Pro-3 · 2h · 136 msgs (latest) │
  │   2 CC Claude Code  this machine · yest · 47 msgs           │
  │   3 CX Codex        continue                                │
  │ <enter>: resume  <tab>: edit  <esc>: back                   │
  ╰─────────────────────────────────────────────────────────────╯
  ```

  Picking a branch makes it the one the agent continues from, in the same
  session. The other branch stays in the transcript, untouched. opencode keeps
  a session as a single line, so a branch it can't take in place is continued
  as a new session linked to the original instead, and the status line says
  so. `atuin ai resume <id>` resumes the newest branch and names the others.
- **Still active elsewhere.** If another machine wrote to the branch in the
  last five minutes, it may still be running there, and resuming here would
  branch the session. Atuin asks first (`atuin ai resume <id>` asks on a
  terminal, and otherwise prints a note). Sync takes a little while to deliver
  messages, so a session active on another machine very recently may not show
  up as active yet.
- **Running here.** Atuin never writes to a transcript that an agent on this
  machine has open. If the copy here needs catching up, close the agent first.
  Pressing ++enter++ again resumes the copy as it is.

## Rebuilding the index

The index is derived entirely from the record store. If it ever looks wrong or
incomplete, schedule a full rebuild:

```shell
atuin store rebuild ai-session
```

The daemon replays every session record into the index the next time it
starts, so restart it (`atuin daemon restart`) to rebuild right away. Commands
that rewrite the record store, such as `atuin store rekey`, `atuin store purge`
and `atuin login`, schedule a rebuild themselves.

If `atuin ai resume` reports that the session database is at an older schema
version, the running daemon is an older atuin than the command: restart it
(`atuin daemon restart`) so it upgrades the database.
