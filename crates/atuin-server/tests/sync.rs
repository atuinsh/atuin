use std::env::temp_dir;
use std::time::Duration;

use atuin_client::api_client;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncEngine};
use atuin_common::encryption::paseto_v4;
use atuin_common::utils::uuid_v7;
use atuin_domain::record::{EncryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordTag};
use atuin_server::{Settings as ServerSettings, launch_with_tcp_listener};
use atuin_server_database::DbSettings;
use atuin_server_sqlite::Sqlite;
use futures_util::TryFutureExt;
use rstest::{fixture, rstest};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// A SQLite-backed server on a random port, shut down and cleaned up when it goes out of scope.
struct TestServer {
    address: url::Url,
    db: std::path::PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    handle: JoinHandle<()>,
}

impl TestServer {
    /// Register a fresh user, and return a client authenticated as them.
    async fn register(&self) -> api_client::Client {
        let username = uuid_v7().as_simple().to_string();
        let password = uuid_v7().as_simple().to_string();
        let email = format!("{}@example.com", uuid_v7().as_simple());

        let resp =
            api_client::register(&self.address, &username, &email, &password, &Default::default())
                .await
                .unwrap();

        api_client::Client::new(
            self.address.clone(),
            &api_client::AuthToken::Token(resp.session),
            5,
            30,
            &Default::default(),
            api_client::caps_client_anonymous(&self.address, &Default::default()).unwrap(),
        )
        .unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.handle.abort();

        // The pool may still hold these open, but unlinking an open file is fine.
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.db.clone().into_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(path);
        }
    }
}

#[fixture]
async fn server() -> TestServer {
    let db = temp_dir().join(format!("atuin-record-sync-{}.db", uuid_v7().as_simple()));

    let server_settings = ServerSettings {
        host: "127.0.0.1".to_owned(),
        port: 0,
        path: String::new(),
        open_registration: true,
        max_record_size: 1024 * 1024 * 1024,
        register_webhook_url: None,
        register_webhook_username: String::new(),
        db_settings: DbSettings {
            db_uri: format!("sqlite://{}", db.to_str().unwrap()),
            read_db_uri: None,
        },
        metrics: atuin_server::settings::Metrics::default(),
        fake_version: None,
    };

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        if let Err(e) = launch_with_tcp_listener::<Sqlite>(
            server_settings,
            listener,
            shutdown_rx.unwrap_or_else(|_| ()),
        )
        .await
        {
            panic!("error running server: {e:?}");
        }
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    TestServer {
        address: url::Url::parse(&format!("http://{addr}")).expect("valid test server url"),
        db,
        shutdown: Some(shutdown_tx),
        handle,
    }
}

fn key() -> paseto_v4::Key {
    // Arbitrary key; doesn't matter for these tests.
    paseto_v4::Key::from([7u8; 32])
}

fn record(host: HostId, tag: &RecordTag, idx: RecordIdx) -> Record<EncryptedData> {
    Record::builder()
        .host(Host::new(host))
        .version("v0".to_string().into())
        .tag(tag.clone())
        .idx(idx)
        .data(EncryptedData {
            raw: String::from("some data"),
            cek: String::from("some key"),
        })
        .build()
}

/// Populate the remote with records `0..=remote_max` and the local store with records
/// `0..=local_max` (or nothing if `local_max` is [`None`]), then run a real sync.
///
/// Returns the records the remote sent us, and the highest record index the local store ends up
/// with.
async fn download(
    server: &TestServer,
    remote_max: RecordIdx,
    local_max: Option<RecordIdx>,
    page_size: u64,
) -> (Vec<RecordId>, RecordIdx) {
    let client = server.register().await;

    let host = HostId(uuid_v7());
    let tag = RecordTag::Other(uuid_v7().as_simple().to_string());

    let records: Vec<Record<EncryptedData>> =
        (0..=remote_max).map(|idx| record(host, &tag, idx)).collect();

    client.post_records(&records).await.unwrap();

    let store = SqliteStore::new(":memory:", Duration::from_secs(2)).await.unwrap();
    if let Some(local_max) = local_max {
        store.push_batch(records.iter().take(local_max as usize + 1)).await.unwrap();
    }

    let key = key();
    let engine = SyncEngine::builder()
        .store(store.clone())
        .client_source(ClientSource::FromClient(client))
        .build()
        .connect()
        .await
        .unwrap()
        .with_page_size(std::num::NonZeroU64::new(page_size).unwrap());
    let (diff, _) = engine.diff().await.unwrap();
    let operations = SyncEngine::operations(diff).unwrap();
    let (_, downloaded) = engine.keyed(&key).sync_remote(operations).await.unwrap();

    let status = store.status().await.unwrap();
    let local_idx = *status.hosts.get(&host).unwrap().get(&tag).unwrap();

    (downloaded, local_idx)
}

