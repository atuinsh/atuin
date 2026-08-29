//! Integration tests for the daemon search service.
//!
//! Boots a real gRPC server on a temporary unix socket with the search
//! component wired up, seeds history spanning directories, hosts, sessions,
//! and shells, then verifies every filter mode and the shell-switch index
//! rebuild through the actual client.

#[cfg(unix)]
mod unix {
    use std::time::Duration;

    use atuin_client::database::{Context, Sqlite};
    use atuin_client::history::History;
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_client::settings::{FilterMode, Settings, init_meta_config_for_testing};
    use atuin_common::filter::OrFilter;
    use atuin_daemon::client::{SearchClient, SearchParams};
    use atuin_daemon::components::SearchComponent;
    use atuin_daemon::{Daemon, DaemonHandle};
    use tempfile::TempDir;
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;
    use uuid::Uuid;

    const SESSION_A: &str = "018f9db6-2222-7000-8000-000000000001";
    const SESSION_B: &str = "018f9db6-2222-7000-8000-000000000002";

    fn seed_history(
        command: &str,
        cwd: &str,
        hostname: &str,
        session: &str,
        shell: Option<&str>,
    ) -> History {
        let mut history: History = History::import()
            .timestamp(time::OffsetDateTime::now_utc())
            .command(command)
            .cwd(cwd)
            .cmd_origin(
                #[allow(deprecated)]
                atuin_domain::record::CmdOrigin::parse_lenient(hostname),
            )
            .session(session.to_string())
            .build()
            .into();
        history.shell = shell.map(str::to_owned);
        history
    }

