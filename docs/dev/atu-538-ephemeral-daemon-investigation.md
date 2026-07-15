# ATU-538 — Removing the Daemonless Code Paths (Daemon-Only Client)

> **Status:** Investigation complete + **working proof-of-concept landed** (see Part 7). Full census, feasibility verdict, architecture, and phased plan below.
> **Issue:** ATU-538 "Investigate whether making daemonless mode just spawn an ephemeral daemon is possible."
> **Project:** Completely Cutover to Daemon.
> **Goal (as clarified by maintainer):** **Completely remove** the client's direct-to-SQLite and direct-to-record-store code paths. No config toggle, no fallback. The daemon becomes the sole owner of all local storage; the CLI becomes a thin RPC client that spawns the daemon on demand.
>
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement the plan in Part 4 task-by-task. Steps use checkbox (`- [ ]`) syntax. **This is a multi-release re-architecture, not a single PR** — do not attempt it in one shot, and get the Part 3 design decisions ratified before starting.

**One-line verdict:** **Feasible, and the right end-state — but it is a re-architecture, not a refactor.** "Just spawn a daemon" is already solved (autostart does `fork`+exec today). The real cost is that *removing direct storage access entirely* forces the daemon to serve the **complete** data API that ~25 subcommands currently satisfy by opening SQLite themselves — including full search-row hydration, stats, import bulk-insert, sync, key rotation, and the dotfiles/kv/scripts record stores. The tractable way to do this without rewriting every command is to **proxy the existing `Database`/`Store` traits over gRPC** so command handlers stay unchanged and only the ~6 construction sites and the trait backends move.

---

## Part 1 — The mechanism already exists; the hard part is the surface

### 1.1 Spawning a daemon on demand is a solved problem

The autostart infrastructure in `crates/atuin/src/command/client/daemon.rs` already implements the issue's "`fork+execve` into the daemon" idea:

- `spawn_daemon_process()` (`daemon.rs:253-269`) re-execs `atuin daemon start` (+ `--daemonize` on unix; detached `spawn` on Windows).
- `ensure_daemon_running()` (`daemon.rs:355-389`) takes a cross-process startup lock, probes, cleans a stale socket, spawns, and `wait_until_ready()`.
- `try_with_restart()` (`daemon.rs:419-459`) already retries write RPCs across a restart.
- systemd socket-activation is honored and excluded from autostart (`ensure_autostart_supported`, `daemon.rs:335-346`).

So "is it possible to spawn an ephemeral daemon and talk to it?" — **yes, that code ships today.** A *truly* ephemeral (spawn → serve one request → die) daemon is the wrong shape (per-command spawn thrash defeats the in-memory index and background sync); the correct shape is **spawn-on-demand, resident** — exactly what autostart already does. What ATU-538 actually requires beyond this is deleting the *other* half: the direct storage code the client still runs when it isn't talking to a daemon.

### 1.2 What "the other half" actually is (the census result)

Two exhaustive censuses (Appendix A/B) found that **essentially the entire data surface of atuin** is reached by opening local storage directly in the client. The chokepoints:

- `crates/atuin/src/command/client.rs:359-360` opens **both** the history `Sqlite` and the record `SqliteStore` for nearly every non-history subcommand.
- `crates/atuin/src/command/client/history.rs:582 / 600 / 1142` open the history DB for the history subcommands.
- `crates/atuin/src/command/client/history.rs:601 / 607 / 1143 / 1150` open the record store / `HistoryStore`.
- `crates/atuin-ai/src/commands/inline.rs:64` and `crates/atuin-ai/src/mcp.rs:87` open the history DB for `atuin ai` / `atuin mcp`.

Commands that would stop working the instant the client can no longer open storage:

