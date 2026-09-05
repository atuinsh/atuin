#![cfg(unix)]
//! Shared harness for the daemon integration tests.
//!
//! [`TestEnv`] boots a real daemon (journal, History + Search gRPC services on a temp unix socket)
//! over fresh sqlite files, optionally seeded with a deterministic corpus, and exposes every layer
//! a test may want to assert on: the gRPC clients, the journal, the history db, the record store,
//! and the live search index.
#![allow(dead_code, reason = "each integration test crate uses a different subset of the harness")]

pub mod corpus;
pub mod scale;
pub mod strategies;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use atuin_client::database::{Context, Sqlite};
use atuin_client::history::store::{HistoryRecord, HistoryStore};
use atuin_client::history::{History, HistoryId};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::{FilterMode, Settings};
use atuin_common::db::sqlite::Sqlite as CommonSqlite;
use atuin_common::filter::OrFilter;
use atuin_common::utils::uuid_v7;
use atuin_daemon::client::{HistoryClient, SearchClient, SearchParams};
use atuin_daemon::grpc::HistoryService;
use atuin_daemon::grpc::history::pb;
use atuin_daemon::grpc::history::pb::history_server::HistoryServer;
use atuin_daemon::search::{IndexFilterMode, SearchIndex};
use atuin_daemon::{
    Daemon, DaemonEvent, DaemonHandle, HistoryJournal, OutputCapture, SearchComponent,
};
use atuin_domain::record::{CmdOrigin, HostId, RecordTag};
use corpus::{HistoryGen, Seeded};
use hyper_util::rt::TokioIo;
use tempfile::TempDir;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, oneshot};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Server, Uri};
use tower::service_fn;
use uuid::Uuid;

/// A tame history entry: UUID session, strict `host:user` origin, bash. Everything the index needs.
pub fn history(cmd: &str) -> History {
    history_at(cmd, time::OffsetDateTime::now_utc())
}

/// Like [`history`], started at `timestamp`. The index tracks "most recent invocation" with
/// one-second granularity and keeps the first on a tie, so tests about recency must space entries.
pub fn history_at(cmd: &str, timestamp: time::OffsetDateTime) -> History {
    History::daemon()
        .timestamp(timestamp)
        .command(cmd)
        .cwd("/tmp")
        .session(uuid_v7().as_simple().to_string())
        .cmd_origin(CmdOrigin::try_from("test-host:test-user").unwrap())
        .shell("bash")
        // Explicit, so a developer shell exporting `ATUIN_HISTORY_AUTHOR` (e.g. to
        // `claude-code`) can't turn every harness entry into an agent entry the index skips
        // (`History::new` falls back to that env var when unset).
        .author("test-user")
        .build()
        .into()
}

pub struct TestEnvBuilder {
    db_timeout: Duration,
    with_search_component: bool,
    seed_rows: usize,
    seed: u64,
}

impl TestEnvBuilder {
    /// sqlite busy timeout for both databases. Lower it for failure-injection tests so a held
    /// write lock turns into an error quickly.
    #[must_use]
    pub fn db_timeout(mut self, timeout: Duration) -> Self {
        self.db_timeout = timeout;
        self
    }

    /// Register the real `SearchComponent` (background index loader + `HistorySynced` handling)
    /// and run the daemon event loop, as production does. Without it the index is only touched by
    /// the journal and by the harness itself, which keeps assertions deterministic.
    #[must_use]
    pub fn with_search_component(mut self) -> Self {
        self.with_search_component = true;
        self
    }

