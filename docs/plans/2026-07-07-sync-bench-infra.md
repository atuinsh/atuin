# Sync Benchmark Infrastructure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build benchmark infrastructure that exercises record sync over real HTTP, so page size changes (100→1000) show measurable round-trip savings.

**Architecture:** The benchmark starts an in-process axum server backed by an in-memory `Database` implementation (no Postgres needed). A real `reqwest`-based `Client` talks to it over TCP loopback. The benchmark uploads N records, then measures the wall-clock cost of downloading them all at varying page sizes. This isolates HTTP round-trip overhead — the thing the page size change actually optimizes.

The in-memory `Database` lives in a new crate `atuin-bench-support` to keep it reusable across bench targets. The benchmark itself lives in `crates/atuin/benches/` since the top-level `atuin` crate already has `atuin-server`, `atuin-server-database`, and `atuin-server-postgres` as dev-dependencies.

**Tech Stack:** Rust, divan, axum (in-process), reqwest, tokio, `atuin-server`'s `launch_with_tcp_listener`

---

### Task 1: Create `atuin-bench-support` crate with in-memory Database

The `Database` trait (`atuin-server-database/src/lib.rs:114`) has ~20 methods. Only 8 are needed for record sync benchmarks. The rest return `DbError::NotFound` or empty results.

**Files:**
- Create: `crates/atuin-bench-support/Cargo.toml`
- Create: `crates/atuin-bench-support/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Add crate to workspace**

In the root `Cargo.toml`, add `"crates/atuin-bench-support"` to the `[workspace] members` list.

**Step 2: Create `Cargo.toml`**

```toml
[package]
name = "atuin-bench-support"
edition = "2024"
description = "In-memory server backend for benchmarking atuin sync"
publish = false

rust-version = { workspace = true }
version = { workspace = true }

[dependencies]
atuin-server-database = { workspace = true }
atuin-server = { workspace = true }
atuin-client = { path = "../atuin-client", version = "18.16.1", features = ["sync"] }
atuin-common = { workspace = true }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["net", "sync", "time", "rt"] }
tracing = { workspace = true }
uuid = { workspace = true }
time = { workspace = true }
eyre = { workspace = true }
rand = { workspace = true }
```

**Step 3: Implement `InMemoryDb`**

Create `crates/atuin-bench-support/src/lib.rs` with:

```rust
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};
use atuin_server_database::models::{History, NewHistory, NewSession, NewUser, Session, User};
use atuin_server_database::{calendar::TimePeriod, DbError, DbResult, DbSettings, Database};
use time::{OffsetDateTime, UtcOffset};

#[derive(Default)]
struct Inner {
    users: Vec<User>,
    sessions: Vec<Session>,
    records: Vec<Record<EncryptedData>>,
    next_id: i64,
}

#[derive(Clone, Default)]
pub struct InMemoryDb {
    inner: Arc<RwLock<Inner>>,
}

#[async_trait]
impl Database for InMemoryDb {
    async fn new(_settings: &DbSettings) -> DbResult<Self> {
        Ok(Self::default())
    }