`history start/end/list/last/init-store/prune/dedup`, `search` (non-interactive **and** the full interactive TUI: search, count, load, stats, get_dups, query_history, all_with_count), `stats`, `wrapped`, `import` (bulk insert), `sync`, `sync status`, `store status/rebuild/rekey/purge/verify/push/pull`, `scripts new --last`, `ai`, `mcp`, `login`/`register` (key re-encryption), `kv *`, `dotfiles alias/var *`, `scripts *`, `init <shell>` (dotfiles read), and `daemon start` (opens then legitimately hands off to the daemon crate).

That is the true scope of "remove the direct paths": **every one of those operations must become a daemon RPC, or be deleted.**

### 1.3 The one place direct access must remain

The **daemon crate itself** (`crates/atuin-daemon/`) keeps direct `Sqlite`/`SqliteStore` access — it is the single owner. `atuin daemon start` (`client.rs:359`, `daemon.rs:659/669`) opening the DB and handing it to `atuin_daemon::boot` is not a "daemonless path"; it is the boot path. The maintenance-burden duplication we are deleting is the *client-side second implementation* of the same operations, and the `if settings.daemon.enabled { rpc } else { direct }` branching that keeps both alive.

---

## Part 2 — Architecture: proxy the storage traits, don't rewrite every command

### 2.1 The naive approach explodes; the trait-proxy approach doesn't

The history DB is reached through the `Database` trait (`crates/atuin-client/src/database.rs:151-200`): ~18 methods — `save`, `save_bulk`, `load`, `list`, `range`, `update`, `history_count`, `last`, `before`, `delete`, `delete_rows`, `deleted`, `search`, `query_history`, `all_with_count`, `all_paged`, `stats`, `get_dups`. Record stores go through the `Store` trait (`record/store.rs`) and the typed `HistoryStore`/`AliasStore`/`VarStore`/`KvStore`/`ScriptStore` wrappers.

Two ways to make the daemon serve these:

- **(A) Curated RPCs** — design a bespoke gRPC method per command's need (`Stats`, `ListHistory`, `Dedup`, …). *Rejected:* it forces rewriting every command handler to call the new RPCs, re-deriving query/filter logic on the wire, and it multiplies the branch points we are trying to delete.
- **(B) Trait proxy (RECOMMENDED)** — add gRPC methods that mirror the `Database`/`Store` trait surface, then implement **`DaemonDatabase: Database`** and **`DaemonStore: Store`** in the client that forward each call over gRPC to the daemon, which services it with the *one* `Sqlite`/`SqliteStore` impl. Command handlers keep taking `&dyn Database` / `impl Store` and **do not change**. Only the ~6 construction sites swap the concrete type, and the query logic stays single-sourced inside the daemon.

Approach (B) is what makes "delete the direct paths" a bounded, mostly-mechanical project instead of a rewrite of 25 commands.

### 2.2 The resulting shape

```
                 today                              target
   CLI cmd ──► Sqlite (local file)      CLI cmd ──► &dyn Database
   CLI cmd ──► SqliteStore (local)                     │
       (also, when daemon.enabled,                     ▼
        a *second* path over gRPC)          DaemonDatabase / DaemonStore  (client-side proxy)
                                                        │ gRPC (unix socket / TCP)
                                                        ▼
                                            atuin-daemon ──► Sqlite / SqliteStore   (sole owner)
```

Benefits beyond deleting the duplication:
- **Single-writer SQLite for free.** Only the daemon opens the DB, eliminating multi-process lock contention (a recurring source of `database is locked`).
- **One place for query/sync/crypto logic.** No more "inline sync in `history end`" vs "background sync in the daemon" divergence.

### 2.3 Prerequisite: handlers must be generic over the trait, not the concrete `Sqlite`

Several handlers today take a concrete `Sqlite`/`SqliteStore` rather than `&dyn Database`/`impl Store`, and `HistoryStore::new` holds a concrete `SqliteStore`. Making them generic (or trait-object) is a mechanical but wide-reaching prerequisite (Task 1). Where a handler needs the *whole* history set (the **skim** engine calls `all_with_count`, `engines/skim.rs:60`, loading all rows), the proxy must stream — see risks.