#[rstest]
#[case::partial_local(5, Some(2), 100, 3)]
#[case::empty_local(5, None, 100, 6)]
#[case::exact_page(4, Some(0), 4, 4)]
#[case::empty_local_page_plus_one(4, None, 4, 5)]
#[case::empty_local_single_record(0, None, 100, 1)]
#[case::exact_pages(9, Some(1), 4, 8)]
#[case::partial_page(6, Some(0), 4, 6)]
#[tokio::test]
async fn download_fetches_exactly_the_missing_records(
    #[future(awt)] server: TestServer,
    #[case] remote_max: RecordIdx,
    #[case] local_max: Option<RecordIdx>,
    #[case] page_size: u64,
    // The expected number of records to be downloaded.
    #[case] expected: usize,
) {
    let (downloaded, local_idx) = download(&server, remote_max, local_max, page_size).await;

    assert_eq!(local_idx, remote_max, "local store is missing records");
    assert_eq!(downloaded.len(), expected, "downloaded {downloaded:?}");
}

/// Populate the local store with records `0..=local_max` and the remote with records
/// `0..=remote_max` (or nothing if `remote_max` is [`None`]), then run a real sync.
///
/// Returns the number of records we sent and highest record index the remote ends up with.
async fn upload(
    server: &TestServer,
    local_max: RecordIdx,
    remote_max: Option<RecordIdx>,
    page_size: u64,
) -> (u64, RecordIdx) {
    let client = server.register().await;

    let host = HostId(uuid_v7());
    let tag = RecordTag::Other(uuid_v7().as_simple().to_string());

    let records: Vec<Record<EncryptedData>> =
        (0..=local_max).map(|idx| record(host, &tag, idx)).collect();

    let store = SqliteStore::new(":memory:", Duration::from_secs(2)).await.unwrap();
    store.push_batch(records.iter()).await.unwrap();

    if let Some(remote_max) = remote_max {
        client.post_records(&records[..=remote_max as usize]).await.unwrap();
    }

    let key = key();
    let engine = SyncEngine::builder()
        .store(store)
        .client_source(ClientSource::FromClient(client))
        .build()
        .connect()
        .await
        .unwrap()
        .with_page_size(std::num::NonZeroU64::new(page_size).unwrap());
    let (diff, _) = engine.diff().await.unwrap();
    let operations = SyncEngine::operations(diff).unwrap();
    let (uploaded, _) = engine.keyed(&key).sync_remote(operations).await.unwrap();

    let status = engine.record_status().await.unwrap();
    let remote_idx = *status.hosts.get(&host).unwrap().get(&tag).unwrap();

    // The PR that added these tests also changed the type of `uploaded` from `i64` to `u64`; the
    // redundant `as u64` here is just to make it convenient to run these tests before the PR's
    // fixes, by temporarily reverting all the non-test files.
    #[allow(clippy::unnecessary_cast)]
    (uploaded as u64, remote_idx)
}

#[rstest]
#[case::partial_remote(5, Some(2), 100, 3)]
#[case::empty_remote(5, None, 100, 6)]
#[case::exact_page(4, Some(0), 4, 4)]
#[case::empty_remote_page_plus_one(4, None, 4, 5)]
#[case::empty_remote_single_record(0, None, 100, 1)]
#[case::exact_pages(9, Some(1), 4, 8)]
#[case::partial_page(6, Some(0), 4, 6)]
#[tokio::test]
async fn upload_sends_exactly_the_missing_records(
    #[future(awt)] server: TestServer,
    #[case] local_max: RecordIdx,
    #[case] remote_max: Option<RecordIdx>,
    #[case] page_size: u64,
    // The expected number of records to be uploaded.
    #[case] expected: u64,
) {
    let (uploaded, remote_idx) = upload(&server, local_max, remote_max, page_size).await;

    assert_eq!(remote_idx, local_max, "remote is missing records");
    assert_eq!(uploaded, expected);
}
