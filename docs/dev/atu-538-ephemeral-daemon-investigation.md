# ATU-538 — Ephemeral Daemon Investigation & Cutover Plan

> **Status:** Investigation complete — recommendation and phased implementation plan below.
> **Issue:** ATU-538 "Investigate whether making daemonless mode just spawn an ephemeral daemon is possible."
> **Project:** Completely Cutover to Daemon.
>
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement the plan in Part 3 task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Do not begin implementation before the design decisions in Part 2 are ratified by a maintainer** — this is an investigation deliverable, and the plan is contingent on those decisions.

**Goal of the investigation:** Determine whether the daemonless code path can be eliminated by having the client transparently spawn/talk to a daemon, so we stop maintaining two storage code paths.

**One-line verdict:** **Yes, it is feasible — and most of the mechanism already exists.** The autostart feature already does `fork`/`spawn`-into-daemon and waits for readiness. The real work is not "make spawning possible" (it is), but "collapse the branch so the daemon path is the *only* write path," plus deciding the fallback story for environments where a daemon genuinely cannot run. A *truly* ephemeral (spawn → serve one request → die) daemon is **not** recommended; a **spawn-on-demand, persistent** daemon is.

---

## Part 1 — What exists today (grounded architecture map)

### 1.1 There are two independent axes, not one switch

The word "daemonless" hides two separate decisions:

| Axis | Gate | Where |
|---|---|---|
| **Write path** (record `start`/`end`) | runtime `settings.daemon.enabled` | `crates/atuin/src/command/client/history.rs:577`, `:593` |
| **Read path** (interactive TUI search only) | `settings.search_mode == SearchMode::DaemonFuzzy` | `crates/atuin/src/command/client/search/engines.rs:17-29` |

Both are additionally behind the compile-time `#[cfg(feature = "daemon")]` gate. **Non-interactive `atuin search <query>` never uses the daemon at all** — it always queries local SQLite (`crates/atuin/src/command/client/search.rs:310-342`).

This decoupling matters: "cutover to daemon" is really "collapse the *write* axis," because the read axis already has a sane story (see §1.4).

### 1.2 The write path — the actual dual-maintenance burden

`crates/atuin/src/command/client/history.rs`:

```rust
// start_history_entry — history.rs:570-584
#[cfg(feature = "daemon")]
if settings.daemon.enabled {
    return handle_daemon_start(settings, command, author, intent).await; // gRPC to daemon
}
let db = Sqlite::new(db_path, settings.local_timeout).await?;             // daemonless: open DB directly
handle_start(&db, settings, command, author, intent).await

// end_history_entry — history.rs:586-610
#[cfg(feature = "daemon")]
if settings.daemon.enabled {
    return handle_daemon_end(settings, id, exit, duration).await;        // gRPC to daemon
}
// daemonless: open Sqlite + SqliteStore + HistoryStore, then...
handle_end(&db, store, history_store, settings, id, exit, duration).await
```

The daemonless `handle_end` (`history.rs:485-551`) does: `db.update()` → `history_store.push()` → **inline sync** (`history.rs:530-548`). The daemon replicates the exact same `db.save` / `history_store.push` server-side (`crates/atuin-daemon/src/components/history.rs:184-248`) and moves sync to a **background timer** (`crates/atuin-daemon/src/components/sync.rs`). So the two paths differ in *when sync happens* — a semantic difference the cutover must preserve or consciously change.

### 1.3 The daemon can already be spawned on demand — this is the crux

The autostart infrastructure in `crates/atuin/src/command/client/daemon.rs` **already implements the issue's proposed `fork+execve` idea**:

- `spawn_daemon_process()` (`daemon.rs:253-269`): re-execs the current binary as `atuin daemon start` (+ `--daemonize` on unix), stdio nulled.
- `ensure_daemon_running()` (`daemon.rs:355-389`): takes a cross-process startup lock, probes, removes a stale socket, spawns, then `wait_until_ready()`.
- `try_with_restart()` (`daemon.rs:419-459`): on a retriable error and if `settings.daemon.autostart`, restarts and retries. Already wraps `start_history`/`end_history`/`cancel_history`.
- Cold-start is serialized against thundering-herd shell hooks by a pidfile + `.startup.lock` (`daemon.rs:359-361`, `wait_for_pidfile_available` at `:379`).

