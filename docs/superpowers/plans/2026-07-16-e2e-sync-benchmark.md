# End-to-End Sync Benchmark Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a benchmark that measures atuin's record-sync throughput end-to-end — a real `atuin-client` talking HTTP over a real TCP socket to a real in-process `atuin-server` — so that changes to sync paging (such as PR #3584's 100 → 1000 page size) can be justified with numbers.

**Architecture:** A divan benchmark in `crates/atuin-client/benches/sync.rs` starts an in-process `atuin-server` backed by a temp-file SQLite database, bound to `127.0.0.1:0`. Because loopback RTT is effectively zero — and RTT is precisely the cost that a larger page size eliminates — the server's axum router is wrapped in a middleware that sleeps for a configurable round-trip time before handling each request. The benchmark sweeps the cross product of `page_size × rtt`, driving `diff` → `operations` → `sync_remote` directly rather than `sync::sync`, because `sync_remote` takes `page_size` as a parameter and `sync::sync` hardcodes it.

**Tech Stack:** Rust (edition 2024), divan 0.1 (locked at 0.1.21), axum 0.8, tokio, sqlx/SQLite, tempfile.

## Global Constraints

- Rust edition `2024`, `rust-version = "1.97.0"` (workspace-wide).
- `crates/atuin-client/Cargo.toml` sets `autobenches = false` and declares a single `[[bench]]` target named `benchmarks` with `harness = false`. **Every client bench must be registered as a `mod` in `crates/atuin-client/benches/benchmarks.rs`** or it will not run.
- Benchmarks must be deterministic. Use `_util::context::BenchCtx` for all randomness. A bare `rand` accessor is an anti-pattern here — the existing `BenchCtx` doc comments say so explicitly.
- The benchmark's axum dependency must resolve to the same version as `atuin-server`'s (0.8.8) so that `axum::Router` is the same type in both crates. Declare it as `axum = "0.8"`.
- Do **not** use `sqlite::memory:` for the server database. `atuin-server-sqlite/src/lib.rs:30` builds its pool with `SqlitePoolOptions::new()` and never sets `max_connections(1)`, so each pooled connection would get its own private in-memory database: migrations would run on one connection and queries would hit another and find no schema. Use a `sqlite://` file inside a `tempfile::TempDir`.
- The server accepts only `Token` authorization (`crates/atuin-server/src/router.rs:50` rejects anything else). Use `api_client::AuthToken::Token`, never `Bearer`.
- `Store` is not object-safe (`push_batch` takes `impl Iterator`), so all sync functions are generic over `&impl Store`. Never try to box it.

---

## Background: what we are measuring and why this shape

PR #3584 changed exactly one line, `crates/atuin-client/src/record/sync.rs:368`:

```rust
let (uploaded, downloaded) = sync_remote(&client, operations, store, 100).await?;
//                                                                     ^^^ -> 1000
```

`sync_upload` and `sync_download` loop one HTTP request per page. Cutting page count by 10x saves 10x the round-trips. On loopback a round-trip costs microseconds, so a naive localhost benchmark would measure SQLite and serialization — not the thing the change improves — and would report a rounding error. The injected-latency middleware is what makes the benchmark answer the actual question, while keeping real sockets, real HTTP, real JSON, and both real databases in the loop.

We drive `diff` → `operations` → `sync_remote` rather than `sync::sync` for three reasons:

1. `sync_remote` takes `page_size`; `sync::sync` hardcodes `100`. It is the only seam that lets us sweep it.
2. `sync::sync` resolves its auth token from the *meta store* (`settings.sync_auth_token()` → `resolve_sync_auth()`), a separate SQLite database gated behind a process-wide `OnceCell` (`settings::init_meta_config_for_testing`, `crates/atuin-client/src/settings.rs:1773`). A `OnceCell` can be set once per process, which is incompatible with a benchmark that starts several servers.
3. `sync::sync` calls `encryption::load_key`, which reads — and *creates* — a key file on disk.

Timing `sync_remote` alone also matches the agreed coverage exactly: upload and download, without the fixed `record_status`/`diff` overhead folded in.

### Constraints discovered during design — verify these hold

These came out of reading the server and are worth confirming with the benchmark, because they bound how far page size can go:

- **axum's default 2 MB body limit caps page size.** `handlers::v0::record::post` extracts `Json(records): Json<Vec<Record<EncryptedData>>>` (`crates/atuin-server/src/handlers/v0/record.rs:18`) and `atuin-server` never installs a `DefaultBodyLimit` override, so axum's 2 MB default applies. At this benchmark's ~600 bytes/record, `page_size = 1000` is ~600 KB — comfortably under. **But a real user with long commands can have 1–2 KB records, where 1000 records is 1–2 MB and can trip a `413`.** This is a genuine risk for the page-size change and should be raised in the redesign.
- **`max_record_size` is per record, not per request** (`crates/atuin-server/src/handlers/v0/record.rs:31`), so it does not bound page size.
- **No server-side page-size cap exists in this tree.** The PR description mentioned a server default of 1100; `grep` finds no such constant. If a cap existed it would show as a plateau in the results.

### Known measurement caveats — document, do not fix here

`sync_upload` and `sync_download` unconditionally `println!` and build an `indicatif::ProgressBar` (`sync.rs:182-193`, `sync.rs:242-253`). There is no quiet flag.

