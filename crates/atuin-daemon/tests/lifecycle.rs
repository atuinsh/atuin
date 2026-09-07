//! Integration tests for the daemon server lifecycle: every RPC round-trips through a real gRPC
//! server on a temporary unix socket.
#![cfg(unix)]

mod common;

use std::time::Duration;

use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Search;
use atuin_daemon::grpc::history::pb::tail_history_reply::Event;
use atuin_daemon::search::IndexFilterMode;
use common::{TestEnv, history};
use rstest::*;

#[fixture]
async fn env() -> TestEnv {
    TestEnv::builder().build().await
}

#[rstest]
#[tokio::test]
async fn test_status(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let status = client.status().await.unwrap();
    assert!(status.healthy);
    assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(status.protocol, 2);
    assert!(status.pid > 0);
}

#[rstest]
#[tokio::test]
async fn test_start_end_history(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
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
    let end_reply = client.end_history(id, Some(Duration::from_nanos(1_000_000)), 0).await.unwrap();
    assert!(end_reply.record_id.is_some());
}

#[rstest]
#[tokio::test]
async fn end_history_without_duration_derives_from_start(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let history: History = History::daemon()
        .timestamp(time::OffsetDateTime::now_utc() - Duration::from_millis(20))
        .command("sleep 0".to_string())
        .cwd("/tmp".to_string())
        .session("no-duration-session".to_string())
        .cmd_origin(atuin_domain::record::CmdOrigin::try_from("test-host:ellie").unwrap())
        .build()
        .into();

    let start_reply = client.start_history(history).await.unwrap();
    let id: HistoryId = start_reply.id.unwrap().try_into().unwrap();

    // The timeout guards against a regression that hangs while reading the start time and
    // finishing the same entry (the old self-deadlock).
    let end_reply = tokio::time::timeout(Duration::from_secs(5), client.end_history(id, None, 0))
        .await
        .expect("end_history deadlocked on the in-flight borrow")
        .unwrap();
    assert!(end_reply.record_id.is_some());
    let row = env.history_db.load(id).await.unwrap().expect("row saved");
    assert!(row.duration >= 20_000_000, "derived duration too small: {}", row.duration);
}

#[rstest]
#[tokio::test]
async fn test_tail_history_streams_started_and_ended_events(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
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
async fn test_end_unknown_history_fails(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let result = client
        .end_history(HistoryId::from_bytes([0u8; 16]), Some(Duration::from_nanos(1000)), 0)
        .await;
    assert!(result.is_err());
}

#[rstest]
#[tokio::test]
async fn test_shutdown(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    assert!(client.shutdown().await.unwrap());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(client.status().await.is_err());
}

#[rstest]
#[tokio::test]
async fn test_delete_history_removes_entry(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let id = env.record(&mut client, "echo delete-me").await;
    assert_eq!(env.active_rows().await, 1);

    let reply = client.delete_history(vec![id]).await.unwrap();
    assert_eq!(reply.deleted, 1);
    assert_eq!(reply.protocol, 2);
    assert_eq!(env.active_rows().await, 0);

    // Deleting an already-deleted id still succeeds (idempotent), counting the record write.
    let reply = client.delete_history(vec![id]).await.unwrap();
    assert_eq!(reply.deleted, 1);
}

#[rstest]
#[tokio::test]
async fn test_rebuild_history(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    env.record(&mut client, "echo before-rebuild").await;

    let reply = client.rebuild_history().await.unwrap();
    assert_eq!(reply.protocol, 2);

    // The journal keeps working after a rebuild.
    let id = env.record(&mut client, "echo after-rebuild").await;
    assert_eq!(client.delete_history(vec![id]).await.unwrap().deleted, 1);
}

#[rstest]
#[tokio::test]
async fn journal_delete_removes_entry_and_rebuilds_index(#[future(awt)] env: TestEnv) {
    let journal = &env.journal;
    let id_a = journal.start_cmd(history("delete_me"));
    journal.finish(id_a, 0, Duration::from_millis(1)).await.unwrap();
    let id_b = journal.start_cmd(history("keep_me"));
    journal.finish(id_b, 0, Duration::from_millis(1)).await.unwrap();
    assert_eq!(env.index_count().await, 2);

    assert_eq!(journal.delete([id_a], &Search::default()).await.unwrap(), 1);

    let index = env.index.read().await;
    assert_eq!(index.command_count(), 1, "index should be rebuilt without the deleted command");
    assert_eq!(index.search("delete_me", &IndexFilterMode::Global, 10).count(), 0);
    assert_eq!(index.search("keep_me", &IndexFilterMode::Global, 10).count(), 1);
}

#[rstest]
#[tokio::test]
async fn journal_rebuild_reloads_index(#[future(awt)] env: TestEnv) {
    let journal = &env.journal;
    let id_a = journal.start_cmd(history("first_cmd"));
    journal.finish(id_a, 0, Duration::from_millis(1)).await.unwrap();
    let id_b = journal.start_cmd(history("second_cmd"));
    journal.finish(id_b, 0, Duration::from_millis(1)).await.unwrap();
    assert_eq!(env.index_count().await, 2);

    // Wipe both the history db and the index; only the record store still holds the commands.
    env.history_db.delete_rows([id_a, id_b]).await.unwrap();
    assert_eq!(env.active_rows().await, 0);
    *env.index.write().await = atuin_daemon::search::SearchIndex::default();
    assert_eq!(env.index_count().await, 0);

    journal.rebuild(&Search::default()).await.unwrap();

    assert_eq!(env.active_rows().await, 2);
    let index = env.index.read().await;
    assert_eq!(index.command_count(), 2, "rebuild should repopulate the index from the store");
    assert_eq!(index.search("first_cmd", &IndexFilterMode::Global, 10).count(), 1);
    assert_eq!(index.search("second_cmd", &IndexFilterMode::Global, 10).count(), 1);
}