**Conclusion:** the question "is it *possible* to spawn a daemon and talk to it ephemerally?" is already answered by shipping code. Autostart is that mechanism, minus the "it stays resident afterward" part.

### 1.4 The read path already depends on local SQLite regardless

Even in `DaemonFuzzy` search mode, the daemon only returns **ranked UUIDs** from its in-memory index; the client still hydrates full rows from local SQLite (`crates/atuin/src/command/client/search/engines/daemon.rs:104-112`, `full_query` at `:118-228`), and falls back to `db.search` for regex. So the client *always* links and opens SQLite for reads. This means removing the daemonless *read* path is neither necessary nor clearly beneficial — keeping local reads is fine. **The cutover should focus on writes.**

### 1.5 Lifecycle & platform facts that constrain the design

- **No idle/ephemeral shutdown exists.** The daemon runs until `ShutdownRequested`/SIGTERM (`crates/atuin-daemon/src/daemon.rs:297-322`). There is no "serve one request then exit."
- **Windows is a first-class target** (`dist-workspace.toml`, CI matrix `windows-latest`). It has **no `fork`/daemonize** — autostart just `cmd.spawn()`s a detached `atuin daemon start` and the daemon listens on **TCP loopback** `127.0.0.1:8889` (`crates/atuin-daemon/src/server.rs:120-170`, `client.rs` `#[cfg(not(unix))]` blocks). So "fork+execve" is unix-specific framing; the portable primitive is "spawn a detached child process."
- **macOS** has no systemd; it relies on autostart or a manual launch (no launchd plist in-repo).
- **systemd socket activation** exists on Linux (`server.rs:32-67`) and is *incompatible* with autostart (`ensure_autostart_supported` bails, `daemon.rs:335-346`). Any always-on-daemon design must keep deferring to systemd when `daemon.systemd_socket = true`.

---

## Part 2 — Design analysis & recommendation

### 2.1 Option A — Truly ephemeral daemon (spawn → serve → die). NOT recommended.

Spawn a fresh daemon per shell command (or per burst), serve the request, exit when idle.

- **Cost:** every `history start`/`end` hook (i.e. every shell command) pays process-spawn + gRPC-connect + component-boot latency. Even with a short idle window, interactive shells fire these constantly; you'd thrash spawn/teardown.
- **Loses the daemon's whole point:** the in-memory fuzzy index and the background sync loop only pay off if the process persists across commands. An ephemeral daemon rebuilds/dumps that state every time.
- **Verdict:** technically possible, practically worse than both current modes. Reject.

### 2.2 Option B — Spawn-on-demand, *persistent* daemon (RECOMMENDED)

Keep exactly one resident daemon, spawned lazily on first need, reused thereafter — i.e. make autostart's behavior the default and remove the daemonless write branch.

- The daemon becomes the single write path. The daemonless `handle_start`/`handle_end` inline-DB logic is deleted (or demoted to a narrow fallback — see §2.4).
- Sync semantics converge on the daemon's background loop (a deliberate, documented change from inline-on-every-`end`).
- Reuses all the hardening already built: startup lock, stale-socket cleanup, version negotiation, retry-on-restart.
- "Ephemeral" in the issue's spirit is satisfied by *spawn-on-demand*: users who never opened a daemon still get one transparently, without adopting systemd.

**This is the recommendation.** The rest of the plan implements Option B.

### 2.3 The load-bearing open questions (need maintainer ratification before coding)

1. **Fallback policy.** `atuin history start/end` runs on *every shell command*. If the daemon cannot be spawned (locked-down container, no fork, exhausted PIDs, read-only runtime dir), do we:
   - **(2a)** fall back to the current direct-DB write (keep `handle_start`/`handle_end` alive as a fallback only), or
   - **(2b)** fail the hook (history silently not recorded)?

   Recommendation: **(2a) keep the direct-DB path as an explicit, last-resort fallback**, not as a co-equal mode. This retains reliability while removing it from the *happy path*. It does **not** fully eliminate the code — but it removes the *config-driven* dual maintenance and lets the fallback be tested as one narrow branch. If the goal is truly deleting the code, that is **(2b)**, which is riskier and should be a follow-up once telemetry shows autostart is reliable in the wild.