    async fn get_session(&self, token: &str) -> DbResult<Session> {
        let inner = self.inner.read().unwrap();
        inner
            .sessions
            .iter()
            .find(|s| s.token == token)
            .map(|s| Session {
                id: s.id,
                user_id: s.user_id,
                token: s.token.clone(),
            })
            .ok_or(DbError::NotFound)
    }

    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        let session = self.get_session(token).await?;
        let inner = self.inner.read().unwrap();
        inner
            .users
            .iter()
            .find(|u| u.id == session.user_id)
            .map(|u| User {
                id: u.id,
                username: u.username.clone(),
                email: u.email.clone(),
                password: u.password.clone(),
            })
            .ok_or(DbError::NotFound)
    }

    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        let mut inner = self.inner.write().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.sessions.push(Session {
            id,
            user_id: session.user_id,
            token: session.token.clone(),
        });
        Ok(())
    }

    async fn get_user(&self, username: &str) -> DbResult<User> {
        let inner = self.inner.read().unwrap();
        inner
            .users
            .iter()
            .find(|u| u.username == username)
            .map(|u| User {
                id: u.id,
                username: u.username.clone(),
                email: u.email.clone(),
                password: u.password.clone(),
            })
            .ok_or(DbError::NotFound)
    }

    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        let inner = self.inner.read().unwrap();
        inner
            .sessions
            .iter()
            .find(|s| s.user_id == u.id)
            .map(|s| Session {
                id: s.id,
                user_id: s.user_id,
                token: s.token.clone(),
            })
            .ok_or(DbError::NotFound)
    }

    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        let mut inner = self.inner.write().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.users.push(User {
            id,
            username: user.username.clone(),
            email: user.email.clone(),
            password: user.password.clone(),
        });
        Ok(id)
    }

    async fn update_user_password(&self, _u: &User) -> DbResult<()> {
        Err(DbError::NotFound)
    }

    async fn add_records(&self, _user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        let mut inner = self.inner.write().unwrap();
        inner.records.extend(records.iter().cloned());
        Ok(())
    }

    async fn next_records(
        &self,
        _user: &User,
        host: HostId,
        tag: String,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>> {
        let inner = self.inner.read().unwrap();
        let start = start.unwrap_or(0);
        let results: Vec<_> = inner
            .records
            .iter()
            .filter(|r| r.host == host && r.tag == tag && r.idx >= start)
            .take(count as usize)
            .cloned()
            .collect();
        Ok(results)
    }

    async fn status(&self, _user: &User) -> DbResult<RecordStatus> {
        let inner = self.inner.read().unwrap();
        let mut status = RecordStatus::new();
        for record in &inner.records {
            let current = status.hosts.entry(record.host).or_default();
            let tag_idx = current.entry(record.tag.clone()).or_insert(0);
            if record.idx >= *tag_idx {
                *tag_idx = record.idx + 1;
            }
        }
        Ok(status)
    }

    // --- Stubs for methods not needed by record sync ---

    async fn count_history(&self, _user: &User) -> DbResult<i64> {
        Ok(0)
    }
    async fn count_history_cached(&self, _user: &User) -> DbResult<i64> {
        Ok(0)
    }
    async fn delete_user(&self, _u: &User) -> DbResult<()> {
        Err(DbError::NotFound)
    }
    async fn delete_history(&self, _user: &User, _id: String) -> DbResult<()> {
        Err(DbError::NotFound)
    }
    async fn deleted_history(&self, _user: &User) -> DbResult<Vec<String>> {
        Ok(vec![])
    }
    async fn delete_store(&self, _user: &User) -> DbResult<()> {
        Ok(())
    }
    async fn count_history_range(
        &self,
        _user: &User,
        _range: std::ops::Range<OffsetDateTime>,
    ) -> DbResult<i64> {
        Ok(0)
    }
    async fn list_history(
        &self,
        _user: &User,
        _created_after: OffsetDateTime,
        _since: OffsetDateTime,
        _host: &str,
        _page_size: i64,
    ) -> DbResult<Vec<History>> {
        Ok(vec![])
    }
    async fn add_history(&self, _history: &[NewHistory]) -> DbResult<()> {
        Ok(())
    }
    async fn oldest_history(&self, _user: &User) -> DbResult<History> {
        Err(DbError::NotFound)
    }
}
```

**Step 4: Verify it compiles**

Run: `cargo check -p atuin-bench-support`
Expected: Clean compilation.

**Step 5: Commit**

```bash
git add crates/atuin-bench-support/ Cargo.toml
git commit -m "bench: add in-memory Database impl for sync benchmarks

Implements the atuin-server-database Database trait backed by
Arc<RwLock<HashMap>> storage. No Postgres needed — exercises real
HTTP round-trips through axum without external dependencies."
```

---

### Task 2: Add bench server helper to `atuin-bench-support`

Add a `pub async fn start_bench_server()` that starts an in-process axum server with `InMemoryDb`, registers a test user, and returns a `Client` ready for benchmarking.

**Files:**
- Modify: `crates/atuin-bench-support/src/lib.rs`

**Step 1: Add the helper function**

Append to `crates/atuin-bench-support/src/lib.rs`:

```rust
use atuin_client::api_client::{self, AuthToken, Client};
use atuin_server::{Settings as ServerSettings, launch_with_tcp_listener};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub struct BenchServer {
    pub addr: String,
    _shutdown: oneshot::Sender<()>,
    _handle: tokio::task::JoinHandle<()>,
}

impl BenchServer {
    pub async fn start() -> Self {
        let settings = ServerSettings {
            host: "127.0.0.1".to_owned(),
            port: 0,
            path: String::new(),
            sync_v1_enabled: false,
            open_registration: true,
            max_history_length: 8192,
            max_record_size: 1024 * 1024,
            page_size: 1100,
            register_webhook_url: None,
            register_webhook_username: String::new(),
            db_settings: DbSettings {
                db_uri: "memory".to_owned(),
                read_db_uri: None,
            },
            metrics: atuin_server::settings::Metrics::default(),
            fake_version: None,
        };

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());