    /// Seed the history db with `rows` corpus entries before the daemon starts, and load them into
    /// the search index (directly, or via the component's loader when registered).
    #[must_use]
    pub fn seed_rows(mut self, rows: usize) -> Self {
        self.seed_rows = rows;
        self
    }

    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub async fn build(self) -> TestEnv {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("history.db");
        let record_path = tmp.path().join("records.db");
        let key_path = tmp.path().join("key");
        let socket_path = tmp.path().join("test.sock");

        let settings: Settings = Settings::builder()
            .expect("could not build settings builder")
            .set_override("db_path", db_path.to_str().unwrap())
            .unwrap()
            .set_override("record_store_path", record_path.to_str().unwrap())
            .unwrap()
            .set_override("key_path", key_path.to_str().unwrap())
            .unwrap()
            .set_override("daemon.socket_path", socket_path.to_str().unwrap())
            .unwrap()
            // Unroutable on purpose: the capability warm-up must fail fast, not dial the internet.
            .set_override("sync_address", "http://127.0.0.1:1")
            .unwrap()
            .build()
            .expect("could not build settings")
            .try_deserialize()
            .expect("could not deserialize settings");

        let history_db = Sqlite::new(&db_path, self.db_timeout).await.unwrap();
        let store = SqliteStore::new(&record_path, self.db_timeout).await.unwrap();

        let output_capture = OutputCapture::open(tmp.path().join("capture")).unwrap();
        let search_component = SearchComponent::new();
        let index = search_component.index();
        let search_service = search_component.grpc_service();

        // Seed before components start so a registered loader sees the rows.
        let mut history_gen = HistoryGen::new(self.seed);
        let (seeded, expected_index) = if self.seed_rows > 0 {
            let direct_index = (!self.with_search_component).then_some(&index);
            let seeded = corpus::seed_history_db(
                &history_db,
                &mut history_gen,
                self.seed_rows,
                direct_index,
            )
            .await;
            let expected_index = corpus::distinct_indexable_commands(&history_db).await;
            (seeded, expected_index)
        } else {
            (Seeded::default(), 0)
        };

        let mut builder =
            Daemon::builder(settings.clone()).store(store.clone()).history_db(history_db.clone());
        if self.with_search_component {
            builder = builder.component(search_component);
        }
        let mut daemon = builder.build().unwrap();
        let handle = daemon.handle();

        let host_id = HostId(uuid_v7());
        let history_store =
            HistoryStore::new(store.clone(), host_id, handle.encryption_key().clone());
        let journal = Arc::new(HistoryJournal::new(
            handle.caps().clone(),
            history_store.clone(),
            history_db.clone(),
            index.clone(),
            output_capture,
        ));
        let history_service =
            HistoryServer::new(HistoryService::new(journal.clone(), handle.clone()));
        let search_service = search_service.build(handle.clone());

        daemon.start_components().await.unwrap();

        let uds = UnixListener::bind(&socket_path).unwrap();
        let incoming = UnixListenerStream::new(uds);
        let server_handle = handle.clone();
        tokio::spawn(async move {
            let mut rx = server_handle.subscribe();
            Server::builder()
                .add_service(history_service)
                .add_service(search_service)
                .serve_with_incoming_shutdown(incoming, async move {
                    loop {
                        match rx.recv().await {
                            Ok(DaemonEvent::ShutdownRequested) | Err(_) => break,
                            Ok(_) => {}
                        }
                    }
                })
                .await
                .unwrap();
        });
        tokio::spawn(async move {
            daemon.run_event_loop().await.unwrap();
        });

        let env = TestEnv {
            tmp,
            settings,
            history_db,
            store,
            history_store,
            host_id,
            index,
            journal,
            handle,
            socket_path,
            seeded,
        };
        if self.with_search_component && self.seed_rows > 0 {
            env.wait_for_index_count(expected_index).await;
        }
        env
    }
}

pub struct TestEnv {
    pub tmp: TempDir,
    pub settings: Settings,
    pub history_db: Sqlite,
    pub store: SqliteStore,
    pub history_store: HistoryStore,
    pub host_id: HostId,
    pub index: Arc<RwLock<SearchIndex>>,
    pub journal: Arc<HistoryJournal>,
    pub handle: DaemonHandle,
    pub socket_path: PathBuf,
    pub seeded: Seeded,
}

impl TestEnv {
    #[must_use]
    pub fn builder() -> TestEnvBuilder {
        TestEnvBuilder {
            db_timeout: Duration::from_secs(5),
            with_search_component: false,
            seed_rows: 0,
            seed: 0x5EED,
        }
    }