2. **Default flip.** Do we flip `daemon.enabled`/`daemon.autostart` defaults to `true`, or keep them opt-in for a release and only change the *plumbing*? Flipping defaults changes behavior for every existing user (first command in a shell now spawns a daemon). Recommendation: land the plumbing first (Phase 1-3) behind existing config, then flip defaults in a separate, well-announced release (Phase 4).
3. **Sync latency change.** Moving from inline-per-`end` sync to the background loop means the last command before a shell exits may not sync immediately. Acceptable? (The daemon already behaves this way for autostart users; this just makes it universal.) Recommendation: accept and document.
4. **Non-interactive `atuin search` in scripts/CI.** Should a one-off `atuin search` in a script spawn a daemon? Today it never does (pure local read). Recommendation: **leave non-interactive search local** — it is a read, §1.4 shows reads always use local SQLite anyway, and spawning a daemon for a one-shot scripted query is pure overhead.

### 2.4 What "unification" concretely reduces to

After Option B, the only genuinely dual-maintained logic left is the §2.3-item-1 fallback. Everything else collapses:

- `history.rs:577`/`:593` branch → single `ensure_daemon_running` + daemon RPC (with optional fallback).
- Autostart stops being an opt-in feature and becomes the mechanism the write path always uses.
- The daemon `search` engine and local `db` engine remain as-is (read axis unchanged, §1.4).

---

## Part 3 — Phased implementation plan (contingent on §2.3 ratification)

> This plan implements **Option B with a §2.3-item-1(2a) fallback**: the daemon becomes the default write path, spawned on demand; the direct-DB write survives only as an explicit last-resort fallback. Each task ends with an independently testable deliverable. TDD throughout: write the failing test, watch it fail, implement, watch it pass, commit.

### Global Constraints

- **Rust edition/toolchain:** as pinned by `rust-toolchain.toml` (currently 1.97). Do not bump.
- **Must compile with `--no-default-features` and with the `daemon` feature off.** All new daemon-path code stays behind `#[cfg(feature = "daemon")]`; the fallback path must remain the code that compiles when `daemon` is disabled.
- **Windows must keep building and passing CI** (`windows-latest`). Never assume `fork`/unix sockets; use the existing `#[cfg(unix)]` / `#[cfg(not(unix))]` split (TCP on Windows).
- **Never break the shell hook.** `history start`/`end` errors must stay swallowed/non-fatal exactly as today (`history.rs` already logs-and-continues).
- **Respect `daemon.systemd_socket = true`:** never autostart in that mode (`ensure_autostart_supported`, `daemon.rs:335-346`); defer to systemd.
- **Preserve `store_failed` semantics:** non-zero exit with `store_failed = false` must cancel/delete the in-flight entry, in both daemon and fallback paths (`history.rs:560-565`, `:511-518`).
- Conventional-commit messages; frequent commits; one deliverable per task.

---

### Task 1: Introduce a single "resolve the write path" seam (no behavior change)

Refactor `start_history_entry`/`end_history_entry` so the daemon-vs-fallback decision lives in one function instead of being inlined at two call sites. Pure refactor; behavior identical; this is the seam every later task edits.

**Files:**
- Modify: `crates/atuin/src/command/client/history.rs:570-610`
- Test: `crates/atuin/src/command/client/history.rs` (inline `#[cfg(test)]` module) or `crates/atuin/tests/` if one exists.

**Interfaces:**
- Produces: `async fn write_backend(settings: &Settings) -> WriteBackend` where `enum WriteBackend { Daemon, Direct }`, chosen today by `cfg!(feature="daemon") && settings.daemon.enabled`. `start_history_entry`/`end_history_entry` match on it.

- [ ] **Step 1: Write the failing test** — assert that with `daemon.enabled = false`, `write_backend(&settings)` returns `WriteBackend::Direct`; with `daemon.enabled = true` (and feature on), returns `WriteBackend::Daemon`.

