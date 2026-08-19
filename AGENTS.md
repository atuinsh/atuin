# Atuin

Shell history tool. Replaces your shell's built-in history with a SQLite database, adds context (cwd, exit code, duration, hostname), and optionally syncs across machines with end-to-end encryption.

## Workspace crates

```
atuin                  CLI binary + TUI (clap, ratatui, crossterm)
atuin-client           Client library: local DB, encryption, sync, settings
atuin-common           Shared types, API models, utils
atuin-daemon           Background gRPC daemon (tonic) for shell hooks
atuin-dotfiles         Alias/var sync via record store
atuin-history          Sorting algorithms, stats
atuin-kv               Key-value store (synced)
atuin-scripts          Script management (minijinja)
atuin-server           HTTP sync server (axum) - lib + standalone binary
atuin-server-database  Database trait for server
atuin-server-postgres  Postgres implementation (sqlx)
atuin-server-sqlite    SQLite implementation (sqlx)
```

## Two sync protocols

- **V1 (legacy)**: Syncs history entries directly. Being phased out. Toggleable via `sync_v1_enabled`.
- **V2 (current)**: Record store abstraction. All data types (history, KV, aliases, vars, scripts) share the same sync infrastructure using tagged records. Envelope-encrypted with PASETO V4 and per-record CEKs.

## Encryption

- **V1**: XSalsa20Poly1305 (secretbox). Key at `~/.local/share/atuin/key`.
- **V2**: PASETO V4 Local (XChaCha20-Poly1305 + Blake2b). Envelope encryption: each record gets a random CEK wrapped with the master key. Record metadata (id, idx, version, tag, host) is authenticated as implicit assertions.

## Databases

- **Client**: SQLite everywhere. Separate DBs for history, record store, KV, scripts. All use sqlx + WAL mode.
- **Server**: Postgres (primary) or SQLite. Auto-detected from URI prefix.
- Migrations live alongside each crate. Never modify existing migrations, only add new ones.

## Hot paths

`history start`, `history end`, and `init` skip database initialization for latency. Don't add DB calls to these without good reason.

## Conventions

- Rust 2024 edition, toolchain 1.97.0.
- Errors: `eyre::Result` in binaries, `thiserror` for typed errors in libraries.
- Derive boilerplate: `derive_more` (workspace dep) for `Display`, `From`, `Into`, `AsRef`, `Deref`, `Debug` on newtypes and simple enums. Prefer `derive_more` over manual `impl` when the formatting/conversion is a straight delegation. Use `thiserror` (not `derive_more`) for error types. Use `#[as_ref(str)]` on string newtypes for `AsRef<str>`.
- Async: tokio. Client uses `current_thread`; server uses `multi_thread`.
- `#![deny(unsafe_code)]` on client/common, `#![forbid(unsafe_code)]` on server.
- Clippy: `pedantic` + `nursery` on main crate. CI enforces `-D warnings`, on both the default targets and `--tests`.
- Rustdoc: CI runs `cargo doc --document-private-items --no-deps --workspace` with `RUSTDOCFLAGS=-D warnings`. Broken intra-doc links fail the build.
- Format: `cargo +nightly fmt`. `.rustfmt.toml` uses nightly-only options, so formatting requires the nightly toolchain even though the project builds on stable 1.97.0.
- IDs: UUIDv7 (time-ordered), newtype wrappers (`HistoryId`, `RecordId`, `HostId`).
- Serialization: MessagePack for encrypted payloads, JSON for API, TOML for config.
- Storage traits: `Database` (client), `Store` (record store), `Database` (server) -- all `async_trait`.
- History builders: `HistoryImported`, `HistoryCaptured`, `HistoryFromDb` with compile-time field validation.
- Feature flags: `client`, `sync`, `daemon`, `clipboard`, `check-update`.

## Testing

- Unit tests inline with `#[cfg(test)]`. Use `rstest` for every test — `#[rstest]`, never a
  bare `#[test]` (async: `#[rstest]` + `#[tokio::test]`); migrate plain `#[test]`s in files you touch.
- Lean on `#[fixture]`s for shared setup and compose them; when a test needs teardown, return an
  RAII guard from the fixture (e.g. a temp dir removed on `Drop`) rather than cleaning up by hand.
- Parametrize with `#[case(...)]` (input/expected tables) and `#[values(...)]` (cross-products of
  independent parameters) instead of near-duplicate tests.
- Reach for `proptest` when a property holds across many inputs — round-trips (encode/decode, serde,
  parse/display), invariants, idempotence; keep targeted `#[case]`s for known edge cases and regressions.
- Integration tests in `crates/atuin/tests/` need Postgres (`ATUIN_DB_URI` env var).
- Use `rstest` for tests, especially when they can be made simpler using `case`s and `fixture`s.
- Use `":memory:"` SQLite for unit tests needing a database.
- Runner: `cargo nextest`.
- Benchmarks: `divan` in `atuin-client` and `atuin-history`, tracked in CI by CodSpeed. Run them
  locally with `cargo codspeed build && cargo codspeed run`, or with plain `cargo bench`.

## Build and check

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo clippy --tests -- -D warnings
cargo +nightly fmt --check
RUSTDOCFLAGS="-D warnings" cargo doc --document-private-items --no-deps --workspace
```