- The `println!` fires once per operation and will interleave with divan's output tree. Cosmetic.
- The progress bar is the reproducibility hazard: indicatif draws to stderr and suppresses itself when stderr is not a TTY. So the same benchmark measures *more* work on a developer's terminal than in CI, and it penalizes small pages extra (one `set_position` per page). **Always redirect stderr to a file when benchmarking** (Task 6 documents this). Making sync's progress reporting injectable is a good follow-up, but changing shipping code to suit the benchmark is out of scope here.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/atuin-server/src/lib.rs` | Make `make_router` public so a test/benchmark can compose middleware onto the router. |
| `crates/atuin-server/tests/make_router.rs` | New. Proves `make_router` is public and builds a working router on the SQLite backend. |
| `crates/atuin-client/benches/_util/record.rs` | New. `BenchRecord`, moved out of `record/sqlite_store.rs` so both the store bench and the sync bench can build record chains. |
| `crates/atuin-client/benches/_util/server.rs` | New. `BenchServer`: in-process server on a temp-file SQLite DB, latency middleware, registration. |
| `crates/atuin-client/benches/_util/mod.rs` | Declares `context`, `record`, `server`. |
| `crates/atuin-client/benches/record/sqlite_store.rs` | Loses `BenchRecord`, imports it instead. |
| `crates/atuin-client/benches/sync.rs` | New. The `upload` and `download` benchmarks. |
| `crates/atuin-client/benches/benchmarks.rs` | Registers `mod sync;`. |
| `crates/atuin-client/tests/bench_harness.rs` | New. Real `#[tokio::test]` coverage of `BenchServer`, included via `#[path]`. Benches with `harness = false` cannot host `#[test]` functions, so this is how the harness gets tested. |

---

### Task 1: Extract `BenchRecord` into a shared bench utility

Pure refactor, no behavior change. `BenchRecord` currently lives inside `benches/record/sqlite_store.rs` as a private struct; the sync bench needs it too.

**Files:**
- Create: `crates/atuin-client/benches/_util/record.rs`
- Modify: `crates/atuin-client/benches/_util/mod.rs`
- Modify: `crates/atuin-client/benches/record/sqlite_store.rs`

**Interfaces:**
- Consumes: `_util::context::BenchCtx` (existing, unchanged).
- Produces: `pub struct BenchRecord` with `pub fn chain(ctx: &mut BenchCtx, n: usize) -> Vec<Record<EncryptedData>>`. All records in a chain share one `Host` and one `tag`, with `idx` running `0..n`. Callers recover the pair via `records[0].host.id` and `records[0].tag`.

- [ ] **Step 1: Create the shared record module**

Create `crates/atuin-client/benches/_util/record.rs` with the body moved verbatim from `record/sqlite_store.rs`, with `BenchRecord` and `chain` made `pub`:

```rust
use atuin_common::record::{EncryptedData, Host, HostId, Record};
use atuin_common::utils::uuid_v7;
use rand::Rng;
use rand::distributions::Alphanumeric;

use crate::_util::context::BenchCtx;

pub struct BenchRecord;

impl BenchRecord {
    /// Controls how large the record payload is. Roughly, this is between 200 and 400 bytes for
    /// a typical history record.
    ///
    /// Breakdown:
    ///  - id (UUID string, 36 bytes)
    ///  - timestamp (u64, 8 bytes)
    ///  - duration (i64, 8 bytes)
    ///  - exit code (i64, 8 bytes)
    ///  - command (variable — average shell command is ~20-50 bytes, but can be much longer)
    ///  - cwd (path string, ~20-60 bytes)
    ///  - session (string, ~36 bytes)
    ///  - hostname (string, ~10-30 bytes)
    ///  - deleted_at (optional u64)
    ///  - author (string)
    const PAYLOAD_SIZE: usize = 300;

    /// Rough size of the PASETO PIE-wrapped key.
    const KEY_SIZE: usize = 150;

    /// Build a chain of `n` records sharing a single host and tag, with `idx` running `0..n`.
    ///
    /// The payloads are random bytes rather than real PASETO ciphertext. Nothing on either the
    /// upload or download path decrypts a record, so this is indistinguishable to the code under
    /// test while being far cheaper to produce.
    pub fn chain(ctx: &mut BenchCtx, n: usize) -> Vec<Record<EncryptedData>> {
        let host = Host::new(HostId(uuid_v7()));
        let version: String = "v1".into();
        let tag = uuid_v7().simple().to_string();
        let data: String = ctx
            .rng()
            .sample_iter(&Alphanumeric)
            .take(Self::PAYLOAD_SIZE)
            .map(char::from)
            .collect();
        let key: String = ctx
            .rng()
            .sample_iter(&Alphanumeric)
            .take(Self::KEY_SIZE)
            .map(char::from)
            .collect();

        (0..n as u64)
            .map(|idx| {
                Record::builder()
                    .host(host.clone())
                    .version(version.clone())
                    .tag(tag.clone())
                    .data(EncryptedData {
                        data: data.clone(),
                        content_encryption_key: key.clone(),
                    })
                    .idx(idx)
                    .build()
            })
            .collect()
    }
}
```

- [ ] **Step 2: Register the module and allow dead code**