```rust
#[tokio::test]
async fn write_backend_follows_daemon_enabled() {
    let mut settings = Settings::utils_test_config();
    settings.daemon.enabled = false;
    assert_eq!(write_backend(&settings).await, WriteBackend::Direct);
    #[cfg(feature = "daemon")]
    {
        settings.daemon.enabled = true;
        assert_eq!(write_backend(&settings).await, WriteBackend::Daemon);
    }
}
```

- [ ] **Step 2: Run it, confirm it fails** — `cargo test -p atuin --features daemon write_backend_follows_daemon_enabled` → FAIL (`write_backend` not found). (Check whether a `Settings::utils_test_config`-style helper exists; if not, construct `Settings` via the existing test pattern in `atuin-client`.)
- [ ] **Step 3: Implement `WriteBackend` + `write_backend()`** and rewrite the two entry points to `match` on it, calling the *existing* `handle_daemon_*` / `handle_*` bodies unchanged.
- [ ] **Step 4: Run test + full build both feature states** — `cargo test -p atuin --features daemon write_backend...` PASS; `cargo build -p atuin --no-default-features` and `cargo build -p atuin --features daemon` both succeed.
- [ ] **Step 5: Commit** — `refactor(history): centralize daemon-vs-direct write backend selection`

---

### Task 2: Make the daemon write path autostart-on-demand by default

Ensure that when `WriteBackend::Daemon` is selected, a missing daemon is spawned transparently (not only when `autostart` was explicitly set). This is the "spawn ephemeral daemon" behavior.

**Files:**
- Modify: `crates/atuin/src/command/client/daemon.rs` (the `try_with_restart` / `start_history` / `end_history` wrappers around `daemon.rs:419-498`)
- Modify: `crates/atuin/src/command/client/history.rs` (`handle_daemon_start`/`handle_daemon_end`)

**Interfaces:**
- Consumes: `ensure_daemon_running(settings)` (`daemon.rs:355`), `try_with_restart` (`daemon.rs:419`).
- Produces: daemon write RPCs that, on a connect/unavailable error, call `ensure_daemon_running` and retry **whenever the daemon write backend is in use**, independent of the legacy `autostart` opt-in (still gated off for `systemd_socket`).

- [ ] **Step 1: Write the failing test** — a unit test around the retry decision: given a first call that returns a `DaemonClientErrorKind::Connect` error and `systemd_socket = false`, the wrapper attempts `ensure_daemon_running` exactly once and retries. Use a small injected closure/fake so no real socket is needed (mirror the existing `try_with_restart` signature which already takes a closure).

```rust
#[tokio::test]
async fn daemon_write_spawns_on_connect_error() {
    // fake sender: first call -> Connect error, second -> Ok
    // assert ensure-hook invoked once, final result Ok
}
```

- [ ] **Step 2: Run it, confirm it fails** — `cargo test -p atuin --features daemon daemon_write_spawns_on_connect_error` → FAIL.
- [ ] **Step 3: Implement** — thread a "may autostart" decision that is `true` for the daemon write backend (except `systemd_socket`), so `try_with_restart` no longer requires `settings.daemon.autostart` for the write path specifically. Keep `ensure_autostart_supported`'s systemd guard.
- [ ] **Step 4: Run test + `cargo clippy -p atuin --features daemon -- -D warnings`** → PASS/clean.
- [ ] **Step 5: Commit** — `feat(daemon): autostart daemon on demand for the write path`

---

### Task 3: Add the explicit last-resort fallback (§2.3-item-1(2a))

When the daemon write backend is selected but the daemon cannot be spawned (spawn fails or never becomes ready within `startup_timeout`), fall back to the direct-DB write instead of dropping the record — and log a one-time-ish warning.

**Files:**
- Modify: `crates/atuin/src/command/client/history.rs` (`handle_daemon_start`/`handle_daemon_end` error handling)

**Interfaces:**
- Consumes: existing `handle_start`/`handle_end` (the direct-DB writers) and `Sqlite`/`SqliteStore`/`HistoryStore` construction already present in `end_history_entry` (`history.rs:597-607`).
- Produces: `handle_daemon_start`/`handle_daemon_end` that, on an unrecoverable daemon error, delegate to the direct writers and return `Ok`.

