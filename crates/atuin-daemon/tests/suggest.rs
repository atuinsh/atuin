//! Integration tests for the suggestion RPC that backs the pty-proxy popup.
//!
//! These run a real gRPC server over a temporary unix socket and talk to it
//! with the real client, because the failure this pins down only exists at
//! that seam: it is about what the *server* does while a client with a very
//! short deadline is waiting.

#[cfg(unix)]
mod unix {
    use std::time::Duration;

    use atuin_client::database::{Context, Database, Sqlite};
    use atuin_client::history::History;
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_client::settings::{Settings, init_meta_config_for_testing};
    use atuin_daemon::client::SearchClient;
    use atuin_daemon::components::SearchComponent;
    use atuin_daemon::{Daemon, DaemonHandle};
    use rstest::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    /// What the pty-proxy allows the daemon per keystroke. Mirrors
    /// `DAEMON_SUGGEST_TIMEOUT` in the client; the popup gives up after it.
    const CLIENT_DEADLINE: Duration = Duration::from_millis(100);

    fn history(command: &str, cwd: &str, shell: Option<&str>) -> History {
        let builder = History::import()
            .timestamp(time::OffsetDateTime::now_utc())
            .command(command)
            .cwd(cwd)
            .exit(0);
        // The typed builder changes type with each field set, so the two
        // cases cannot share a binding.
        match shell {
            Some(shell) => builder.shell(shell).build().into(),
            None => builder.build().into(),
        }
    }

    fn context(cwd: &str) -> Context {
        Context {
            session: String::new(),
            hostname: "host:user".to_string(),
            cwd: cwd.to_string(),
            host_id: String::new(),
            git_root: None,
        }
    }

    /// A daemon whose search index is populated from `history`, served over a
    /// temp socket. The index is built with no shell filter, the way a daemon
    /// autostarted outside a hooked shell builds it — `$ATUIN_SHELL` is not
    /// set in its environment, so `shells = auto` resolves to "everything".
    async fn daemon_with(entries: Vec<History>) -> (SearchClient, DaemonHandle, TempDir) {
        let tmp = tempfile::tempdir().unwrap();

        let db_path = tmp.path().join("history.db");
        let record_path = tmp.path().join("records.db");
        let key_path = tmp.path().join("key");
        let socket_path = tmp.path().join("test.sock");
        let meta_path = tmp.path().join("meta.db");

        init_meta_config_for_testing(meta_path.to_str().unwrap(), 5.0);

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

        let history_db = Sqlite::new(&db_path, 5.0).await.unwrap();
        history_db.save_bulk(&entries).await.unwrap();
        let store = SqliteStore::new(&record_path, 5.0).await.unwrap();

        let search_component = SearchComponent::new();
        let search_service = search_component.grpc_service();

        let mut daemon = Daemon::builder(settings)
            .store(store)
            .history_db(history_db)
            .component(search_component)
            .build()
            .await
            .unwrap();

        let handle = daemon.handle();
        daemon.start_components().await.unwrap();
        let search_service = search_service.build(handle.clone());

        let uds = UnixListener::bind(&socket_path).unwrap();
        let stream = UnixListenerStream::new(uds);
        let server_handle = handle.clone();
        tokio::spawn(async move {
            let mut rx = server_handle.subscribe();
            Server::builder()
                .add_service(search_service)
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
        tokio::spawn(async move {
            daemon.run_event_loop().await.unwrap();
        });

        // Let the server bind.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = SearchClient::new(socket_path).await.unwrap();
        (client, handle, tmp)
    }

    /// Wait for the daemon's startup index build to finish.
    ///
    /// That build is the one thing that legitimately competes with a
    /// keystroke, and only once per daemon start, so the deadline these
    /// tests hold the daemon to is the steady state that follows it.
    async fn wait_until_indexed(client: &mut SearchClient) {
        for _ in 0..100 {
            let suggestions = client
                .suggest("cargo", 8, context("/home/user/repo"), true)
                .await
                .expect("suggest rpc failed");
            if !suggestions.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("index never became ready");
    }

    /// Every keystroke has to be answered inside the popup's deadline on a
    /// history of real size — including when the index was built for a shell
    /// filter other than the caller's, which is the state a daemon started
    /// outside a hooked shell is always in. Rebuilding to match reads the
    /// whole database; doing that on the keystroke costs the popup its
    /// deadline, and abandoning it there costs the rebuild too, so the next
    /// keystroke starts over and no suggestion ever arrives.
    #[rstest]
    #[tokio::test]
    // A 20k-entry index searched by an unoptimized build misses the deadline
    // by a wide margin, and that says nothing about the shipped daemon.
    #[cfg_attr(debug_assertions, ignore = "latency only holds in a release build")]
    async fn answers_every_keystroke_within_the_popup_deadline() {
        // Enough history that a rebuild is real work, the way it is for
        // anyone who has used atuin for a while.
        let mut entries: Vec<History> = (0..20_000)
            .map(|i| {
                history(
                    &format!("cargo run --example bench-{i}"),
                    "/home/user/repo",
                    Some("zsh"),
                )
            })
            .collect();
        entries.push(history(
            "cargo build --release",
            "/home/user/repo",
            Some("zsh"),
        ));
        entries.push(history("cargo test", "/home/user/repo", Some("zsh")));
        // Older history, recorded before atuin tracked shells at all.
        entries.push(history("cargo clippy", "/home/user/repo", None));
        let (mut client, handle, _tmp) = daemon_with(entries).await;
        wait_until_indexed(&mut client).await;

        for attempt in 0..10 {
            let call = client.suggest("cargo", 8, context("/home/user/repo"), true);
            let suggestions = tokio::time::timeout(CLIENT_DEADLINE, call)
                .await
                .unwrap_or_else(|_| {
                    panic!("attempt {attempt}: daemon missed the {CLIENT_DEADLINE:?} deadline")
                })
                .expect("suggest rpc failed");

            assert!(
                !suggestions.is_empty(),
                "attempt {attempt}: no suggestions for a prefix every command shares"
            );
        }

        handle.shutdown();
    }

    /// The ranking the popup depends on, proven over the wire rather than
    /// against the index directly: commands run here lead.
    #[rstest]
    #[tokio::test]
    async fn ranks_the_current_directory_first_over_the_wire() {
        let entries = vec![
            history("cargo build --release", "/home/user/elsewhere", Some("zsh")),
            history("cargo build --release", "/home/user/elsewhere", Some("zsh")),
            history("cargo build --release", "/home/user/elsewhere", Some("zsh")),
            history("cargo clippy", "/home/user/repo", Some("zsh")),
        ];
        let (mut client, handle, _tmp) = daemon_with(entries).await;
        let suggestions = client
            .suggest("cargo", 8, context("/home/user/repo"), true)
            .await
            .expect("suggest rpc failed");
        let commands: Vec<String> = suggestions.into_iter().map(|s| s.command).collect();

        assert_eq!(
            commands.first().map(String::as_str),
            Some("cargo clippy"),
            "the command run here should lead, despite being run less: {commands:?}"
        );

        handle.shutdown();
    }
}
