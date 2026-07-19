# Commonize SQLite Store Setup in atuin-common Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the near-identical SQLite pool-opening boilerplate duplicated across six stores into one feature-gated `atuin_common::sqlite` builder, and migrate every store onto it — behavior-preserving.

**Architecture:** A new `#[cfg(feature = "sqlite")]` module in `atuin-common` exposes `pool(path, timeout) -> PoolBuilder`, a builder whose `.open().await` does the filesystem prep (broken-symlink guard, `:memory:` detection, parent-dir creation), applies the configured `SqliteConnectOptions` pragmas, connects the `SqlitePool`, and optionally restricts file permissions to `0o600`. Each store replaces its hand-rolled `new()` internals with a builder call carrying exactly its current pragma set; migrations stay per-store (they use the `sqlx::migrate!` macro).

**Tech Stack:** Rust (edition 2024), `sqlx` 0.8 (SQLite), `atuin-common`, `tokio` + `tempfile` (dev only).

## Global Constraints

- MSRV `1.97.0`; edition 2024. Let-chains (`if … && let …`) are used and available.
- Feature name is exactly **`sqlite`**, defined as `sqlite = ["sqlx/sqlite", "sqlx/regexp"]` on `atuin-common`. `sqlx` is already a (non-optional) dependency of `atuin-common`; this feature only enables its `sqlite` and `regexp` sub-features. The module is gated `#[cfg(feature = "sqlite")]`.
- **Behavior-preserving.** Each store's resulting `SqliteConnectOptions` must be identical to today. The builder is faithful to *which methods each store calls* — `synchronous` is `Option` (unset ⇒ not called ⇒ sqlx default), `foreign_keys`/`regexp`/`restrict_permissions` are opt-in booleans (false ⇒ not called). This exact per-store mapping is mandatory:

  | Store | file | journal | synchronous | foreign_keys | regexp | restrict_perms |
  |---|---|---|---|---|---|---|
  | history `Sqlite` | `atuin-client/src/database.rs` | Wal (default) | Normal | (unset) | true | (no) |
  | record `SqliteStore` | `atuin-client/src/record/sqlite_store.rs` | Wal (default) | Normal | true | (no) | (no) |
  | meta `MetaStore` | `atuin-client/src/meta.rs` | **Delete** | (unset) | (unset) | (no) | **true** |
  | kv `Database` | `atuin-kv/src/database.rs` | Wal (default) | Normal | true | true | (no) |
  | scripts `Database` | `atuin-scripts/src/database.rs` | Wal (default) | Normal | true | true | (no) |
  | ai `AiSessionStore` | `atuin-ai/src/store.rs` | Wal (default) | (unset) | (unset) | (no) | **true** |

  Every store also gets `optimize_on_close(true, None)` and `create_if_missing(true)` (universal — the builder always applies them). "(unset)" foreign_keys/synchronous means the builder does NOT call that setter, so sqlx's default (foreign keys ON, synchronous FULL) applies exactly as today.
- **One intentional behavior change, called out for review:** the broken-symlink case currently does `eprintln!(...) + std::process::exit(1)` inside library code (4 stores). The builder replaces this with a returned `sqlx::Error` that propagates to the CLI's normal error handling (no library `process::exit`). The meta and ai stores, which had no broken-symlink check, gain this (strictly safer) guard. Note this in the PR.
- No new runtime dependencies. `atuin-common` gains only dev-dependencies `tokio` and `tempfile` (both already in the workspace).

## File Structure

- **Create** `crates/atuin-common/src/sqlite.rs` — the `PoolBuilder`, `pool()`, `version()`, and unit tests. Single responsibility: open a configured SQLite pool.
- **Modify** `crates/atuin-common/Cargo.toml` — add the `sqlite` feature and the `tokio`/`tempfile` dev-deps.
- **Modify** `crates/atuin-common/src/lib.rs` — register `#[cfg(feature = "sqlite")] pub mod sqlite;`.
- **Modify** the six store files + their crates' `Cargo.toml` (enable `atuin-common/sqlite`), one per task.

---

### Task 1: Add the `atuin_common::sqlite` module (feature + builder + tests)

