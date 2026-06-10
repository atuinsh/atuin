# Atuin

Rust workspace for Atuin, a shell history replacement that stores command history in SQLite and optionally syncs encrypted history and records across machines.

## Commands

- `cargo build` - build the workspace.
- `cargo test` - run Rust tests.
- `cargo clippy -- -D warnings` - run clippy with warnings as errors.
- `cargo fmt --check` - check formatting.
- `cargo nextest run` - preferred test runner noted in `AGENTS.md`, if installed.
- `cargo deny check` - dependency/license/advisory checks using `deny.toml`, if installed.

These commands are discovered from repo docs/config; do not say they passed unless run in the current session.

## Important Paths

- `crates/atuin/` - CLI binary and TUI.
- `crates/atuin-client/` - client DB, settings, encryption, and sync.
- `crates/atuin-common/` - shared API models/types/utilities.
- `crates/atuin-daemon/` - background gRPC daemon for shell hooks.
- `crates/atuin-server/` - Axum sync server.
- `crates/atuin-server-postgres/` and `crates/atuin-server-sqlite/` - server storage implementations.
- `crates/atuin-history/` - history sorting/stats.
- `crates/atuin-kv/`, `crates/atuin-dotfiles/`, `crates/atuin-scripts/` - V2 record-backed synced data types.
- `docs/` and `docs-i18n/` - documentation sources.
- `AGENTS.md` - existing repo-specific architecture/convention notes; read it before larger changes.

## Conventions

- Rust toolchain is pinned in `rust-toolchain.toml` to `1.95.0`; workspace package uses Rust 2024 edition.
- V1 sync is legacy direct history sync; V2 is the current tagged record-store sync path.
- Client storage is SQLite; server storage is Postgres or SQLite depending on URI.
- Do not modify existing migrations; add new migrations instead.
- Keep `history start`, `history end`, and `init` latency-sensitive. Avoid adding database initialization to those hot paths.
- Errors use `eyre::Result` in binaries and `thiserror` for typed library errors.
- Local PATH may contain shadowed Atuin binaries; use `command -v` / `which -a` before trusting a local version check.
