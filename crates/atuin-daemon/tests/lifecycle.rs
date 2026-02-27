//! Integration tests for the daemon server lifecycle.
//!
//! Each test spins up a real gRPC server on a temporary unix socket,
//! connects a client, and exercises the daemon RPCs.

#[cfg(unix)]
mod unix {
    use std::time::Duration;

    use atuin_client::database::Sqlite;
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_client::settings::{Settings, init_meta_config_for_testing};
    use atuin_daemon::client::HistoryClient;
    use atuin_daemon::components::HistoryComponent;
    use atuin_daemon::{Daemon, DaemonHandle};
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    /// Spins up a daemon server on a temp socket and returns a connected client,
    /// the daemon handle (for shutdown), and the temp dir (must be held to keep paths alive).
    async fn start_test_daemon() -> (HistoryClient, DaemonHandle, TempDir) {
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
        let history_db = Sqlite::new(&db_path, 5.0).await.unwrap();
        let store = SqliteStore::new(&record_path, 5.0).await.unwrap();

        // Create the history component and get its gRPC service
        let history_component = HistoryComponent::new();
        let history_service = history_component.grpc_service();

        // Build and start the daemon
        let mut daemon = Daemon::builder(settings)
            .store(store)
            .history_db(history_db)
            .component(history_component)
            .build()
            .await
            .unwrap();

        let handle = daemon.handle();

        // Start components (this initializes the history component with the handle)
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
                            Ok(_) => continue,
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

        let client = HistoryClient::new(socket_path.to_string_lossy().to_string())
            .await
            .unwrap();

        (client, handle, tmp)
    }

    #[tokio::test]
    async fn test_status() {
        let (mut client, _handle, _tmp) = start_test_daemon().await;

        let status = client.status().await.unwrap();
        assert!(status.healthy);
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.protocol, 1);
        assert!(status.pid > 0);
    }

    #[tokio::test]
    async fn test_start_end_history() {
        use atuin_client::history::History;

        let (mut client, _handle, _tmp) = start_test_daemon().await;

        let history = History::daemon()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("echo hello".to_string())
            .cwd("/tmp".to_string())
            .session("test-session".to_string())
            .hostname("test-host".to_string())
            .build()
            .into();

        let start_reply = client.start_history(history).await.unwrap();
        assert!(!start_reply.id.is_empty());

        let end_reply = client
            .end_history(start_reply.id, 1_000_000, 0)
            .await
            .unwrap();
        assert!(!end_reply.id.is_empty());
    }

    #[tokio::test]
    async fn test_end_unknown_history_fails() {
        let (mut client, _handle, _tmp) = start_test_daemon().await;

        let result = client
            .end_history("nonexistent-id".to_string(), 1000, 0)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_shutdown() {
        let (mut client, _handle, _tmp) = start_test_daemon().await;

        let accepted = client.shutdown().await.unwrap();
        assert!(accepted);

        // Give server time to shut down.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Subsequent calls should fail since the server is gone.
        let result = client.status().await;
        assert!(result.is_err());
    }
}