### 2.4 Records/sync/dotfiles are higher-level and can use direct RPCs

Unlike the wide history *read* surface, record-store *writes* and sync are few and high-level (`push`, `delete_entries`, `re_encrypt`, `purge`, `verify`, `status`, `sync`, `rebuild`, plus dotfiles/kv/scripts `set/get/list/delete`). These are better exposed as explicit daemon RPCs than as a raw `Store`-trait proxy, because they carry policy (encryption keys, sync cadence, projection rebuilds) that must live daemon-side anyway. The daemon already owns the encryption key and a background sync loop (`components/sync.rs`), so moving `login`'s `re_encrypt` and `sync`'s network calls behind the daemon also **centralizes key handling** instead of loading the key in every client process.

---

## Part 3 — Decisions to ratify before coding

1. **No fallback — accepted consequence.** With the direct paths deleted, if the daemon cannot be spawned (no `fork`, locked-down sandbox, read-only `XDG_RUNTIME_DIR`, exhausted PIDs), **atuin cannot record or search history at all** — it fails rather than degrades. This is the explicit goal; documenting it here so it is a chosen tradeoff, not a surprise. Mitigation is limited to clear error messaging and making spawn as robust as possible.
2. **Every command spawns/needs a daemon**, including one-shots in scripts/CI (`atuin stats`, `atuin search <q>`, `atuin import`, `atuin mcp`, `atuin ai`). First invocation pays cold-start (already serialized by the startup lock). Acceptable? (Recommendation: yes, but keep cold-start fast and consider a `--no-daemon`-style *escape hatch for import only*, see risks.)
3. **Import bulk path.** `atuin import` does `save_bulk` of potentially millions of rows (`import.rs:155/168`). Proxying that row-by-row over gRPC is a non-starter; it needs a **client-streaming `SaveBulk` RPC**, or import runs *inside* the daemon. Ratify which.
4. **Sync/network ownership moves to the daemon** (diff/operations/sync_remote, `store push`/`pull`, `history end` auto-sync). Confirm the daemon becomes the sole sync driver and the CLI `sync`/`store push/pull` commands become thin RPC triggers.
5. **Scope of stores.** Confirm the dotfiles (`alias`/`var`), `kv`, and `scripts` record stores are in scope for removal too (they are separate `Store`-backed DBs). They are the long tail of the census and the largest incremental surface.
6. **Non-daemon build (`--no-default-features`, `daemon` feature off).** Today the daemonless path *is* the code that compiles with the `daemon` feature off. If we delete it, either (a) the `daemon` feature becomes non-optional, or (b) a minimal embedded path remains for that build. Ratify — this decides whether `#[cfg(feature = "daemon")]` gates disappear entirely.

---

## Part 4 — Phased implementation plan (contingent on Part 3)

> Multi-release. Each phase is independently shippable and leaves the tree working. TDD throughout: write the failing test, watch it fail, implement, watch it pass, commit. Every phase keeps Windows CI (`windows-latest`, TCP transport) green and preserves the non-fatal-on-error behavior of the shell hooks.

### Global Constraints

- Toolchain per `rust-toolchain.toml` (currently 1.97); do not bump.
- Keep Windows building/passing at every phase (no `fork`/unix-socket assumptions; use the existing `#[cfg(unix)]`/`#[cfg(not(unix))]` split).
- `history start`/`end` must stay non-fatal on error, exactly as today.
- Never autostart when `daemon.systemd_socket = true`; defer to systemd (`ensure_autostart_supported`, `daemon.rs:335-346`).
- Preserve `store_failed` semantics: non-zero exit with `store_failed = false` cancels the in-flight entry (`history.rs:560-565`, `:511-518`).
- Conventional commits; one shippable deliverable per phase; breaking changes flagged with `!` + changelog.