Replace the entire contents of `crates/atuin-client/benches/_util/mod.rs` with:

```rust
// This module is shared between the `benchmarks` bench target and `tests/bench_harness.rs`.
// Each target uses a different subset of it, so unused items are expected.
#![allow(dead_code)]

pub mod context;
pub mod record;
```

The inner `#![allow(dead_code)]` applies to `_util` and all of its children. Without it, `tests/bench_harness.rs` (Task 3) warns about `BenchCtx::now`, which it does not use.

- [ ] **Step 3: Point the store bench at the shared module**

In `crates/atuin-client/benches/record/sqlite_store.rs`, delete the entire `struct BenchRecord;` declaration and its `impl BenchRecord { ... }` block (everything from `struct BenchRecord;` down to the closing brace before `struct BenchSqliteStore`), then replace the import block at the top of the file:

```rust
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::store::Store;
use atuin_common::record::{EncryptedData, Host, HostId, Record};
use atuin_common::utils::uuid_v7;
use rand::Rng;
use rand::distributions::Alphanumeric;
use tempfile::TempDir;

use crate::_util::context::BenchCtx;
```

with:

```rust
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::store::Store;
use tempfile::TempDir;

use crate::_util::context::BenchCtx;
use crate::_util::record::BenchRecord;
```

- [ ] **Step 4: Verify the refactor compiles and the existing bench still runs**

Run: `cargo bench -p atuin-client --bench benchmarks -- record::sqlite_store::push_batch 2>/dev/null`

Expected: compiles with no warnings, and divan prints a `push_batch` tree with rows for args `1`, `10`, and `100`, each showing a median time. Timings should be unchanged from before the refactor — this task moves code and nothing else.

- [ ] **Step 5: Commit**

```bash
git add crates/atuin-client/benches/_util/mod.rs crates/atuin-client/benches/_util/record.rs crates/atuin-client/benches/record/sqlite_store.rs
git commit -m "refactor(bench): extract BenchRecord into a shared bench utility"
```

---

### Task 2: Make `atuin_server::make_router` public

The benchmark needs a `Router` it can wrap in latency middleware before serving. `launch_with_tcp_listener` builds the router internally and gives no seam to layer onto; `make_router` (`crates/atuin-server/src/lib.rs:91`) is exactly the right seam but is private.

**Files:**
- Modify: `crates/atuin-server/src/lib.rs:91`
- Modify: `crates/atuin-server/Cargo.toml` (add a `[dev-dependencies]` section — the crate has none today)
- Test: `crates/atuin-server/tests/make_router.rs`

**Interfaces:**
- Produces: `pub async fn make_router<Db: Database>(settings: Settings) -> Result<Router, eyre::Error>` — used by `_util::server::BenchServer` in Task 3.

- [ ] **Step 1: Add the dev-dependencies the test needs**

Append to `crates/atuin-server/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
# `util` enables `tower::ServiceExt::oneshot`, which drives the router without a socket.
tower = { version = "0.5", features = ["util"] }
```

`tower` is already a normal dependency at workspace version `0.5`; cargo unifies the feature sets, so this only adds `util`.

- [ ] **Step 2: Write the failing test**

Create `crates/atuin-server/tests/make_router.rs`:

```rust
use atuin_server::{Settings, make_router, settings::Metrics};
use atuin_server_database::DbSettings;
use atuin_server_sqlite::Sqlite;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// `make_router` is the seam benchmarks and tests use to compose middleware onto the server
/// before serving it. This proves it is reachable from outside the crate and that it produces a
/// working router against the SQLite backend.
#[tokio::test]
async fn make_router_builds_a_serviceable_router() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("server.db");

    let settings = Settings {
        host: "127.0.0.1".to_owned(),
        port: 0,
        path: String::new(),
        open_registration: true,
        max_record_size: 1024 * 1024 * 1024,
        register_webhook_url: None,
        register_webhook_username: String::new(),
        db_settings: DbSettings {
            db_uri: format!("sqlite://{}", db_path.display()),
            read_db_uri: None,
        },
        metrics: Metrics::default(),
        fake_version: None,
    };

    let router = make_router::<Sqlite>(settings).await.unwrap();

    let response = router
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p atuin-server --test make_router`

Expected: FAIL to compile with `error[E0603]: function \`make_router\` is private`.

- [ ] **Step 4: Make the function public**

In `crates/atuin-server/src/lib.rs:91`, change:

```rust
async fn make_router<Db: Database>(settings: Settings) -> Result<Router, eyre::Error> {
```

to:

```rust
/// Build the server's `Router` without serving it.
///
/// Exposed so that tests and benchmarks can compose additional middleware onto the router — for
/// example a latency injector — before handing it to [`axum::serve`]. Most callers want
/// [`launch`] or [`launch_with_tcp_listener`] instead.
pub async fn make_router<Db: Database>(settings: Settings) -> Result<Router, eyre::Error> {
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p atuin-server --test make_router`

Expected: PASS — `test make_router_builds_a_serviceable_router ... ok`, `1 passed`.

- [ ] **Step 6: Commit**

```bash
git add crates/atuin-server/src/lib.rs crates/atuin-server/Cargo.toml crates/atuin-server/tests/make_router.rs
git commit -m "feat(server): expose make_router so tests and benches can layer middleware"
```