        let handle = tokio::spawn(async move {
            launch_with_tcp_listener::<InMemoryDb>(
                settings,
                listener,
                shutdown_rx.unwrap_or_else(|_| ()),
            )
            .await
            .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        Self {
            addr,
            _shutdown: shutdown_tx,
            _handle: handle,
        }
    }

    pub async fn register_client(&self) -> Client<'_> {
        let username = uuid::Uuid::new_v4().to_string();
        let email = format!("{username}@bench.test");
        let password = "bench-password";

        let resp = api_client::register(&self.addr, &username, &email, password)
            .await
            .unwrap();

        Client::new(&self.addr, AuthToken::Token(resp.session), 5, 30).unwrap()
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p atuin-bench-support`
Expected: Clean compilation.

**Step 3: Commit**

```bash
git add crates/atuin-bench-support/src/lib.rs
git commit -m "bench: add BenchServer helper for in-process sync benchmarks

Starts an axum server on a random port with InMemoryDb, registers
a test user, and returns a ready-to-use Client. No external
services required."
```

---

### Task 3: Write the sync download benchmark

The benchmark measures what the page size change actually optimizes: the wall-clock cost of downloading N records through HTTP at different page sizes. With page_size=100, downloading 1000 records takes 10 HTTP round-trips. With page_size=1000, it takes 1.

**Files:**
- Create: `crates/atuin/benches/sync.rs`
- Modify: `crates/atuin/Cargo.toml` (add divan + bench target)

**Step 1: Add dev-dependencies and bench target to `crates/atuin/Cargo.toml`**

Add to `[dev-dependencies]`:
```toml
divan = "0.1.14"
atuin-bench-support = { path = "../atuin-bench-support" }
```

Add at end of file:
```toml
[[bench]]
name = "sync"
harness = false
```

**Step 2: Write the benchmark**

Create `crates/atuin/benches/sync.rs`:

```rust
use atuin_bench_support::BenchServer;
use atuin_common::record::{EncryptedData, HostId, Record};
use atuin_common::utils::uuid_v7;

fn main() {
    divan::main();
}

fn generate_records(n: usize) -> (HostId, String, Vec<Record<EncryptedData>>) {
    let host = HostId(uuid_v7());
    let tag = "bench".to_string();

    let records: Vec<_> = (0..n)
        .map(|i| {
            Record::builder()
                .host(host)
                .version("v0".to_string())
                .tag(tag.clone())
                .idx(i as u64)
                .data(EncryptedData {
                    data: format!("bench-data-{i}"),
                    content_encryption_key: "bench-key".to_string(),
                })
                .build()
        })
        .collect();

    (host, tag, records)
}

/// Downloads 1000 pre-uploaded records at varying page sizes.
///
/// This directly measures the HTTP round-trip overhead that the page size
/// constant controls. At page_size=100, 1000 records require 10 HTTP requests.
/// At page_size=1000, they require 1.
#[divan::bench(args = [100, 1000], min_time = 5, sample_count = 50)]
fn download_records(bencher: divan::Bencher, page_size: u64) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let server = rt.block_on(BenchServer::start());
    let client = rt.block_on(server.register_client());
    let (host, tag, records) = generate_records(1000);
    rt.block_on(client.post_records(&records)).unwrap();

    bencher.bench(|| {
        rt.block_on(async {
            let mut offset = 0u64;
            loop {
                let page = client
                    .next_records(host, tag.clone(), offset, page_size)
                    .await
                    .unwrap();
                if page.is_empty() {
                    break;
                }
                offset += page.len() as u64;
            }
        })
    });
}
```

**Step 3: Verify it compiles**

Run: `cargo bench -p atuin --bench sync -- --test`
Expected: Compiles and lists `download_records` with both args.

**Step 4: Run benchmark**

Run: `cargo bench -p atuin --bench sync`
Expected: Output showing `download_records/100` significantly slower than `download_records/1000` due to 10× more HTTP round-trips.

**Step 5: Commit**

```bash
git add crates/atuin/benches/sync.rs crates/atuin/Cargo.toml
git commit -m "bench: add sync download benchmark comparing page sizes

Exercises real HTTP round-trips through an in-process axum server
with InMemoryDb. Downloads 1000 records at page_size 100 vs 1000,
directly measuring the round-trip overhead the page size controls."
```