- [ ] **Step 1: Write the failing test** — simulate `ensure_daemon_running` failing (e.g. point `socket_path`/`pidfile` at an unwritable/uncreatable location, or inject a failing spawn hook) and assert the record is still persisted to the local `Sqlite` DB afterward. Read it back with `db.load(id)`.
- [ ] **Step 2: Run it, confirm it fails** — `cargo test -p atuin --features daemon daemon_write_falls_back_to_direct` → FAIL (record absent).
- [ ] **Step 3: Implement the fallback** — wrap the daemon RPC; on terminal failure, `warn!` and call the direct writer. Preserve `store_failed`/exit-code cancel semantics in the fallback branch.
- [ ] **Step 4: Run test + both-feature builds** — PASS; `--no-default-features` still builds (fallback body is the code that must survive with `daemon` off).
- [ ] **Step 5: Commit** — `feat(history): fall back to direct DB write when daemon unavailable`

---

### Task 4: Documentation — reference + migration note

Document the new behavior: the daemon is the blessed write path, spawned on demand; the direct write is a fallback; sync moves to the background loop for these users.

**Files:**
- Modify: `docs/docs/reference/daemon.md`
- Modify: `docs/docs/reference/config.md` (or wherever `daemon.*` keys are documented) to describe the new default/behavior of `daemon.enabled`/`daemon.autostart`.
- Modify: this file's Part 2 open questions → record the ratified decisions.

- [ ] **Step 1:** Update `daemon.md`: explain spawn-on-demand, the systemd-socket exception, and the fallback.
- [ ] **Step 2:** Note the sync-timing change (inline → background) and its user-visible effect.
- [ ] **Step 3:** Build the docs site if tooling is available (`mkdocs build` under `docs/`), else visually review the Markdown.
- [ ] **Step 4: Commit** — `docs(daemon): document spawn-on-demand write path and fallback`

---

### Task 5 (SEPARATE RELEASE — gated on §2.3-item-2): Flip defaults

**Do not bundle with Tasks 1–4.** Once telemetry/dogfooding shows spawn-on-demand is reliable, flip `daemon.enabled` (and/or `autostart`) defaults to `true` so new users get the daemon path without config.

**Files:**
- Modify: `crates/atuin-client/src/settings.rs:1583-1584` (`daemon.enabled` / `daemon.autostart` defaults)

- [ ] **Step 1: Write/adjust the failing test** — assert the default `Settings` now reports the daemon write backend on a platform where autostart is supported.
- [ ] **Step 2:** Confirm it fails against current defaults.
- [ ] **Step 3:** Flip the `set_default` values.
- [ ] **Step 4:** Full test suite `cargo test --workspace`; verify Windows CI green.
- [ ] **Step 5: Commit** — `feat(daemon)!: default to daemon write path (spawn on demand)` (breaking-change footer; changelog + release-notes call-out).

---

## Part 4 — Risks & mitigations (summary)

| Risk | Mitigation |
|---|---|
| Daemon can't spawn in restricted envs | Task 3 direct-DB fallback keeps history recording working |
| Cold-start latency on first shell command | Existing startup lock + `wait_until_ready`; latency paid once per daemon lifetime, not per command |
| Windows (no fork) | Reuse existing detached-`spawn` + TCP path; keep CI on `windows-latest` |
| systemd users double-managing the daemon | Keep `ensure_autostart_supported` guard; never autostart when `systemd_socket = true` |
| Sync no longer inline-per-command | Documented behavior change (Task 4); matches what autostart users already experience |
| Scripted one-off `atuin search` spawning daemons | Leave non-interactive search local (§2.3-item-4) |

## Part 5 — Recommendation to the maintainer

1. **Adopt Option B** (spawn-on-demand persistent daemon), not a truly ephemeral one.
2. **Ratify the four §2.3 decisions** (fallback policy, default-flip timing, sync-latency acceptance, non-interactive search stays local).
3. **Land Tasks 1–4 first** (plumbing + fallback + docs) behind current config, then **Task 5** (default flip) as a separate, announced release.
4. The daemonless *read* path stays — it is not a maintenance problem and reads always use local SQLite anyway.