---

### Task 3: `BenchServer` — an in-process server with injectable latency

**Files:**
- Create: `crates/atuin-client/benches/_util/server.rs`
- Modify: `crates/atuin-client/benches/_util/mod.rs`
- Modify: `crates/atuin-client/Cargo.toml` (dev-dependencies)
- Test: `crates/atuin-client/tests/bench_harness.rs`

**Interfaces:**
- Consumes: `atuin_server::make_router` (Task 2); `_util::context::BenchCtx` and `_util::record::BenchRecord` (Task 1).
- Produces:
  - `pub struct BenchServer` with `pub async fn start(rtt: Duration) -> BenchServer` and `pub async fn register(&self) -> atuin_client::api_client::Client<'_>`. No public `address()` accessor: nothing outside the struct needs the address, since `register` is the only way in.
  - `BenchServer` shuts its task down in `Drop`. There is deliberately no `shutdown()` method: `register` returns a `Client<'_>` borrowing the server's address `String`, so a by-value `shutdown(self)` would conflict with the outstanding borrow.

- [ ] **Step 1: Add the dev-dependencies**

In `crates/atuin-client/Cargo.toml`, replace the `[dev-dependencies]` section:

```toml
[dev-dependencies]
tokio = { version = "1", features = ["full"] }
pretty_assertions = { workspace = true }
divan = "0.1.14"
tempfile = "3"
```

with:

```toml
[dev-dependencies]
tokio = { version = "1", features = ["full"] }
pretty_assertions = { workspace = true }
divan = "0.1.14"
tempfile = "3"

# The end-to-end sync benchmark (benches/sync.rs) runs a real server in-process and talks to it
# over a real socket. `atuin-server` does not depend on `atuin-client`, so this is not a cycle.
# `axum` must match the version `atuin-server` uses (0.8) so that `Router` is the same type.
atuin-server = { workspace = true }
atuin-server-database = { workspace = true }
atuin-server-sqlite = { workspace = true }
axum = "0.8"
```

- [ ] **Step 2: Write the failing test**

Create `crates/atuin-client/tests/bench_harness.rs`:

```rust
//! Tests for the benchmark harness in `benches/_util`.
//!
//! The `benchmarks` bench target sets `harness = false`, so it cannot host `#[test]` functions.
//! Including the modules by path here gives the harness real test coverage — a benchmark built on
//! a broken harness produces confident, wrong numbers.

#[path = "../benches/_util/mod.rs"]
mod _util;

use std::time::Duration;

use _util::context::BenchCtx;
use _util::record::BenchRecord;
use _util::server::BenchServer;

#[tokio::test]
async fn registers_a_user_and_round_trips_records() {
    let server = BenchServer::start(Duration::ZERO).await;
    let client = server.register().await;

    assert!(client.me().await.is_ok(), "session token should work");

    let mut ctx = BenchCtx::new();
    let records = BenchRecord::chain(&mut ctx, 5);
    let host = records[0].host.id;
    let tag = records[0].tag.clone();

    client.post_records(&records).await.unwrap();

    // `RecordStatus.hosts` maps host -> tag -> max(idx), so a 5-record chain reports 4.
    let status = client.record_status().await.unwrap();
    assert_eq!(status.get(host, tag.clone()), Some(4));

    let downloaded = client.next_records(host, tag, 0, 10).await.unwrap();
    assert_eq!(downloaded.len(), 5);
}

#[tokio::test]
async fn delete_store_resets_server_state() {
    let server = BenchServer::start(Duration::ZERO).await;
    let client = server.register().await;

    let mut ctx = BenchCtx::new();
    let records = BenchRecord::chain(&mut ctx, 5);
    client.post_records(&records).await.unwrap();
    assert!(!client.record_status().await.unwrap().hosts.is_empty());

    // The upload benchmark relies on this to give every sample an empty server.
    client.delete_store().await.unwrap();
    assert!(client.record_status().await.unwrap().hosts.is_empty());
}

#[tokio::test]
async fn latency_layer_delays_every_request() {
    let rtt = Duration::from_millis(100);
    let server = BenchServer::start(rtt).await;
    let client = server.register().await;

    let start = std::time::Instant::now();
    client.me().await.unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed >= rtt,
        "expected at least one injected RTT of {rtt:?}, took {elapsed:?}"
    );
}
```

Note there is deliberately **no** `zero_latency_does_not_delay` test asserting a wall-clock *upper* bound. Such a test flakes under load, and it would not protect the `if !rtt.is_zero()` guard it appears to: `tokio::time::sleep(Duration::ZERO)` returns immediately, so deleting the guard would not fail it. The zero-RTT path is covered end-to-end by `registers_a_user_and_round_trips_records`; the middleware is covered by the lower-bound assertion above, which has a wide margin and does not flake.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p atuin-client --test bench_harness`

Expected: FAIL to compile with `error[E0583]: file not found for module \`server\`` (or `unresolved import \`_util::server\``).

- [ ] **Step 4: Write the server harness**

Create `crates/atuin-client/benches/_util/server.rs`:

```rust
use std::net::SocketAddr;
use std::time::Duration;

use atuin_client::api_client::{self, AuthToken, Client};
use atuin_common::utils::uuid_v7;
use atuin_server::settings::Metrics;
use atuin_server::{Settings as ServerSettings, make_router};
use atuin_server_database::DbSettings;
use atuin_server_sqlite::Sqlite;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Sleeps for one round-trip time before handling each request.
///
/// Loopback RTT is effectively zero, which is exactly the cost that a larger sync page size
/// removes — so without this, a localhost benchmark measures SQLite and serialization and reports
/// the paging change as noise. Delaying per *request* rather than per byte is deliberate: it
/// models one RTT per round-trip regardless of body size, whereas a byte-level delay would charge
/// a large page once per TCP segment and invert the result.
///
/// This models latency only. Bandwidth is still loopback-fast, and connection setup is not
/// delayed (reqwest keep-alive means that happens once per benchmark anyway).
async fn latency(State(rtt): State<Duration>, request: Request, next: Next) -> Response {
    if !rtt.is_zero() {
        tokio::time::sleep(rtt).await;
    }

    next.run(request).await
}

/// A real `atuin-server` running in-process, reachable over a real TCP socket on loopback.
pub struct BenchServer {
    address: String,
    // Holds the server's SQLite file. Dropped, and so deleted, with the server.
    _db_dir: TempDir,
    handle: JoinHandle<()>,
}

impl BenchServer {
    const CONNECT_TIMEOUT_S: u64 = 5;

    /// Generous: a page at a high injected RTT is slow, and a timeout mid-benchmark would show up
    /// as a confusing panic rather than a slow number.
    const TIMEOUT_S: u64 = 120;

    /// Start a server that sleeps `rtt` before handling each request.
    pub async fn start(rtt: Duration) -> Self {
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("server.db");

        let settings = ServerSettings {
            host: "127.0.0.1".to_owned(),
            port: 0,
            path: String::new(),
            open_registration: true,
            max_record_size: 1024 * 1024 * 1024,
            register_webhook_url: None,
            register_webhook_username: String::new(),
            db_settings: DbSettings {
                // Not `sqlite::memory:` — the server's pool would hand each connection its own
                // private database, so migrations and queries would disagree about the schema.
                db_uri: format!("sqlite://{}", db_path.display()),
                read_db_uri: None,
            },
            metrics: Metrics::default(),
            fake_version: None,
        };

        let router = make_router::<Sqlite>(settings)
            .await
            .unwrap()
            .layer(middleware::from_fn_with_state(rtt, latency));

        // Bind before spawning: the socket is listening the moment `start` returns, so callers
        // never need to sleep and hope the server came up.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service())
                .await
                .expect("bench server failed");
        });

        Self {
            address: format!("http://{addr}"),
            _db_dir: db_dir,
            handle,
        }
    }

    /// Register a fresh user and return a client authenticated as them.
    pub async fn register(&self) -> Client<'_> {
        let username = uuid_v7().as_simple().to_string();
        let email = format!("{username}@example.com");
        let password = uuid_v7().as_simple().to_string();

        let registration = api_client::register(&self.address, &username, &email, &password)
            .await
            .unwrap();

        // The server rejects any scheme other than `Token` (atuin-server/src/router.rs:50).
        Client::new(
            &self.address,
            AuthToken::Token(registration.session),
            Self::CONNECT_TIMEOUT_S,
            Self::TIMEOUT_S,
        )
        .unwrap()
    }
}

impl Drop for BenchServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
```

- [ ] **Step 5: Register the module**

In `crates/atuin-client/benches/_util/mod.rs`, add `pub mod server;` so the file reads:

```rust
// This module is shared between the `benchmarks` bench target and `tests/bench_harness.rs`.
// Each target uses a different subset of it, so unused items are expected.
#![allow(dead_code)]

pub mod context;
pub mod record;
pub mod server;
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p atuin-client --test bench_harness`

Expected: PASS — `3 passed; 0 failed`.

If `registers_a_user_and_round_trips_records` fails on `record_status` with a version error, the `semver` middleware is not attaching `Atuin-Version`; that means the latency layer was composed onto the wrong router. It must be `.layer(...)` on the value returned by `make_router`, which wraps that crate's middleware stack rather than replacing it.

- [ ] **Step 7: Commit**

```bash
git add crates/atuin-client/Cargo.toml crates/atuin-client/benches/_util/mod.rs crates/atuin-client/benches/_util/server.rs crates/atuin-client/tests/bench_harness.rs
git commit -m "feat(bench): add in-process server harness with injectable latency"
```

---

### Task 4: The upload benchmark

**Files:**
- Create: `crates/atuin-client/benches/sync.rs`
- Modify: `crates/atuin-client/benches/benchmarks.rs`

**Interfaces:**
- Consumes: `BenchServer::start` / `BenchServer::register` (Task 3), `BenchRecord::chain` (Task 1), `BenchCtx::new` (existing); `atuin_client::record::sync::{diff, operations, sync_remote}`.
- Produces: `struct SyncArg { page_size: u64, rtt_ms: u64 }` and `const ARGS: [SyncArg; 6]`, both reused by Task 5's download benchmark.

Divan requires an `args` element type to be `Any + Copy + Send + Sync` and either `ToString` or `Debug`, and passes it **by value**. A tuple would satisfy that but would be labelled `(100, 0)`; a `Copy` struct with a `Display` impl labels rows `page=100 rtt=0ms`.

