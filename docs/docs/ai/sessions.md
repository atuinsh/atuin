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