---

### Phase 0 — Make handlers generic over the storage traits (no behavior change)

Prerequisite refactor so a proxy can be substituted for `Sqlite`/`SqliteStore` later. Pure refactor; behavior identical.

**Files (representative):** `crates/atuin/src/command/client.rs`, `.../history.rs`, `.../search*.rs`, `.../stats.rs`, `.../sync.rs`, `crates/atuin-client/src/history/store.rs` (make `HistoryStore` generic over `Store`).

- [ ] **Step 1:** Add a compile-time assertion / test that the command handlers accept `&dyn Database` (e.g. a test that constructs a trivial `Database` mock and passes it to `run_non_interactive`).
- [ ] **Step 2:** Run it; confirm it fails to compile (handlers currently demand concrete `Sqlite`).
- [ ] **Step 3:** Change handler signatures from `&Sqlite`/`Sqlite` to `&dyn Database`/`impl Database`; make `HistoryStore<S: Store>` generic. Construction sites still pass concrete `Sqlite`/`SqliteStore`.
- [ ] **Step 4:** `cargo test --workspace` + `cargo build -p atuin --no-default-features` + `--features daemon` all green.
- [ ] **Step 5: Commit** — `refactor(client): make command handlers generic over Database/Store traits`

---

### Phase 1 — `DaemonDatabase` read proxy + proto for the `Database` read surface

Add gRPC methods mirroring the read half of `Database` (`search`, `query_history`, `list`, `range`, `load`, `last`, `before`, `history_count`, `stats`, `get_dups`, `all_with_count`/`all_paged` as streaming), a server impl backed by the daemon's `Sqlite`, and a client `DaemonDatabase` implementing those `Database` methods over gRPC. Writes still go direct in this phase.

**Files:** `crates/atuin-daemon/proto/history.proto` (or a new `database.proto`); `crates/atuin-daemon/src/components/` (new service impl); `crates/atuin-daemon/src/client.rs` (new client); `crates/atuin/src/command/client/.../DaemonDatabase` proxy.

- [ ] **Step 1:** Failing test — a round-trip: start an in-process daemon service over a `Sqlite`, seed rows, call `DaemonDatabase::search(...)`, assert the same rows as calling `Sqlite::search(...)` directly.
- [ ] **Step 2:** Run it; confirm it fails (RPC/proxy absent).
- [ ] **Step 3:** Add the proto methods + `prost`/`tonic` regen, server impl delegating to `Sqlite`, and `DaemonDatabase` client forwarding + type (de)serialization for `History`, `FilterMode`, `SearchMode`, `Context`.
- [ ] **Step 4:** Test passes; clippy clean; both feature builds green.
- [ ] **Step 5: Commit** — `feat(daemon): serve the Database read surface over gRPC`

---

### Phase 2 — Route all history *reads* through `DaemonDatabase`; delete direct read construction

Swap the read construction sites to build a `DaemonDatabase` (ensuring a daemon is running) instead of a `Sqlite`. This removes direct SQLite reads from `search`, `stats`, `wrapped`, interactive TUI, `history list/last`, `scripts new --last`, `ai`, `mcp`.

**Files:** `crates/atuin/src/command/client.rs:359`; `crates/atuin-ai/src/commands/inline.rs:64`, `mcp.rs:87`; `crates/atuin/src/command/client/search/engines/daemon.rs` (drop the local-hydrate fallback — the daemon now returns rows).

- [ ] **Step 1:** Failing integration test — run `atuin search <q>` / `atuin stats` against a fixture with a daemon and assert correct output with **no** client-side `Sqlite::new` (assert via a build-time grep test or a seam that panics if the client opens the DB).
- [ ] **Step 2:** Confirm failure.
- [ ] **Step 3:** Replace read constructors with `DaemonDatabase` behind `ensure_daemon_running`; collapse the `SearchMode::DaemonFuzzy` engine into the single daemon-backed engine (`engines.rs:17-29`).
- [ ] **Step 4:** Full suite + Windows-path build green.
- [ ] **Step 5: Commit** — `feat(client): route history reads through the daemon; remove direct-read SQLite`