- [ ] **Step 1: Write the failing benchmark**

Create `crates/atuin-client/benches/sync.rs`:

```rust
//! End-to-end record sync benchmarks.
//!
//! These drive a real `atuin-client` against a real in-process `atuin-server` over a real TCP
//! socket on loopback, with a configurable round-trip time injected server-side. See
//! `_util::server::latency` for why the RTT is injected and what it does and does not model.
//!
//! They benchmark `sync_remote` rather than `sync::sync` because `sync_remote` takes `page_size`
//! as a parameter while `sync::sync` hardcodes it (`record/sync.rs:368`) — and because
//! `sync::sync` resolves its auth token through a process-wide `OnceCell` meta store, which a
//! benchmark that starts several servers cannot use.
//!
//! `diff` and `operations` run in untimed setup, so each sample times the paged transfer alone.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p atuin-client --bench benchmarks -- sync 2>/tmp/atuin-bench.stderr
//! ```
//!
//! Redirecting stderr is not optional. `sync_upload`/`sync_download` draw an `indicatif` progress
//! bar to stderr, which indicatif suppresses only when stderr is not a TTY. Left attached to a
//! terminal it adds real, page-count-proportional work, so a terminal run and a CI run measure
//! different things.

use std::time::Duration;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::store::Store;
use atuin_client::record::sync::{diff, operations, sync_remote};

use crate::_util::context::BenchCtx;
use crate::_util::record::BenchRecord;
use crate::_util::server::BenchServer;

/// The size of the corpus each sample transfers. Chosen to resemble a first sync of a real shell
/// history, and to make the page-count difference between 100 and 1000 large (100 requests vs 10).
const RECORDS: usize = 10_000;

/// Matches the timeout the sqlite_store bench uses. `settings::test_local_timeout` is
/// `#[cfg(test)]`, so it is not reachable from a bench target.
const SQL_TIMEOUT_S: f64 = 5.0;

#[derive(Clone, Copy)]
pub struct SyncArg {
    /// The `page_size` handed to `sync_remote`. 100 is what `sync::sync` uses today; 1000 is what
    /// PR #3584 proposed.
    pub page_size: u64,
    /// Injected server-side round-trip time. 0 is bare loopback, 20ms a same-region server, 100ms
    /// a cross-continent one.
    pub rtt_ms: u64,
}

impl std::fmt::Display for SyncArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "page={} rtt={}ms", self.page_size, self.rtt_ms)
    }
}

pub const ARGS: [SyncArg; 6] = [
    SyncArg { page_size: 100, rtt_ms: 0 },
    SyncArg { page_size: 1000, rtt_ms: 0 },
    SyncArg { page_size: 100, rtt_ms: 20 },
    SyncArg { page_size: 1000, rtt_ms: 20 },
    SyncArg { page_size: 100, rtt_ms: 100 },
    SyncArg { page_size: 1000, rtt_ms: 100 },
];

/// Client holds `RECORDS` records, server holds none: the full corpus goes up.
///
/// `sample_size = 1` is required — divan would otherwise calibrate a sample size against a
/// multi-second iteration and run for a very long time.
#[divan::bench(args = ARGS, sample_count = 5, sample_size = 1)]
fn upload(bencher: divan::Bencher, arg: SyncArg) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(BenchServer::start(Duration::from_millis(arg.rtt_ms)));
    let client = rt.block_on(server.register());

    bencher
        .with_inputs(|| {
            rt.block_on(async {
                // Uploading mutates the server, so every sample needs it emptied first.
                client.delete_store().await.unwrap();

                let dir = tempfile::tempdir().unwrap();
                let store = SqliteStore::new(dir.path().join("records.db"), SQL_TIMEOUT_S)
                    .await
                    .unwrap();

                let mut ctx = BenchCtx::new();
                let records = BenchRecord::chain(&mut ctx, RECORDS);
                store.push_batch(records.iter()).await.unwrap();

                let (diffs, _) = diff(&client, &store).await.unwrap();
                let ops = operations(diffs, &store).await.unwrap();

                (dir, store, ops)
            })
        })
        .bench_values(|(_dir, store, ops)| {
            let (uploaded, _) = rt
                .block_on(sync_remote(&client, ops, &store, arg.page_size))
                .unwrap();

            // A silently empty sync would benchmark nothing at all, very quickly.
            assert_eq!(uploaded, RECORDS as i64);
        });
}
```

- [ ] **Step 2: Register the benchmark**

Replace the contents of `crates/atuin-client/benches/benchmarks.rs` with:

```rust
mod _util;
mod history;
mod ordering;
mod record;
mod sync;