**Files:**
- Create: `crates/atuin-common/src/sqlite.rs`
- Modify: `crates/atuin-common/Cargo.toml` (`[features]` + `[dev-dependencies]`)
- Modify: `crates/atuin-common/src/lib.rs` (register the module after `pub mod shell;`)
- Test: `crates/atuin-common/src/sqlite.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces (relied on by Tasks 2-7), all at `atuin_common::sqlite`:
  - `pub fn pool(path: &Path, timeout: f64) -> PoolBuilder<'_>`
  - `PoolBuilder` methods (each `#[must_use]`, consume+return `Self`): `journal_mode(SqliteJournalMode)`, `synchronous(SqliteSynchronous)`, `foreign_keys(bool)`, `regexp(bool)`, `restrict_permissions(bool)`
  - `pub async fn open(self) -> sqlx::Result<SqlitePool>` (terminal)
  - `pub async fn version(pool: &SqlitePool) -> sqlx::Result<String>`

- [ ] **Step 1: Add the feature and dev-deps to `Cargo.toml`**

In `crates/atuin-common/Cargo.toml`, add to `[features]` (after the `unicode = ...` line):

```toml
# The `sqlite` module: shared SQLite pool setup for Atuin's SQLite stores.
sqlite = ["sqlx/sqlite", "sqlx/regexp"]
```

And extend `[dev-dependencies]` to:

```toml
[dev-dependencies]
pretty_assertions = { workspace = true }
proptest = { workspace = true }
rstest = { workspace = true }
tokio = { workspace = true }
tempfile = { workspace = true }
```

- [ ] **Step 2: Register the module and write the failing tests**

In `crates/atuin-common/src/lib.rs`, add after `pub mod shell;`:

```rust
#[cfg(feature = "sqlite")]
pub mod sqlite;
```