---

### Phase 3 — Write/delete/import over the daemon; delete the daemonless write branch

Add write RPCs (`save`, `update`, `delete`, streaming `save_bulk`), route `history start/end/prune/dedup/init-store`, `import`, and interactive/`--delete` deletes through them, and **delete `handle_start`/`handle_end` direct bodies and the `settings.daemon.enabled` branch** at `history.rs:577/593`.

**Files:** proto write methods; daemon server write impl; `crates/atuin/src/command/client/history.rs` (remove daemonless branch + direct constructors at `:582/600/601/607/1142/1150`); `import.rs` (streaming bulk).

- [ ] **Step 1:** Failing tests — `history start`→`end` persists via daemon and reads back; `import` streams N rows and count matches; failed-cmd cancel semantics preserved.
- [ ] **Step 2:** Confirm failure.
- [ ] **Step 3:** Implement write RPCs + streaming bulk; delete the direct write branch and constructors.
- [ ] **Step 4:** Suite green; `--no-default-features` decision from Part 3.6 enforced.
- [ ] **Step 5: Commit** — `feat(client)!: daemon owns all history writes; remove daemonless write path`

---

### Phase 4 — Record store, sync, and key ops behind the daemon

Move `store status/rebuild/rekey/purge/verify/push/pull`, `sync`/`sync status`, and `login`/`register` key re-encryption to daemon RPCs; delete the client's `SqliteStore`/`HistoryStore` construction and inline sync (`client.rs:360`, `history.rs:528-548/601-609`, `sync.rs`, `store/*`, `account/login.rs:263`).

- [ ] **Step 1–5:** Per-operation TDD as above; the daemon becomes the sole sync driver and key holder; CLI sync/store commands become thin RPC triggers. Commit `feat(client)!: daemon owns record store, sync, and key rotation`.

---

### Phase 5 — Dotfiles / kv / scripts stores + final cleanup

Move `dotfiles alias/var`, `kv`, `scripts`, and `init <shell>` dotfiles reads behind the daemon; delete the last direct `*Store::new` sites; remove now-dead `#[cfg(feature = "daemon")]` branches per Part 3.6; delete the `daemon.enabled` setting (or repurpose).

- [ ] **Step 1–5:** Per-store TDD; final grep-test asserts **zero** `Sqlite::new`/`SqliteStore::new`/`*Store::new` outside `crates/atuin-daemon/` and the daemon boot path. Commit `feat(client)!: complete removal of direct storage access from the CLI`.

---

## Part 5 — Risks & mitigations

| Risk | Mitigation / decision |
|---|---|
| Daemon can't spawn ⇒ atuin non-functional (no fallback) | Accepted per Part 3.1; invest in robust spawn + clear errors |
| Cold-start on every one-shot command (CI/scripts) | Existing startup lock; keep boot fast; possible import-only escape hatch (Part 3.2/3.3) |
| `import` of millions of rows over gRPC | Client-streaming `SaveBulk` RPC or in-daemon import (Part 3.3) |
| skim engine loads *all* history (`all_with_count`) | Streaming read RPC, or reconsider skim under daemon |
| Windows (no fork) | Existing detached-spawn + TCP loopback path; keep `windows-latest` CI |
| systemd users double-managing daemon | Keep `ensure_autostart_supported` guard |
| Encryption key now daemon-held | Centralizes key handling; audit key lifecycle during Phase 4 |
| `--no-default-features` build loses its storage path | Ratify Part 3.6 (feature becomes non-optional vs minimal embedded path) |
| Large blast radius / long migration | Phased; each phase ships independently and keeps the tree working |