    /// Boots a daemon whose history db already contains `seeded`, and returns
    /// a connected search client plus the seeded entries' ids.
    async fn start_seeded_daemon(seeded: &[History]) -> (SearchClient, DaemonHandle, TempDir) {
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

        let history_db = Sqlite::new(&db_path, Duration::from_secs(5)).await.unwrap();
        for history in seeded {
            history_db.save(history).await.unwrap();
        }
        let store = SqliteStore::new(&record_path, Duration::from_secs(5)).await.unwrap();

        let search_component = SearchComponent::new();
        let search_service = search_component.grpc_service();

        let mut daemon = Daemon::builder(settings)
            .store(store)
            .history_db(history_db)
            .component(search_component)
            .build()
            .unwrap();

        let handle = daemon.handle();
        daemon.start_components().await.unwrap();

        let uds = UnixListener::bind(&socket_path).unwrap();
        let stream = UnixListenerStream::new(uds);

        let server_handle = handle.clone();
        let search_service = search_service.build(handle.clone());
        tokio::spawn(async move {
            let mut rx = server_handle.subscribe();
            Server::builder()
                .add_service(search_service)
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

        tokio::spawn(async move {
            daemon.run_event_loop().await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = SearchClient::new(socket_path.clone()).await.unwrap();

        (client, handle, tmp)
    }

    fn context(cwd: &str, hostname: &str, session: &str, git_root: Option<&str>) -> Context {
        Context {
            session: session.to_string(),
            cwd: cwd.to_string(),
            #[allow(deprecated)]
            cmd_origin: atuin_domain::record::CmdOrigin::parse_lenient(hostname),
            host_id: "test-host-id".to_string(),
            git_root: git_root.map(Into::into),
        }
    }

    /// Runs one search and returns the matched history ids (hyphenated uuids).
    async fn search(
        client: &mut SearchClient,
        query_id: u64,
        query: &str,
        filter_mode: FilterMode,
        context: Context,
        shells: &[&str],
    ) -> Vec<String> {
        let params = SearchParams {
            query: query.to_string(),
            query_id,
            filter_mode,
            context: Some(context),
            shells: OrFilter::from_list(shells.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .unwrap_or_default(),
        };
        let mut stream = client.search(params).await.unwrap();
        let response = stream.message().await.unwrap().expect("expected response");
        assert_eq!(response.query_id, query_id);
        response
            .ids
            .iter()
            .map(|id| {
                let bytes: [u8; 16] = id.as_slice().try_into().unwrap();
                Uuid::from_bytes(bytes).to_string()
            })
            .collect()
    }

    fn ids_of(entries: &[&History]) -> Vec<String> {
        let mut ids: Vec<String> =
            entries.iter().map(|h| Uuid::parse_str(&h.id.0).unwrap().to_string()).collect();
        ids.sort();
        ids
    }

    fn sorted(mut ids: Vec<String>) -> Vec<String> {
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn filters_and_shell_switching_work_end_to_end() {
        // alpha: bash, host-a, session A, inside the workspace
        // beta:  zsh, host-b, session B, outside the workspace
        // gamma: unknown shell, host-a, session A, workspace root
        let alpha = seed_history("echo alpha", "/work/repo/sub", "host-a", SESSION_A, Some("bash"));
        let beta = seed_history("echo beta", "/elsewhere", "host-b", SESSION_B, Some("zsh"));
        let gamma = seed_history("echo gamma", "/work/repo", "host-a", SESSION_A, None);
        let seeded = [alpha.clone(), beta.clone(), gamma.clone()];

        let (mut client, handle, _tmp) = start_seeded_daemon(&seeded).await;
        let ctx = || context("/work/repo", "host-a", SESSION_A, None);

        // Wait for the background loader to finish indexing the seeded db.
        let mut query_id = 0;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            query_id += 1;
            let results =
                search(&mut client, query_id, "echo", FilterMode::Global, ctx(), &[]).await;
            if results.len() == 3 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "index never finished loading; got {results:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Directory: exact cwd match only.
        query_id += 1;
        let results =
            search(&mut client, query_id, "echo", FilterMode::Directory, ctx(), &[]).await;
        assert_eq!(sorted(results), ids_of(&[&gamma]), "directory filter");

        // Workspace: everything under the git root.
        query_id += 1;
        let results = search(
            &mut client,
            query_id,
            "echo",
            FilterMode::Workspace,
            context("/work/repo/sub", "host-a", SESSION_A, Some("/work/repo")),
            &[],
        )
        .await;
        assert_eq!(sorted(results), ids_of(&[&alpha, &gamma]), "workspace");

        // Host: only host-b's command.
        query_id += 1;
        let results = search(
            &mut client,
            query_id,
            "echo",
            FilterMode::Host,
            context("/work/repo", "host-b", SESSION_A, None),
            &[],
        )
        .await;
        assert_eq!(sorted(results), ids_of(&[&beta]), "host filter");

        // Session: both session-A commands.
        query_id += 1;
        let results = search(&mut client, query_id, "echo", FilterMode::Session, ctx(), &[]).await;
        assert_eq!(sorted(results), ids_of(&[&alpha, &gamma]), "session");

        // Unknown filter targets match nothing rather than erroring.
        query_id += 1;
        let results = search(
            &mut client,
            query_id,
            "echo",
            FilterMode::Host,
            context("/work/repo", "no-such-host", SESSION_A, None),
            &[],
        )
        .await;
        assert!(results.is_empty(), "unknown host should match nothing");

        // Changing shells rebuilds the index: bash only, then bash+unknown,
        // then zsh only, then back to all.
        for (shells, expected, label) in [
            (vec!["bash"], ids_of(&[&alpha]), "bash only"),
            (vec!["bash", ""], ids_of(&[&alpha, &gamma]), "bash+unknown"),
            (vec!["zsh"], ids_of(&[&beta]), "zsh only"),
            (vec![], ids_of(&[&alpha, &beta, &gamma]), "back to all"),
        ] {
            query_id += 1;
            let results =
                search(&mut client, query_id, "echo", FilterMode::Global, ctx(), &shells).await;
            assert_eq!(sorted(results), expected, "shell filter: {label}");
        }

        handle.shutdown();
    }
}
