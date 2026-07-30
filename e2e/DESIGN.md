# atuin end-to-end testing — design

**Date:** 2026-07-29
**Status:** Foundation implemented (`e2e/`); staged roadmap below.

## Goal

Test atuin the way a user actually uses it — **real shells with hooks installed,
driven through a PTY, plus multi-client sync through a real server** — covering
the surface the existing Rust tests can't reach.

### What already exists (and what this does *not* duplicate)

- `crates/atuin/tests/` — spins up a real `atuin-server` in-process and drives it
  with the `atuin-client` library. Covers the client↔server protocol at the
  *library* level. We do not re-test this.
- `test-behavior.sh` — a bespoke bash script that stands up real binaries + daemon
  + server and syncs between two sandboxed clients (bug #3627). This suite is that
  idea, generalized and maintainable.

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Surface | Both layers: shell/TUI **and** binary/sync | The gap Rust tests leave |
| Shells | bash, zsh, fish, nushell | Each is a distinct hook integration |
| Language | Python: `pytest` + `pexpect` + `pyte` | Best-in-class PTY driving; `pyte` gives robust screen assertions |
| Containers | `testcontainers-python` manages **everything** — shells + server | Version-pinned matrix; containers are inherently sandboxed |
| Binary build | Compiled **inside** the base image | Host is macOS, containers are Linux — avoids cross-compilation |
| Server backend | sqlite by default; Postgres optional | Self-contained; prod-parity is a container swap |
| Isolation | Inherent (containers have no access to host `~/.local/share/atuin`) | Removes the real-DB-corruption hazard entirely |

## Architecture

```
host: pytest ──testcontainers──┬─ server container (atuin-server, sqlite)
        │                      ├─ client-A container (shell + atuin + daemon)
        │                      └─ client-B container (shell + atuin + daemon)
        └─ pexpect ─docker exec -t─▶ shell PTY ─▶ pyte Screen ─▶ assertions
```

- **Images** (`images/Dockerfile.*`): a base image compiles `atuin` (features
  `client,sync,daemon,ai,pty-proxy` — no clipboard/check-update) and
  `atuin-server`; per-shell images add a shell and an rc that runs `atuin init`
  and pins a **prompt sentinel** (`ATUIN_E2E> `) the harness synchronises on.
- **`harness/`**: `images` (build), `containers` (server + client wrappers on a
  shared network), `session.PtyShell` (pexpect + pyte), `atuin` (register/login/
  sync/search + per-shell adapters).
- **Assertions**: public surface first (`atuin search --cmd-only`); pyte screen
  for the TUI; read-only DB / daemon socket only when the surface can't show it.

### Key mechanics

- Clients reach the server at `http://server:8888` via a network alias.
- Clients share an account key by reading the client `key` file (no `atuin key`
  command exists) and passing it to `atuin login -k`.
- `PtyShell` runs in bytes mode and mirrors output into `pyte` via pexpect's
  `logfile_read`, so `expect` handles synchronisation while `pyte` handles
  "what's on screen".

## Staged roadmap

1. **Walking skeleton (this PR):** zsh + bash; capture test + two-client sync
   test; base + per-shell images; harness; server fixture; CI workflow.
2. TUI `Ctrl-R` test (present, marked `tui`); harden bash-preexec capture.
3. fish + nushell adapters; dotfiles/KV sync; #3627 as a named regression test.
4. CI matrix hardening; image build caching (cargo-chef / BuildKit cache mounts).

## Known risks

- **PTY-over-`docker exec`** is the main flakiness surface (double-PTY, prompt
  sync, ratatui redraws). Mitigated by prompt sentinels + pyte parsing. Fallback:
  run `pytest` inside each container.
- **nushell** install pulls a release binary by arch and its prompt handling is
  fiddly — treated as unverified until it stabilises.
- **Image build time**: full compile on first run / source change. Acceptable for
  now; caching is roadmap item 4.