fn main() {
    divan::main();
}
```

- [ ] **Step 3: Run the benchmark and check the shape of the result**

Run: `cargo bench -p atuin-client --bench benchmarks -- sync::upload 2>/tmp/atuin-bench.stderr`

Expected: a divan tree with six rows under `upload`, labelled `page=100 rtt=0ms` through `page=1000 rtt=100ms`, each with a median time. Interleaved `Uploading 10000 records to ...` lines from `sync_upload`'s `println!` are expected and harmless.

Expect roughly (exact numbers are machine-dependent; the *ratios* are the point):

| arg | round-trips | rough median |
| --- | --- | --- |
| `page=100 rtt=0ms` | 100 | baseline |
| `page=1000 rtt=0ms` | 10 | modestly faster than baseline |
| `page=100 rtt=20ms` | 100 | ≥ 2s |
| `page=1000 rtt=20ms` | 10 | ≈ 0.2s + work |
| `page=100 rtt=100ms` | 100 | ≥ 10s |
| `page=1000 rtt=100ms` | 10 | ≈ 1s + work |

The whole `sync` filter takes a few minutes, most of it in the `page=100 rtt=100ms` rows. That is the benchmark working as designed: it is the cost the change removes.

If a row reports microseconds, the assertion did not fire but the sync did nothing — check that `delete_store` is running in setup.

- [ ] **Step 4: Verify the assertion catches a broken sync**

This is the benchmark's equivalent of watching a test fail first: it proves the timed section really moves 10,000 records rather than short-circuiting on an empty diff. Temporarily replace the assertion line `assert_eq!(uploaded, RECORDS as i64);` with `assert_eq!(uploaded, 10_001);` and run:

Run: `cargo bench -p atuin-client --bench benchmarks -- sync::upload 2>/tmp/atuin-bench.stderr`

Expected: panic with `assertion \`left == right\` failed: left: 10000, right: 10001`. This proves the benchmark is really transferring the corpus rather than short-circuiting. Revert the change afterwards and re-run Step 3 to confirm it passes again.

- [ ] **Step 5: Commit**

```bash
git add crates/atuin-client/benches/sync.rs crates/atuin-client/benches/benchmarks.rs
git commit -m "feat(bench): add end-to-end sync upload benchmark"
```

---

### Task 5: The download benchmark

**Files:**
- Modify: `crates/atuin-client/benches/sync.rs`

**Interfaces:**
- Consumes: `SyncArg`, `ARGS`, `RECORDS`, `SQL_TIMEOUT_S` from Task 4; `BenchServer`, `BenchRecord`, `BenchCtx`.
- Produces: `const SEED_CHUNK: usize`. Nothing consumed by later tasks.

Downloading never mutates the server, so unlike upload the corpus is seeded **once per arg** and every sample reads it. Only the client's store needs recreating per sample.

- [ ] **Step 1: Write the benchmark**

Append to `crates/atuin-client/benches/sync.rs`. `SEED_CHUNK` is defined here rather than alongside the other constants in Task 4 because only this benchmark seeds the server; defining it earlier would leave it unused and warn:

```rust
/// Keeps each seeding request under axum's default 2 MB body limit.
const SEED_CHUNK: usize = 1_000;

/// Server holds `RECORDS` records, client holds none: the full corpus comes down.
///
/// Downloading does not mutate the server, so the corpus is seeded once per arg rather than per
/// sample — only the client's store is rebuilt each time.
#[divan::bench(args = ARGS, sample_count = 5, sample_size = 1)]
fn download(bencher: divan::Bencher, arg: SyncArg) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(BenchServer::start(Duration::from_millis(arg.rtt_ms)));
    let client = rt.block_on(server.register());

    rt.block_on(async {
        let mut ctx = BenchCtx::new();
        let records = BenchRecord::chain(&mut ctx, RECORDS);

        // Chunked to stay under axum's default 2 MB request body limit.
        for chunk in records.chunks(SEED_CHUNK) {
            client.post_records(chunk).await.unwrap();
        }
    });

    bencher
        .with_inputs(|| {
            rt.block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let store = SqliteStore::new(dir.path().join("records.db"), SQL_TIMEOUT_S)
                    .await
                    .unwrap();

                let (diffs, _) = diff(&client, &store).await.unwrap();
                let ops = operations(diffs, &store).await.unwrap();

                (dir, store, ops)
            })
        })
        .bench_values(|(_dir, store, ops)| {
            let (_, downloaded) = rt
                .block_on(sync_remote(&client, ops, &store, arg.page_size))
                .unwrap();

            assert_eq!(downloaded.len(), RECORDS);
        });
}
```

- [ ] **Step 2: Run the benchmark to verify it works**

Run: `cargo bench -p atuin-client --bench benchmarks -- sync::download 2>/tmp/atuin-bench.stderr`

Expected: six rows under `download`, same labels as `upload`, same shape of result — `page=1000` roughly 10x fewer round-trips than `page=100`, with the gap widening as `rtt_ms` grows.

