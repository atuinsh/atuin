//! Integration tests for the daemon server lifecycle.
//!
//! Each test spins up a real gRPC server on a temporary unix socket,
//! connects a client, and exercises the daemon RPCs.

#[cfg(unix)]
mod unix {
    use std::time::Duration;

    use atuin_client::database::Sqlite;
    use atuin_client::history::store::HistoryStore;
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::record::HostId;
    use atuin_common::utils::uuid_v7;
    use atuin_daemon::client::HistoryClient;
    use atuin_daemon::history::history_server::HistoryServer;
    use atuin_daemon::server::HistoryService;
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio::sync::watch;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    /// Spins up a daemon server on a temp socket and returns a connected client,
    /// the shutdown sender, and the temp dir (must be held to keep paths alive).
    async fn start_test_daemon() -> (HistoryClient, watch::Sender<bool>, TempDir) {
        let tmp = tempfile::tempdir().unwrap();

        let db_path = tmp.path().join("history.db");
        let record_path = tmp.path().join("records.db");

        let history_db = Sqlite::new(&db_path, 5.0).await.unwrap();
        let store = SqliteStore::new(&record_path, 5.0).await.unwrap();

        let host_id = HostId(uuid_v7());
        let encryption_key = [0u8; 32];
        let history_store = HistoryStore::new(store, host_id, encryption_key);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let service = HistoryService::new(history_store, history_db, shutdown_tx.clone());

        let socket_path = tmp.path().join("test.sock");
        let uds = UnixListener::bind(&socket_path).unwrap();
        let stream = UnixListenerStream::new(uds);

        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            Server::builder()
                .add_service(HistoryServer::new(service))
                .serve_with_incoming_shutdown(stream, async move {
                    let _ = rx.changed().await;
                })
                .await
                .unwrap();
        });

        // Give the server a moment to bind.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = HistoryClient::new(socket_path.to_string_lossy().to_string())
            .await
            .unwrap();

        (client, shutdown_tx, tmp)
    }

    #[tokio::test]
    async fn test_status() {
        let (mut client, _shutdown, _tmp) = start_test_daemon().await;

        let status = client.status().await.unwrap();
        assert!(status.healthy);
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.protocol, 1);
        assert!(status.pid > 0);
    }

    #[tokio::test]
    async fn test_start_end_history() {
        use atuin_client::history::History;

        let (mut client, _shutdown, _tmp) = start_test_daemon().await;

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
        let (mut client, _shutdown, _tmp) = start_test_daemon().await;

        let result = client
            .end_history("nonexistent-id".to_string(), 1000, 0)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_shutdown() {
        let (mut client, _shutdown_tx, _tmp) = start_test_daemon().await;

        let accepted = client.shutdown().await.unwrap();
        assert!(accepted);

        // Give server time to shut down.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Subsequent calls should fail since the server is gone.
        let result = client.status().await;
        assert!(result.is_err());
    }
}
