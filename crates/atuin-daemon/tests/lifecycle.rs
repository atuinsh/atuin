//! Integration tests for the daemon server lifecycle.
//!
//! Each test spins up a real gRPC server on a temporary unix socket,
//! connects a client, and exercises the daemon RPCs.

#[cfg(unix)]
mod unix {
    use std::sync::Arc;
    use std::time::Duration;

    use atuin_client::database::Sqlite;
    use atuin_client::history::HistoryId;
    use atuin_client::history::store::HistoryStore;
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_client::settings::{Settings, init_meta_config_for_testing};
    use atuin_daemon::client::HistoryClient;
    use atuin_daemon::grpc::HistoryService;
    use atuin_daemon::grpc::history::pb::history_server::HistoryServer;
    use atuin_daemon::{Daemon, DaemonHandle, HistoryJournal, SearchComponent, SemanticComponent};
    use rstest::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    /// Spins up a daemon server on a temp socket and returns a connected client,
    /// the daemon handle (for shutdown), and the temp dir (must be held to keep paths alive).
    #[fixture]
    async fn daemon() -> (HistoryClient, DaemonHandle, TempDir) {
        let tmp = tempfile::tempdir().unwrap();

        let db_path = tmp.path().join("history.db");
        let record_path = tmp.path().join("records.db");
        let key_path = tmp.path().join("key");
        let socket_path = tmp.path().join("test.sock");
        let meta_path = tmp.path().join("meta.db");

        // Initialize the meta store config for testing (required for Settings::host_id())
        init_meta_config_for_testing(meta_path.to_str().unwrap(), 5.0);

        // Build settings with test paths
        let settings: Settings = Settings::builder()
            .expect("could not build settings builder")
            .set_override("db_path", db_path.to_str().unwrap())
            .expect("failed to set db_path")
            .set_override("record_store_path", record_path.to_str().unwrap())
            .expect("failed to set record_store_path")
            .set_override("key_path", key_path.to_str().unwrap())
            .expect("failed to set key_path")
            .set_override("daemon.socket_path", socket_path.to_str().unwrap())
            .expect("failed to set socket_path")
            .set_override("meta.db_path", meta_path.to_str().unwrap())
            .expect("failed to set meta.db_path")
            .build()
            .expect("could not build settings")
            .try_deserialize()
            .expect("could not deserialize settings");

        // Create databases
        let history_db = Sqlite::new(&db_path, Duration::from_secs(5)).await.unwrap();
        let store = SqliteStore::new(&record_path, Duration::from_secs(5)).await.unwrap();

        // Dependencies the command registry needs (Arc-backed, shared with the components).
        let semantic_component = SemanticComponent::new();
        let search_index = SearchComponent::new().index();

        // Build and start the daemon
        let mut daemon =
            Daemon::builder(settings).store(store).history_db(history_db).build().unwrap();

        let handle = daemon.handle();

        // Build the command registry and the History gRPC service that drives it
        // (mirrors atuin_daemon::boot).
        let host_id = Settings::host_id().await.unwrap();
        let history_store =
            HistoryStore::new(handle.store().clone(), host_id, handle.encryption_key().clone());
        let journal = Arc::new(HistoryJournal::new(
            handle.caps().clone(),
            history_store,
            handle.history_db().clone(),
            semantic_component,
            search_index,
        ));
        let history_service = HistoryServer::new(HistoryService::new(journal, handle.clone()));

        // Start components (none registered, but keeps the lifecycle identical to production).
        daemon.start_components().await.unwrap();

        // Start the gRPC server
        let uds = UnixListener::bind(&socket_path).unwrap();
        let stream = UnixListenerStream::new(uds);

        let server_handle = handle.clone();
        tokio::spawn(async move {
            let mut rx = server_handle.subscribe();
            Server::builder()
                .add_service(history_service)
                .serve_with_incoming_shutdown(stream, async move {
                    loop {
                        match rx.recv().await {
                            Ok(atuin_daemon::DaemonEvent::ShutdownRequested) => break,
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                })
                .await
                .unwrap();
        });

        // Spawn the daemon event loop in the background
        tokio::spawn(async move {
            daemon.run_event_loop().await.unwrap();
        });

        // Give the server a moment to bind.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = HistoryClient::new(socket_path.clone()).await.unwrap();

        (client, handle, tmp)
    }

    #[rstest]
    #[tokio::test]
    async fn test_status(#[future] daemon: (HistoryClient, DaemonHandle, TempDir)) {
        let (mut client, _handle, _tmp) = daemon.await;

        let status = client.status().await.unwrap();
        assert!(status.healthy);
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.protocol, 2);
        assert!(status.pid > 0);
    }

    #[rstest]
    #[tokio::test]
    async fn test_start_end_history(#[future] daemon: (HistoryClient, DaemonHandle, TempDir)) {
        use atuin_client::history::History;

        let (mut client, _handle, _tmp) = daemon.await;

        let history = History::daemon()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("echo hello".to_string())
            .cwd("/tmp".to_string())
            .session("test-session".to_string())
            .cmd_origin(
                #[allow(deprecated)]
                atuin_domain::record::CmdOrigin::parse_lenient("test-host"),
            )
            .build()
            .into();

        let start_reply = client.start_history(history).await.unwrap();
        assert!(start_reply.id.is_some());

        let id: HistoryId = start_reply.id.unwrap().try_into().unwrap();
        let end_reply =
            client.end_history(id, Some(Duration::from_nanos(1_000_000)), 0).await.unwrap();
        assert!(end_reply.record_id.is_some());
    }

    #[rstest]
    #[tokio::test]
    async fn end_history_without_duration_derives_from_start(
        #[future] daemon: (HistoryClient, DaemonHandle, TempDir),
    ) {
        use atuin_client::history::History;

        let (mut client, _handle, _tmp) = daemon.await;

        let history: History = History::daemon()
            .timestamp(time::OffsetDateTime::now_utc() - Duration::from_millis(20))
            .command("sleep 0".to_string())
            .cwd("/tmp".to_string())
            .session("no-duration-session".to_string())
            .cmd_origin(
                atuin_domain::record::CmdOrigin::try_from("test-host:ellie".to_string()).unwrap(),
            )
            .build()
            .into();

        let start_reply = client.start_history(history).await.unwrap();
        let id: HistoryId = start_reply.id.unwrap().try_into().unwrap();

        // Omitting the duration makes the daemon derive it from the command's start timestamp.
        // Every other lifecycle case passes an explicit duration, leaving this path uncovered. The
        // timeout guards against a regression that hangs while reading the start time and finishing
        // the same entry.
        let end_reply =
            tokio::time::timeout(Duration::from_secs(5), client.end_history(id, None, 0))
                .await
                .expect("end_history deadlocked on the in-flight borrow")
                .unwrap();
        assert!(end_reply.record_id.is_some());
    }

    #[rstest]
    #[tokio::test]
    async fn test_tail_history_streams_started_and_ended_events(
        #[future] daemon: (HistoryClient, DaemonHandle, TempDir),
    ) {
        use atuin_client::history::History;
        use atuin_daemon::grpc::history::pb::tail_history_reply::Event;

        let (mut client, _handle, _tmp) = daemon.await;
        let mut stream = client.tail_history().await.unwrap();

        let history = History::daemon()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("git status".to_string())
            .cwd("/tmp/repo".to_string())
            .session("tail-session".to_string())
            .cmd_origin(atuin_domain::record::CmdOrigin::try_from("test-host:ellie").unwrap())
            .author("claude".to_string())
            .intent("inspect repository state".to_string())
            .shell("bash")
            .build()
            .into();

        let start_reply = client.start_history(history).await.unwrap();

        let started = stream.message().await.unwrap().unwrap();
        let started_history = match started.event {
            Some(Event::Started(history)) => history,
            other => panic!("expected a Started event, got {other:?}"),
        };
        assert_eq!(started_history.id, start_reply.id);
        assert_eq!(started_history.command, "git status");
        assert_eq!(started_history.cwd, "/tmp/repo");
        assert_eq!(started_history.hostname, "test-host:ellie");
        assert_eq!(started_history.author, "claude");
        assert_eq!(started_history.intent, "inspect repository state");

        let end_id: HistoryId = start_reply.id.clone().unwrap().try_into().unwrap();
        client.end_history(end_id, Some(Duration::from_nanos(1_000_000)), 0).await.unwrap();

        let ended = stream.message().await.unwrap().unwrap();
        let ended_history = match ended.event {
            Some(Event::Ended(history)) => history,
            other => panic!("expected an Ended event, got {other:?}"),
        };
        assert_eq!(ended_history.id, start_reply.id);
        assert_eq!(ended_history.exit, 0);
        assert_eq!(ended_history.duration, 1_000_000);
    }

    #[rstest]
    #[tokio::test]
    async fn test_end_unknown_history_fails(
        #[future] daemon: (HistoryClient, DaemonHandle, TempDir),
    ) {
        let (mut client, _handle, _tmp) = daemon.await;

        let result = client
            .end_history(HistoryId::from_bytes([0u8; 16]), Some(Duration::from_nanos(1000)), 0)
            .await;
        assert!(result.is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn test_shutdown(#[future] daemon: (HistoryClient, DaemonHandle, TempDir)) {
        let (mut client, _handle, _tmp) = daemon.await;

        let accepted = client.shutdown().await.unwrap();
        assert!(accepted);

        // Give server time to shut down.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Subsequent calls should fail since the server is gone.
        let result = client.status().await;
        assert!(result.is_err());
    }
}