Do not expect a particular download-vs-upload ordering at `rtt=0ms`. Both directions write 10,000 rows into a WAL SQLite (upload into the server's, download into the client's), so there is no a priori reason for either to dominate. Measured on this branch, download is in fact *faster* (149ms/131ms vs upload's 220ms/248ms) — the server's `add_records` path (a `uuid_v7()` per row plus `insert ... on conflict do nothing` with 10 bind params) is heavier than the client's `push_batch`. Record what you measure.

- [ ] **Step 3: Verify the assertion catches a broken sync**

Temporarily change the assertion to `assert_eq!(downloaded.len(), 10_001)` and run:

Run: `cargo bench -p atuin-client --bench benchmarks -- sync::download 2>/tmp/atuin-bench.stderr`

Expected: panic with `assertion \`left == right\` failed: left: 10000, right: 10001`. Revert and re-run Step 2.

- [ ] **Step 4: Run the whole suite once to confirm nothing regressed**

Run: `cargo bench -p atuin-client --bench benchmarks 2>/tmp/atuin-bench.stderr`

Expected: `ordering`, `record::sqlite_store::push_batch`, `sync::upload`, and `sync::download` all report. No panics.

Note `history` does **not** appear: `benches/history.rs` contains no `#[divan::bench]` function — it is a helper module (`BenchHistory`) that `ordering.rs` consumes. This is pre-existing, not something these tasks changed.

- [ ] **Step 5: Commit**

```bash
git add crates/atuin-client/benches/sync.rs
git commit -m "feat(bench): add end-to-end sync download benchmark"
```

---

### Task 6: Record the results and how to reproduce them

A benchmark nobody can interpret is a benchmark nobody will run. This task captures the measured numbers and the caveats needed to trust them.

**Files:**
- Create: `docs/superpowers/plans/2026-07-16-e2e-sync-benchmark-results.md`

- [ ] **Step 1: Collect a clean run**

Run: `cargo bench -p atuin-client --bench benchmarks -- sync 2>/tmp/atuin-bench.stderr | tee /tmp/atuin-sync-bench.txt`

Expected: the full 12-row matrix (upload × 6 args, download × 6 args). Takes several minutes.

- [ ] **Step 2: Write up the results**

Create `docs/superpowers/plans/2026-07-16-e2e-sync-benchmark-results.md` using this structure, filling in the real numbers from Step 1 — do not invent them:

```markdown
# End-to-end sync benchmark: baseline results

Measured on: <hardware, OS, rustc version>
Command: `cargo bench -p atuin-client --bench benchmarks -- sync 2>/tmp/atuin-bench.stderr`
Corpus: 10,000 records, ~600 bytes each on the wire.

## Results

| direction | page size | injected RTT | round-trips | median |
| --- | --- | --- | --- | --- |
| upload | 100 | 0ms | 100 | |
| upload | 1000 | 0ms | 10 | |
| upload | 100 | 20ms | 100 | |
| upload | 1000 | 20ms | 10 | |
| upload | 100 | 100ms | 100 | |
| upload | 1000 | 100ms | 10 | |
| download | 100 | 0ms | 100 | |
| ... | | | | |

## Reading these numbers

- The `rtt=0ms` rows are the honest loopback cost: SQLite, JSON, and HTTP framing. The page-size
  change barely moves them, which is why a plain localhost benchmark cannot justify PR #3584.
- The `rtt=20ms` and `rtt=100ms` rows are where the change pays. The gap between page=100 and
  page=1000 should approach `90 × rtt` — the 90 round-trips removed.
- Latency is injected server-side per request. This models RTT only: bandwidth stays
  loopback-fast, so these numbers are a *lower bound* on the real-world win. A real WAN link also
  pays serialization delay on a 600 KB page, which favours smaller pages slightly.

## Caveats

- **Always redirect stderr.** `sync_upload`/`sync_download` draw an `indicatif` progress bar that
  suppresses itself only when stderr is not a TTY. On a terminal it adds work proportional to page
  count, which distorts exactly the comparison being made.
- The server runs SQLite, not the Postgres that production uses. This keeps server-side variance
  out of the measurement, but it means these numbers do not predict server-side load.
- Payloads are random bytes, not real PASETO ciphertext. Nothing on the sync path decrypts, so
  this is invisible to the code under test — but it does mean record size is a fixed 300-byte
  assumption rather than a real distribution.

## Follow-ups this benchmark surfaced

- **axum's 2 MB default body limit bounds page size.** `handlers::v0::record::post` extracts
  `Json<Vec<Record<EncryptedData>>>` and `atuin-server` never overrides `DefaultBodyLimit`. At
  this benchmark's ~600 bytes/record, page=1000 is ~600 KB and safe. A user with 1–2 KB records
  would send 1–2 MB and could get a `413`. Before raising the page size, either bound the request
  by bytes rather than record count, or raise the server's limit deliberately.
- Sync's progress reporting is not injectable, which is what forces the stderr caveat above.
  Threading a quiet/draw-target option through `sync_remote` would make this benchmark
  reproducible by construction.
```

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/plans/2026-07-16-e2e-sync-benchmark-results.md
git commit -m "docs(bench): record baseline end-to-end sync benchmark results"
```

---

## Appendix: things that will bite an implementer

- **Borrowck around `bencher`.** `server` must outlive `client` (`Client<'a>` borrows the address `String`), and both closures passed to `with_inputs`/`bench_values` capture `&client` and `&rt` immutably at once. That is fine, but a by-value `BenchServer::shutdown(self)` would not be — which is why shutdown lives in `Drop`.
- **Don't reach for `sqlite::memory:`** for the *server* DB. See Global Constraints. The *client* store may use it (the sync unit tests do), but these benchmarks use temp files so that client-side SQLite behaves like it does in production.
- **`api_client::register` does a `GET /user/{username}` first** and bails if it hits. With injected latency that costs two RTTs per registration — untimed setup, so it does not affect results.
- **Divan invokes a bench fn once per arg**, not once per sample, which is why the server can be started at the top of the fn and shared across that arg's samples.
- **`push_batch` is one INSERT per record inside a transaction**, so seeding 10,000 records at once hits no bind-parameter limit.
