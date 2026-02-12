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

- Rust 2024 edition, toolchain 1.93.1.
- Errors: `eyre::Result` in binaries, `thiserror` for typed errors in libraries.
- Async: tokio. Client uses `current_thread`; server uses `multi_thread`.
- `#![deny(unsafe_code)]` on client/common, `#![forbid(unsafe_code)]` on server.
- Clippy: `pedantic` + `nursery` on main crate. CI enforces `-D warnings -D clippy::redundant_clone`.
- Format: `cargo fmt`. Only non-default: `reorder_imports = true`.
- IDs: UUIDv7 (time-ordered), newtype wrappers (`HistoryId`, `RecordId`, `HostId`).
- Serialization: MessagePack for encrypted payloads, JSON for API, TOML for config.
- Storage traits: `Database` (client), `Store` (record store), `Database` (server) -- all `async_trait`.
- History builders: `HistoryImported`, `HistoryCaptured`, `HistoryFromDb` with compile-time field validation.
- Feature flags: `client`, `sync`, `daemon`, `clipboard`, `check-update`.

## Testing

- Unit tests inline with `#[cfg(test)]`, async via `#[tokio::test]`.
- Integration tests in `crates/atuin/tests/` need Postgres (`ATUIN_DB_URI` env var).
- Use `":memory:"` SQLite for unit tests needing a database.
- Runner: `cargo nextest`.
- Benchmarks: `divan` in `atuin-history`.

## Build and check

```sh
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```