Create `crates/atuin-common/src/sqlite.rs` with ONLY the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::{pool, version};
    use sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous};
    use std::path::Path;

    #[tokio::test]
    async fn opens_in_memory_and_runs_a_query() {
        let p = pool(Path::new(":memory:"), 5.0).open().await.unwrap();
        let one: i64 = sqlx::query_scalar("SELECT 1").fetch_one(&p).await.unwrap();
        assert_eq!(one, 1);
    }

    #[tokio::test]
    async fn version_is_non_empty() {
        let p = pool(Path::new(":memory:"), 5.0).open().await.unwrap();
        assert!(!version(&p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn creates_file_and_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("test.db");
        let _p = pool(&path, 5.0).open().await.unwrap();
        assert!(path.exists());
    }

    #[tokio::test]
    async fn regexp_enabled_when_requested() {
        let p = pool(Path::new(":memory:"), 5.0)
            .regexp(true)
            .open()
            .await
            .unwrap();
        let m: i64 = sqlx::query_scalar("SELECT 'abc' REGEXP 'b'")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(m, 1);
    }

    #[tokio::test]
    async fn foreign_keys_on_when_requested() {
        let p = pool(Path::new(":memory:"), 5.0)
            .foreign_keys(true)
            .open()
            .await
            .unwrap();
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn synchronous_normal_when_requested() {
        let p = pool(Path::new(":memory:"), 5.0)
            .synchronous(SqliteSynchronous::Normal)
            .open()
            .await
            .unwrap();
        let s: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(s, 1); // 1 == NORMAL
    }

    #[tokio::test]
    async fn journal_mode_delete_when_requested() {
        // Journal mode is observable only on a real file (in-memory reports
        // "memory"), so use a temp file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.db");
        let p = pool(&path, 5.0)
            .journal_mode(SqliteJournalMode::Delete)
            .open()
            .await
            .unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "delete");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn broken_symlink_errors() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("does-not-exist.db");
        let link = dir.path().join("link.db");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(pool(&link, 5.0).open().await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restrict_permissions_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perms.db");
        let _p = pool(&path, 5.0)
            .restrict_permissions(true)
            .open()
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p atuin-common --features sqlite --lib sqlite`
Expected: FAIL — compile error, `pool`/`version` not found.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/atuin-common/src/sqlite.rs`, above the test module:

```rust
//! Shared SQLite pool setup for Atuin's SQLite-backed stores.
//!
//! Compiled only when the `sqlite` feature is enabled. Use [`pool`] to build a
//! configured [`SqlitePool`]; the defaults match the common Atuin store and the
//! builder methods cover the per-store divergences.

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use crate::utils;

/// Builder for a configured [`SqlitePool`], created via [`pool`].
///
/// Defaults: WAL journal, `optimize_on_close`, `create_if_missing`, SQLite's
/// default synchronous mode and foreign-key setting, no regexp, no permission
/// restriction. Adjust with the builder methods, then call [`open`](Self::open).
#[derive(Debug, Clone)]
pub struct PoolBuilder<'a> {
    path: &'a Path,
    timeout: f64,
    journal_mode: SqliteJournalMode,
    synchronous: Option<SqliteSynchronous>,
    foreign_keys: bool,
    regexp: bool,
    restrict_permissions: bool,
}

/// Begin configuring a [`SqlitePool`] for the database at `path`, acquiring
/// connections within `timeout` seconds.
pub fn pool(path: &Path, timeout: f64) -> PoolBuilder<'_> {
    PoolBuilder {
        path,
        timeout,
        journal_mode: SqliteJournalMode::Wal,
        synchronous: None,
        foreign_keys: false,
        regexp: false,
        restrict_permissions: false,
    }
}

impl<'a> PoolBuilder<'a> {
    /// Journal mode (default [`SqliteJournalMode::Wal`]).
    #[must_use]
    pub fn journal_mode(mut self, mode: SqliteJournalMode) -> Self {
        self.journal_mode = mode;
        self
    }

    /// Synchronous mode. When left unset, SQLite's default (FULL) applies.
    #[must_use]
    pub fn synchronous(mut self, synchronous: SqliteSynchronous) -> Self {
        self.synchronous = Some(synchronous);
        self
    }

    /// Explicitly enforce foreign keys. When left `false`, the setter is not
    /// called and SQLite's default (foreign keys ON in sqlx) applies.
    #[must_use]
    pub fn foreign_keys(mut self, enabled: bool) -> Self {
        self.foreign_keys = enabled;
        self
    }

    /// Register the `REGEXP` operator (needs sqlx's `regexp` feature, which the
    /// `sqlite` feature enables).
    #[must_use]
    pub fn regexp(mut self, enabled: bool) -> Self {
        self.regexp = enabled;
        self
    }

    /// After opening, restrict the database file to `0o600` on Unix. No-op on
    /// non-Unix and for in-memory databases.
    #[must_use]
    pub fn restrict_permissions(mut self, enabled: bool) -> Self {
        self.restrict_permissions = enabled;
        self
    }

    /// Prepare the parent directory, apply the configured pragmas, connect the
    /// pool, and (if requested) restrict file permissions.
    ///
    /// Errors if the path is not valid UTF-8 or is a broken symlink. In-memory
    /// databases (paths containing `:memory:`) skip all filesystem preparation.
    pub async fn open(self) -> sqlx::Result<SqlitePool> {
        let path = self.path;

        let path_str = path.as_os_str().to_str().ok_or_else(|| {
            sqlx::Error::Configuration(
                format!("sqlite db path is not valid UTF-8: {path:?}").into(),
            )
        })?;
        let is_memory = path_str.contains(":memory:");

        if !is_memory {
            if utils::broken_symlink(path) {
                return Err(sqlx::Error::Configuration(
                    format!(
                        "sqlite db path ({path:?}) is a broken symlink; unable to read or create replacement"
                    )
                    .into(),
                ));
            }

            if !path.exists()
                && let Some(dir) = path.parent()
            {
                std::fs::create_dir_all(dir).map_err(sqlx::Error::Io)?;
            }
        }

        let mut opts = SqliteConnectOptions::from_str(path_str)?
            .journal_mode(self.journal_mode)
            .optimize_on_close(true, None)
            .create_if_missing(true);

        if let Some(synchronous) = self.synchronous {
            opts = opts.synchronous(synchronous);
        }
        if self.foreign_keys {
            opts = opts.foreign_keys(true);
        }
        if self.regexp {
            opts = opts.with_regexp();
        }

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(self.timeout))
            .connect_with(opts)
            .await?;

        #[cfg(unix)]
        if self.restrict_permissions && !is_memory {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(sqlx::Error::Io)?;
        }

        Ok(pool)
    }
}

/// Query the underlying SQLite library version (`SELECT sqlite_version()`).
pub async fn version(pool: &SqlitePool) -> sqlx::Result<String> {
    sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(pool)
        .await
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atuin-common --features sqlite --lib sqlite`
Expected: PASS (9 tests on Unix; 7 on non-Unix).

- [ ] **Step 6: Clippy**

Run: `cargo clippy -p atuin-common --features sqlite --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/atuin-common/Cargo.toml crates/atuin-common/src/lib.rs crates/atuin-common/src/sqlite.rs
git commit -m "feat(common): add feature-gated sqlite pool builder"
```

---

### Task 2: Migrate the record store (`atuin-client`)

**Files:**
- Modify: `crates/atuin-client/Cargo.toml` (enable `atuin-common/sqlite`)
- Modify: `crates/atuin-client/src/record/sqlite_store.rs` (`new()` at lines 35-68; imports at 12-18)

**Interfaces:**
- Consumes: `atuin_common::sqlite::pool` (Task 1). Config: `synchronous(Normal).foreign_keys(true)`.

- [ ] **Step 1: Enable the feature**

In `crates/atuin-client/Cargo.toml`, change the `[dependencies.atuin-common]` `features` line from `features = ["tracing"]` to:

```toml
features = ["tracing", "sqlite"]
```

- [ ] **Step 2: Replace the `new()` body**

In `crates/atuin-client/src/record/sqlite_store.rs`, replace the `new` function (lines 35-68) with:

```rust
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening sqlite database at {path:?}");

        let pool = atuin_common::sqlite::pool(path, timeout)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .open()
            .await?;

        Self::setup_db(&pool).await?;

        Ok(Self { pool })
    }
```

Then update the sqlx import block (lines 12-18) — `SqliteConnectOptions`, `SqliteJournalMode`, `SqlitePoolOptions` are now unused; keep `Row`, `SqliteRow`, `SqliteSynchronous`:

```rust
use sqlx::{
    Row,
    sqlite::{SqlitePool, SqliteRow, SqliteSynchronous},
};
```

- [ ] **Step 3: Remove other now-unused imports**

Run `cargo build -p atuin-client` and remove whatever it flags as unused (expected: `std::str::FromStr` at line 5, and `fs_err as fs` at line 10 if not used elsewhere in the file). Let `cargo clippy -p atuin-client --all-targets -- -D warnings` be the arbiter.

- [ ] **Step 4: Build, test, clippy**

Run: `cargo test -p atuin-client --lib record` then `cargo clippy -p atuin-client --all-targets -- -D warnings`
Expected: the record-store tests pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/atuin-client/Cargo.toml crates/atuin-client/src/record/sqlite_store.rs
git commit -m "refactor(client): open record store via atuin_common::sqlite"
```

---

### Task 3: Migrate the history store (`atuin-client`)

**Files:**
- Modify: `crates/atuin-client/src/database.rs` (`new()` lines 243-274; `sqlite_version()` lines 276-280; imports 15-21)

**Interfaces:**
- Consumes: `atuin_common::sqlite::{pool, version}` (Task 1). Config: `synchronous(Normal).regexp(true)`. (`atuin-common/sqlite` already enabled by Task 2.)

- [ ] **Step 1: Replace the `new()` body**

In `crates/atuin-client/src/database.rs`, replace `new` (lines 243-274) with:

```rust
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening sqlite database at {path:?}");

        let pool = atuin_common::sqlite::pool(path, timeout)
            .synchronous(SqliteSynchronous::Normal)
            .regexp(true)
            .open()
            .await?;

        Self::setup_db(&pool).await?;
        Ok(Self { pool })
    }
```

- [ ] **Step 2: Delegate `sqlite_version()`**

Replace `sqlite_version` (lines 276-280) with:

```rust
    pub async fn sqlite_version(&self) -> Result<String> {
        atuin_common::sqlite::version(&self.pool).await
    }
```

- [ ] **Step 3: Trim the sqlx imports**

Update the sqlx import block (lines 15-21) — `SqliteConnectOptions`, `SqliteJournalMode`, `SqlitePoolOptions` are now unused; keep `Result`, `Row`, `SqliteRow`, `SqliteSynchronous`:

```rust
use sqlx::{
    Result, Row,
    sqlite::{SqlitePool, SqliteRow, SqliteSynchronous},
};
```

Then run `cargo build -p atuin-client` and remove any other newly-unused imports it flags (candidates: `str::FromStr` line 4; `fs_err as fs` line 11 and `std::time::Duration` line 5 only if unused elsewhere in the file). Let clippy be the arbiter.

- [ ] **Step 4: Build, test, clippy**

Run: `cargo test -p atuin-client --lib` then `cargo clippy -p atuin-client --all-targets -- -D warnings`
Expected: all atuin-client lib tests pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/atuin-client/src/database.rs
git commit -m "refactor(client): open history store via atuin_common::sqlite"
```

---

### Task 4: Migrate the meta store (`atuin-client`)

**Files:**
- Modify: `crates/atuin-client/src/meta.rs` (`new()` lines 33-83; imports line 7)

**Interfaces:**
- Consumes: `atuin_common::sqlite::pool` (Task 1). Config: `journal_mode(Delete).restrict_permissions(true)`.

- [ ] **Step 1: Replace the `new()` body**

In `crates/atuin-client/src/meta.rs`, replace `new` (lines 33-83) with:

```rust
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening meta sqlite database at {path:?}");

        // DELETE journal mode (not WAL): this is a small, infrequently-written
        // KV store, so WAL's concurrency wins aren't needed, and DELETE avoids
        // the auxiliary -wal/-shm files that complicate permission handling.
        // Session tokens live here, so restrict the file to 0o600.
        let pool = atuin_common::sqlite::pool(path, timeout)
            .journal_mode(SqliteJournalMode::Delete)
            .restrict_permissions(true)
            .open()
            .await?;

        sqlx::migrate!("./meta-migrations").run(&pool).await?;

        let store = Self {
            pool,
            cached_host_id: OnceCell::const_new(),
        };

        let is_memory = path
            .as_os_str()
            .to_str()
            .is_some_and(|s| s.contains(":memory:"));
        if !is_memory {
            store.migrate_files().await?;
        }

        Ok(store)
    }
```

- [ ] **Step 2: Trim imports**

Update the sqlx import (line 7) — `SqliteConnectOptions`, `SqlitePoolOptions` are now unused; keep `SqliteJournalMode`, `SqlitePool`:

```rust
use sqlx::sqlite::{SqliteJournalMode, SqlitePool};
```

Then run `cargo build -p atuin-client` and remove any other newly-unused imports it flags (candidates: `std::str::FromStr` line 2, `std::time::Duration` line 3, and the `eyre!` import at line 6 if `eyre!` is no longer used in the file). Let clippy be the arbiter.

- [ ] **Step 3: Build, test, clippy**

Run: `cargo test -p atuin-client --lib meta` then `cargo clippy -p atuin-client --all-targets -- -D warnings`
Expected: meta tests pass; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/atuin-client/src/meta.rs
git commit -m "refactor(client): open meta store via atuin_common::sqlite"
```

---

### Task 5: Migrate the KV store (`atuin-kv`)

**Files:**
- Modify: `crates/atuin-kv/Cargo.toml` (enable `atuin-common/sqlite`)
- Modify: `crates/atuin-kv/src/database.rs` (`new()` lines 22-54; `sqlite_version()` lines 56-60; imports 4-10)

**Interfaces:**
- Consumes: `atuin_common::sqlite::{pool, version}`. Config: `synchronous(Normal).foreign_keys(true).regexp(true)`.

- [ ] **Step 1: Enable the feature**

In `crates/atuin-kv/Cargo.toml`, change the `atuin-common` line (line 18) to:

```toml
atuin-common = { path = "../atuin-common", version = "18.17.1", features = ["sqlite"] }
```

- [ ] **Step 2: Replace `new()`**

Replace `new` (lines 22-54) with:

```rust
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening KV sqlite database at {:?}", path);

        let pool = atuin_common::sqlite::pool(path, timeout)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .regexp(true)
            .open()
            .await?;

        Self::setup_db(&pool).await?;
        Ok(Self { pool })
    }
```

- [ ] **Step 3: Delegate `sqlite_version()`**

Replace `sqlite_version` (lines 56-60) with:

```rust
    pub async fn sqlite_version(&self) -> Result<String> {
        atuin_common::sqlite::version(&self.pool).await
    }
```

- [ ] **Step 4: Trim imports**

Update the sqlx import (lines 4-10) — remove `SqliteConnectOptions`, `SqliteJournalMode`, `SqlitePoolOptions`; keep `Result`, `Row`, `SqlitePool`, `SqliteRow`, `SqliteSynchronous`:

```rust
use sqlx::{
    Result, Row,
    sqlite::{SqlitePool, SqliteRow, SqliteSynchronous},
};
```

Then `cargo build -p atuin-kv` and remove other flagged imports (candidates: `str::FromStr` and `time::Duration` from line 1; `tokio::fs` line 11 if now unused). Let clippy be the arbiter.

- [ ] **Step 5: Build, test, clippy**

Run: `cargo test -p atuin-kv` then `cargo clippy -p atuin-kv --all-targets -- -D warnings`
Expected: pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/atuin-kv/Cargo.toml crates/atuin-kv/src/database.rs
git commit -m "refactor(kv): open store via atuin_common::sqlite"
```

---

### Task 6: Migrate the scripts store (`atuin-scripts`)

**Files:**
- Modify: `crates/atuin-scripts/Cargo.toml` (enable `atuin-common/sqlite`)
- Modify: `crates/atuin-scripts/src/database.rs` (`new()` lines 23-55; `sqlite_version()` lines 57-61; imports 4-10)

**Interfaces:**
- Consumes: `atuin_common::sqlite::{pool, version}`. Config: `synchronous(Normal).foreign_keys(true).regexp(true)` (identical to KV).

- [ ] **Step 1: Enable the feature**

In `crates/atuin-scripts/Cargo.toml`, change the `atuin-common` line (line 18) to:

```toml
atuin-common = { path = "../atuin-common", version = "18.17.1", features = ["sqlite"] }
```

- [ ] **Step 2: Replace `new()`**

Replace `new` (lines 23-55) with:

```rust
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening script sqlite database at {:?}", path);

        let pool = atuin_common::sqlite::pool(path, timeout)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .regexp(true)
            .open()
            .await?;

        Self::setup_db(&pool).await?;
        Ok(Self { pool })
    }
```

- [ ] **Step 3: Delegate `sqlite_version()`**

Replace `sqlite_version` (lines 57-61) with:

```rust
    pub async fn sqlite_version(&self) -> Result<String> {
        atuin_common::sqlite::version(&self.pool).await
    }
```

- [ ] **Step 4: Trim imports**

Update the sqlx import (lines 4-10) — remove `SqliteConnectOptions`, `SqliteJournalMode`, `SqlitePoolOptions`; keep `Result`, `Row`, `SqlitePool`, `SqliteRow`, `SqliteSynchronous`:

```rust
use sqlx::{
    Result, Row,
    sqlite::{SqlitePool, SqliteRow, SqliteSynchronous},
};
```

Then `cargo build -p atuin-scripts` and remove other flagged imports (candidates: `str::FromStr` and `time::Duration` from line 1; `tokio::fs` line 11 if now unused). Let clippy be the arbiter.

- [ ] **Step 5: Build, test, clippy**

Run: `cargo test -p atuin-scripts` then `cargo clippy -p atuin-scripts --all-targets -- -D warnings`
Expected: pass; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/atuin-scripts/Cargo.toml crates/atuin-scripts/src/database.rs
git commit -m "refactor(scripts): open store via atuin_common::sqlite"
```

---

### Task 7: Migrate the AI session store (`atuin-ai`)

**Files:**
- Modify: `crates/atuin-ai/Cargo.toml` (enable `atuin-common/sqlite`)
- Modify: `crates/atuin-ai/src/store.rs` (`new()` lines 58-93; imports line 6)

**Interfaces:**
- Consumes: `atuin_common::sqlite::pool`. Config: `restrict_permissions(true)` only (Wal + FULL synchronous + default foreign keys are the builder defaults).

- [ ] **Step 1: Enable the feature**

In `crates/atuin-ai/Cargo.toml`, change the `atuin-common` line (line 24) to:

```toml
atuin-common = { workspace = true, features = ["unicode", "sqlite"] }
```

- [ ] **Step 2: Replace `new()`**

In `crates/atuin-ai/src/store.rs`, replace `new` (lines 58-93) with:

```rust
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();

        // AI session tokens/content live here, so restrict the file to 0o600.
        let pool = atuin_common::sqlite::pool(path, timeout)
            .restrict_permissions(true)
            .open()
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }
```

- [ ] **Step 3: Trim imports**

Update the sqlx import (line 6) — the entire line is now unused (no `Sqlite*` connect types referenced in this file's `new`). Verify with `cargo build -p atuin-ai` whether any other function in `store.rs` uses `SqlitePool` (the struct field type almost certainly does). If `SqlitePool` is still referenced, reduce the import to:

```rust
use sqlx::sqlite::SqlitePool;
```

Otherwise remove the line entirely. Then remove any other newly-unused imports the build flags (candidates: `std::str::FromStr` line 2, `std::time::Duration` line 3, and the `eyre!` name at line 5 if unused). Let `cargo clippy -p atuin-ai --all-targets -- -D warnings` be the arbiter.

- [ ] **Step 4: Build, test, clippy**

Run: `cargo test -p atuin-ai --lib` then `cargo clippy -p atuin-ai --all-targets -- -D warnings`
Expected: pass; clippy clean.

- [ ] **Step 5: Full workspace verification**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace --lib`
Expected: clean build, no warnings, all lib tests pass. Then confirm no store still constructs `SqliteConnectOptions` directly:

Run: `grep -rn "SqliteConnectOptions" crates/atuin-client crates/atuin-kv crates/atuin-scripts crates/atuin-ai`
Expected: NO matches (all six stores now go through `atuin_common::sqlite`).

- [ ] **Step 6: Commit**

```bash
git add crates/atuin-ai/Cargo.toml crates/atuin-ai/src/store.rs
git commit -m "refactor(ai): open session store via atuin_common::sqlite"
```

---

## Self-Review

- **Spec coverage:** The shared builder + feature is Task 1; all six stores are migrated (Tasks 2-7), each with its exact pragma set per the Global Constraints table. `sqlite_version()` is consolidated for the three stores that expose it (history, kv, scripts). Migrations stay per-store (they use the `sqlx::migrate!` macro, which must expand at the call site). The `sqlite` feature is defined once and enabled by each consuming crate.
- **Behavior preservation:** The builder is faithful to each store's method calls — `synchronous` is `Option` (meta/ai leave it unset ⇒ sqlx default FULL, matching today; the rest set NORMAL), `foreign_keys`/`regexp` are opt-in (only the stores that called them set them), `optimize_on_close`/`create_if_missing` are universal. The `:memory:` skip and `0o600` restriction match meta/ai exactly. Verified per-store against the extracted current code.
- **Intentional change flagged:** broken-symlink `process::exit(1)` → returned `sqlx::Error`, and meta/ai gain the (previously absent) broken-symlink guard. Called out in Global Constraints and to be noted in the PR.
- **Type consistency:** `pool()` returns `PoolBuilder<'_>`; every builder method takes `self`→`Self`; the terminal `open()` and `version()` return `sqlx::Result<_>`. Stores whose `new()` returns `sqlx::Result` propagate `open().await?` directly; stores returning `eyre::Result` absorb the `sqlx::Error` via `?` (eyre `From<Error>`). Both compile.
- **Feature wiring:** `sqlite = ["sqlx/sqlite", "sqlx/regexp"]` (sqlx is already a non-optional dep). Each consumer adds `sqlite` to its `atuin-common` features. The `regexp` capability is now encapsulated in `atuin-common` — kv/scripts no longer rely on cross-crate sqlx feature unification to reach `.with_regexp()`.
- **Placeholder scan:** No TBD/TODO; every code step shows complete code, exact commands, expected results.