    /// A fresh connection, like every shell hook makes. Retries briefly in case the server task
    /// has not started accepting yet.
    pub async fn history_client(&self) -> HistoryClient {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match HistoryClient::new(self.socket_path.clone()).await {
                Ok(client) => return client,
                Err(e) if tokio::time::Instant::now() < deadline => {
                    tracing::debug!("daemon not accepting yet: {e}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => panic!("could not connect to test daemon: {e}"),
            }
        }
    }

    pub async fn search_client(&self) -> SearchClient {
        SearchClient::new(self.socket_path.clone()).await.unwrap()
    }

    /// The prost-generated client, for sending hand-built (possibly malformed) messages.
    pub async fn raw_history_client(&self) -> pb::history_client::HistoryClient<Channel> {
        let path = self.socket_path.clone();
        let channel =
            Endpoint::try_from("http://atuin_local_daemon:0")
                .unwrap()
                .connect_with_connector(service_fn(move |_: Uri| {
                    let path = path.clone();
                    async move {
                        Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(path).await?))
                    }
                }))
                .await
                .unwrap();
        pb::history_client::HistoryClient::new(channel)
    }

    /// Start and end `cmd` through the RPCs, the way a shell does.
    pub async fn record(&self, client: &mut HistoryClient, cmd: &str) -> HistoryId {
        self.record_at(client, cmd, time::OffsetDateTime::now_utc()).await
    }

    /// [`Self::record`] with an explicit start timestamp (see [`history_at`]).
    pub async fn record_at(
        &self,
        client: &mut HistoryClient,
        cmd: &str,
        timestamp: time::OffsetDateTime,
    ) -> HistoryId {
        let start = client.start_history(history_at(cmd, timestamp)).await.unwrap();
        let id: HistoryId = start.id.unwrap().try_into().unwrap();
        client.end_history(id, Some(Duration::from_millis(1)), 0).await.unwrap();
        id
    }

    pub async fn active_rows(&self) -> i64 {
        self.history_db.history_count(false).await.unwrap()
    }

    /// Rows in the history db with exactly this `(timestamp, cwd, command)`: the triple the
    /// table's unique index dedups on. Uses its own connection because [`Sqlite`] keeps its pool
    /// private.
    pub async fn rows_with_triple(&self, timestamp: i64, cwd: &str, command: &str) -> i64 {
        let db = CommonSqlite::builder(self.settings.db_path.as_os_str())
            .timeout(Duration::from_secs(5))
            .open()
            .await
            .expect("open a second connection to the history db");
        atuin_common::db::query_scalar(
            "select count(1) from history where timestamp = ?1 and cwd = ?2 and command = ?3",
        )
        .bind(timestamp)
        .bind(cwd)
        .bind(command)
        .fetch_one(db.pool())
        .await
        .unwrap()
    }

    pub async fn active_ids(&self) -> HashSet<HistoryId> {
        let mut ids = HashSet::new();
        let mut pager = self.history_db.all_paged(corpus::SEED_BATCH, false, false);
        while let Some(page) = pager.next().await.unwrap() {
            ids.extend(page.into_iter().map(|h| h.id));
        }
        ids
    }

    /// What the index's `command_count` must be once it is in sync with the history db.
    pub async fn expected_command_count(&self) -> usize {
        corpus::distinct_indexable_commands(&self.history_db).await
    }

    pub async fn index_count(&self) -> usize {
        self.index.read().await.command_count()
    }

    /// Ids the live index returns for `query` (global filter, generous limit).
    pub async fn index_hits(&self, query: &str) -> Vec<HistoryId> {
        self.index.read().await.search(query, &IndexFilterMode::Global, 200).collect()
    }

    /// Ids the Search RPC returns for `query`, i.e. what the TUI would show.
    pub async fn rpc_hits(&self, client: &mut SearchClient, query: &str) -> Vec<HistoryId> {
        let params = SearchParams {
            query: query.to_owned(),
            query_id: 1,
            filter_mode: FilterMode::Global,
            context: None::<Context>,
            shells: OrFilter::all(),
        };
        let mut stream = client.search(params).await.unwrap();
        let response = stream.message().await.unwrap().expect("search response");
        response
            .ids
            .iter()
            .map(|id| HistoryId::new(Uuid::from_bytes(id.as_slice().try_into().unwrap())))
            .collect()
    }

    /// Every decoded history record in the store, in idx order.
    pub async fn history_records(&self) -> Vec<HistoryRecord> {
        self.history_store.history().await.unwrap()
    }

    /// The store's `idx` values for this host's history series, ascending.
    pub async fn record_idxs(&self) -> Vec<u64> {
        let mut idxs: Vec<u64> = self
            .store
            .all_tagged(&RecordTag::History)
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.host.id == self.host_id)
            .map(|r| r.idx)
            .collect();
        idxs.sort_unstable();
        idxs
    }

    /// Replay the record store, in idx order, into a brand-new history db: what another machine
    /// syncing this store would end up with.
    pub async fn fresh_db_from_store(&self) -> Sqlite {
        let fresh = Sqlite::new(
            self.tmp.path().join(format!("replay-{}.db", uuid_v7())),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let mut records = self.store.all_tagged(&RecordTag::History).await.unwrap();
        // One host per test env, so idx alone is the sync order.
        records.sort_by_key(|r| r.idx);
        let ids: Vec<_> = records.iter().map(|r| r.id).collect();
        self.history_store.build_all(&fresh, &ids).await.unwrap();
        fresh
    }

    /// Wait (up to 30s) for the index to hold exactly `count` commands.
    pub async fn wait_for_index_count(&self, count: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let got = self.index_count().await;
            if got == count {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "index never reached {count} commands (last saw {got})"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Hold the history db's write lock so every write the daemon attempts fails with
    /// "database is locked" after the busy timeout.
    pub async fn lock_history_db(&self) -> WriteLock {
        WriteLock::hold(&self.settings.db_path, "delete from history where id = '__no_such_row__'")
            .await
    }

    /// Same for the record store.
    pub async fn lock_record_store(&self) -> WriteLock {
        WriteLock::hold(
            &self.settings.record_store_path,
            "delete from store where id = '__no_such_row__'",
        )
        .await
    }
}

/// A sqlite write lock held from a second connection until [`WriteLock::release`].
pub struct WriteLock {
    release: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl WriteLock {
    /// `no_op_write` must be a statement that writes nothing but still needs the writer lock
    /// (sqlx's `query` wants a `'static` string, hence a literal per database).
    async fn hold(path: impl AsRef<Path>, no_op_write: &'static str) -> Self {
        let path = path.as_ref().to_owned();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let (locked_tx, locked_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let db = CommonSqlite::builder(path.as_os_str())
                .timeout(Duration::from_secs(5))
                .open()
                .await
                .expect("open second connection for lock injection");
            let mut tx = db.pool().begin().await.unwrap();
            // A no-op write takes the WAL writer lock; holding the transaction keeps it.
            atuin_common::db::query(no_op_write).execute(&mut *tx).await.unwrap();
            let _ = locked_tx.send(());
            let _ = release_rx.await;
            tx.rollback().await.unwrap();
        });
        locked_rx.await.unwrap();
        Self {
            release: Some(release_tx),
            task,
        }
    }

    pub async fn release(mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let _ = (&mut self.task).await;
    }
}

/// A fresh current-thread runtime, for proptest bodies (which are synchronous) that need a daemon
/// per case rather than the one [`SharedEnv`] hands out.
#[must_use]
pub fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// One daemon shared by every proptest case in a test binary. proptest bodies are synchronous, so
/// this owns the runtime too.
pub struct SharedEnv {
    runtime: tokio::runtime::Runtime,
    env: TestEnv,
}

impl SharedEnv {
    pub fn get() -> &'static Self {
        static CELL: OnceLock<SharedEnv> = OnceLock::new();
        CELL.get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            let env = runtime.block_on(TestEnv::builder().build());
            Self { runtime, env }
        })
    }

    pub fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.runtime.block_on(f)
    }

    #[must_use]
    pub fn env(&self) -> &TestEnv {
        &self.env
    }
}