## Part 6 — Recommendation

1. **Proceed with complete removal** via the **trait-proxy architecture** (Part 2) — it is the only approach that deletes the duplication without rewriting every command.
2. **Ratify the six Part 3 decisions** first; #3 (import), #4 (sync ownership), and #6 (no-default-features build) are load-bearing.
3. **Land Phases 0–5 across several releases**, each shippable; do **not** attempt this in one PR.
4. Accept and document that the daemon becomes a hard dependency (no degraded mode).

---

## Part 7 — Implementation status (landed in this branch)

**The entire history-`Database` axis is now daemon-only in the shipped build.**
Under the default (`daemon`) feature, the CLI never opens the history SQLite
database itself — every read and write is served by the daemon over gRPC. The
daemonless code survives only under `#[cfg(not(feature = "daemon"))]` (the
minimal no-daemon build) and behind `#[cfg(test)]`.

What is routed through the daemon now:
- **Writes:** `history start`/`end` always go through the daemon
  (`start_history_entry`/`end_history_entry` call the daemon handlers
  unconditionally; the direct `handle_start`/`handle_end` are `cfg(not(daemon))`).
  The write hooks spawn the daemon on demand regardless of the old
  `daemon.autostart` opt-in (`autostart_enabled()` = "not systemd-managed").
- **Reads (all commands):** `search` (interactive + non-interactive), `stats`,
  `wrapped`, `history list/last/prune/dedup/init-store`, `scripts new --last`,
  `import`, `sync`/`store` history-count, and `mcp` — all receive a
  `Box<dyn Database>` from a single `history_database()` helper that ensures the
  daemon is running and returns a `DaemonDatabase` proxy.
- **The linchpin:** a blanket `impl Database for Box<dyn Database>`
  (`atuin-client/src/database.rs`) so every handler written against
  `impl Database`/`&impl Database` accepts the boxed proxy **unchanged**. Only
  `mcp` (concrete `&Sqlite` → `&dyn Database`) and the `daemon` subcommand
  (opens its own real `Sqlite`, since it *is* the owner) needed edits.
- The proxy is fully de-stubbed: `stats` and `all_with_count` now forward too.

Verified end-to-end with no `daemon.enabled` set: history written via the
daemon, then `history list`, `search` (fuzzy + prefix), and `stats` all served
by the daemon; a single daemon owns the socket; `daemon status`/`stop` behave.
`cargo build`/`clippy`/`test` are clean on default and `--no-default-features`.

The `atuin ai` interactive TUI and the MCP server are done too: `AppContext`'s
history handle is now `Arc<dyn Database>` and both build the proxy.

**Still on the direct path (next tranche — the record store):** everything
built on the `Store` trait — `sync` (network), `store
rebuild/rekey/purge/verify/push/pull`, `login`/`register` key rotation,
`search --delete`/interactive delete, and the `dotfiles`/`kv`/`scripts` typed
stores. This is a materially bigger change than the Database axis: the `Store`
trait is ~18 methods over `Record<EncryptedData>`, and all five typed stores
(`HistoryStore`/`AliasStore`/`VarStore`/`KvStore`/`ScriptStore`) hold a
*concrete* `SqliteStore`, so a `DaemonStore` proxy requires genericizing them
across `atuin-client`/`atuin-dotfiles`/`atuin-kv`/`atuin-scripts` plus ~10
command signatures. Load-bearing design decision first: does the daemon stay a
dumb encrypted-blob store (crypto stays client-side — least disruptive) or does
key handling move server-side too?

To *physically delete* the last daemonless code (the `cfg(not(daemon))` arms),
promote `daemon` from an optional feature to a hard dependency — a Cargo change
that drops the no-daemon build. Deferred pending that explicit call.

---

### Earlier proof of concept (superseded by the above)

The first slice proved the core claim — **the CLI can serve a real read
entirely through the daemon, never opening SQLite itself** — before the axis-wide
cutover:

What was built:

- **`crates/atuin-daemon/proto/database.proto`** — a `StorageDatabase` gRPC
  service mirroring the read/write half of the `Database` trait
  (`Save`/`SaveBulk`/`Load`/`List`/`Range`/`Update`/`HistoryCount`/`Last`/
  `Before`/`Delete`/`DeleteRows`/`Deleted`/`Search`/`QueryHistory`/`GetDups`),
  plus `HistoryRecord`/`Context`/`OptFiltersMsg` messages.
- **`crates/atuin-daemon/src/database/mod.rs`** — `History ⇄ HistoryRecord`,
  `Context`, `OptFilters`, `SearchMode`, `FilterMode` conversions.
- **`crates/atuin-daemon/src/components/database.rs`** — server-side
  `StorageDatabaseService`, delegating every call to the daemon's owned
  `Sqlite` via the existing `Database` trait. Registered in `server.rs`
  (unix + Windows) and `boot()`.
- **`crates/atuin-daemon/src/proxy.rs`** — **`DaemonDatabase`**, a client-side
  `impl Database` that forwards each call over gRPC. This is the drop-in that
  replaces `Sqlite` at construction sites. `all_paged` works for free (it only
  needs a boxed `Database` and drives itself via `query_history`).
- **`crates/atuin/src/command/client/search.rs`** — non-interactive
  `atuin search` now spawns the daemon on demand (`ensure_daemon_running`) and
  runs the query through `DaemonDatabase` when `daemon.enabled` is set. The
  command handler was **unchanged** — it already took `impl Database`, which
  is exactly why the proxy approach is low-churn.

Smoke test (isolated env, `daemon.enabled = true`): history written while
daemonless, then `atuin search echo` transparently spawned the daemon and
returned the right rows over gRPC; `search cargo` → 1 match; a non-matching
query → exit 1; `daemon status`/`stop` behaved. No client-side `Sqlite::new`
on the search read path.

Deliberately stubbed for the slice (return a clear "not yet supported" error,
wired up in the full cutover): `all_with_count` (skim engine) and `stats`
(interactive inspector). Writes/records/sync/dotfiles are not yet routed — that
is Phases 3–5. This PoC is Phase 1 + a sliver of Phase 2, and it compiles on
all three feature configurations (`--features daemon`, default,
`--no-default-features`).

---

## Appendix A — Full census: direct history-DB (`Database`/`Sqlite`) access

*(Excludes `crates/atuin-daemon/`, which is the intended sole owner.)*

**CONSTRUCT sites:** `client.rs:359` (all non-early-return cmds); `history.rs:582` (start), `:600` (end), `:1142` (list/last/init-store/prune/dedup); `atuin-ai/src/commands/inline.rs:64` (ai); `atuin-ai/src/mcp.rs:87-88` (mcp). (`doctor.rs:360` opens only an in-memory DB for a version string.)

**READ:** `search.rs:332` + delete-loop `:281`; `search/engines/db.rs:27`; `search/engines.rs:76`; `search/engines/skim.rs:60` (`all_with_count`); `search/engines/daemon.rs:88` (regex fallback), `:111` (`query_history` hydrate); `search/interactive.rs:1760` (`history_count`), `:1876` (`query_history`), `:1955` (`load`), `:1972` (`stats`), `:1935` (dispatch); `stats.rs:54` (`list`), `:58/62/66/70/74` (`range`); `wrapped.rs:319` (`range`); `history.rs:962` (`list`), `:991` (`list`), `:1053` (`get_dups`), `:1178` (`last`), `:499` (`load`); `sync.rs:99/130` (`history_count`); `sync/status.rs:31/32`; `scripts.rs:236` (`list`); `atuin-ai/src/tools/mod.rs:1206` (`search`, from `driver.rs:782` + `mcp.rs:52`); `atuin-ai/src/commands/inline.rs:73` (`last`).

**WRITE:** `history.rs:441` (`save`), `:527` (`update`); `import.rs:155/168` (`save_bulk`).

**DELETE:** `history.rs:515` (`delete`), `:1028` (prune), `:1089` (dedup).

**INDIRECT (`&db` handed to `HistoryStore`/sync):** `history.rs:1196` (`init_store`), `:1026/1087` (`incremental_build`); `search.rs:278`; `search/interactive.rs:1861/1884`; `sync.rs:42` (`crate::sync::build`); `command/client/sync.rs:111/125`; `store/rebuild.rs:61` (`build`); `store/pull.rs:90`.

## Appendix B — Full census: direct record-store (`Store`/`SqliteStore`/`HistoryStore`/…) access & sync

*(Excludes `crates/atuin-daemon/`.)*

**Shared CONSTRUCT:** `client.rs:360` opens `SqliteStore` for every non-early-return cmd.

**Per-command (file:line → op):**
- `history end`: `history.rs:601/607` construct; `:528` push; `:535` `record::sync::sync`; `:538` `crate::sync::build`; `:541` legacy sync.
- `history init-store`: `:1143/1150` construct; `:1196` `init_store`.
- `history prune`: `:1020` construct; `:1025` `delete`; `:1026` `incremental_build`.
- `history dedup`: `:1078` construct; `:1086` `delete`; `:1087` `incremental_build`.
- `sync`: `command/client/sync.rs:89` construct; `:91/116` `sync`; `:95/120` `build`; `:100` `len_tag`; `:111` `init_store`; `:125` legacy sync.
- `search --delete`: `search.rs:226` construct; `:277` `delete_entries`; `:278` `incremental_build`.
- `search -i` delete: `interactive.rs:1860/1883` `delete_entries`; `:1861/1884` `incremental_build`.
- `store status`: `store.rs:58/76/91/92` `status/first/last`.
- `store rebuild`: `store/rebuild.rs:59-61` history `build`; `:74-78` dotfiles; `:86-90` scripts.
- `store rekey`: `store/rekey.rs:49` `re_encrypt`.
- `store purge`: `store/purge.rs:19` `purge`.
- `store verify`: `store/verify.rs:19` `verify`.
- `store push`: `store/push.rs:63/73/106` diff/operations/`sync_remote`.
- `store pull`: `store/pull.rs:41` `delete_all`; `:51/61/86` diff/operations/`sync_remote`; `:90` `build`.
- `login`/`register`: `account/login.rs:263` `re_encrypt`; `account/register.rs:119` delegates.
- `kv *`: `kv.rs:73` `KvStore::new`.
- `dotfiles alias`: `dotfiles/alias.rs:164` `AliasStore::new`.
- `dotfiles var`: `dotfiles/var.rs:164` `VarStore::new`.
- `scripts *`: `scripts.rs:572` `ScriptStore::new`.
- `init <shell>`: `init.rs:130` construct; `:137-138` alias/var read.
- `wrapped`: `wrapped.rs:332` alias read.
- `daemon start`: `daemon.rs:86/658/669` receive store, hand to `atuin_daemon::boot` (kept — this is the owner).
- Shared projection `crate::sync::build`: `sync.rs:24` `load_key`, `:34` `HistoryStore::new`, `:35-38` alias/var/kv/script stores, `:42-62` `incremental_build`/`build`.

**Underlying `.push` impls:** `atuin-client/src/history/store.rs:139/167`; `atuin-dotfiles/src/store.rs:251/287`, `store/var.rs:298/334`; `atuin-kv/src/store.rs:104`; `atuin-scripts/src/store.rs:49`.

**Not implicated:** `crates/atuin-ai/` has no direct record-store/sync access. `import`, `stats`, `sync status`, `logout`, `account delete/change-password/link` open the store at dispatch but don't use it (would still need the lazy-construct change).
