//! `atuin import` hands shell history to the daemon, which adds it to the history db, the record
//! store and the search index.
#![cfg(unix)]

mod common;

use std::time::Duration;

use atuin_client::history::{History, HistoryId};
use atuin_common::utils::uuid_v7;
use common::{TestEnv, history_at};
use rstest::*;

#[fixture]
async fn env() -> TestEnv {
    TestEnv::builder().build().await
}

/// Finished entries as an importer produces them, a second apart so none collide in the db.
fn imported(commands: impl IntoIterator<Item = String>) -> Vec<History> {
    let start = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    (0..)
        .zip(commands)
        .map(|(i, cmd)| History {
            duration: 1_000,
            ..history_at(&cmd, start + time::Duration::seconds(i))
        })
        .collect()
}

/// The same entries under the fresh ids an importer mints on every run.
fn reminted(histories: &[History]) -> Vec<History> {
    histories
        .iter()
        .map(|h| History {
            id: HistoryId::from(uuid_v7()),
            ..h.clone()
        })
        .collect()
}

fn three() -> Vec<History> {
    imported((0..3).map(|i| format!("echo imported {i}")))
}

/// Imported entries reach every place a recorded command does: db, store and search index.
#[rstest]
#[tokio::test]
async fn import_lands_in_db_store_and_search(#[future(awt)] env: TestEnv) {
    let histories = three();
    let mut client = env.history_client().await;

    let reply = client.import_history(histories.clone()).await.unwrap();

    assert_eq!(reply.imported, 3);
    assert_eq!(reply.protocol, 6);
    for h in &histories {
        assert_eq!(env.history_db.load(h.id).await.unwrap().as_ref(), Some(h));
    }
    assert_eq!(env.history_records().await.len(), 3);
    assert_eq!(env.index_count().await, 3);
}

/// Re-running an import (fresh ids, same entries) adds only what's new: no duplicate rows,
/// records to sync, or search results.
#[rstest]
#[tokio::test]
async fn reimport_only_adds_new_entries(#[future(awt)] env: TestEnv) {
    let histories = three();
    let mut client = env.history_client().await;
    client.import_history(histories.clone()).await.unwrap();

    let mut again = reminted(&histories);
    again.extend(imported(["echo brand new".to_owned()]));
    let reply = client.import_history(again).await.unwrap();

    assert_eq!(reply.imported, 1);
    assert_eq!(env.active_rows().await, 4);
    assert_eq!(env.history_records().await.len(), 4);
    assert_eq!(env.index_count().await, 4);
}

/// 100 x 64 KiB is past the daemon's 4 MiB message limit, were it sent as one message.
#[rstest]
#[tokio::test]
async fn import_of_long_commands_is_not_cut_short(#[future(awt)] env: TestEnv) {
    let long = imported((0..100).map(|i| format!("echo {i} {}", "x".repeat(64 * 1024))));
    let mut client = env.history_client().await;

    assert_eq!(client.import_history(long).await.unwrap().imported, 100);
    assert_eq!(env.active_rows().await, 100);
}

/// The daemon imports a long stream a bounded batch at a time; every batch lands.
#[rstest]
#[tokio::test]
async fn import_of_many_entries_spans_batches(#[future(awt)] env: TestEnv) {
    let many = imported((0..2500).map(|i| format!("echo many {i}")));
    let mut client = env.history_client().await;

    assert_eq!(client.import_history(many).await.unwrap().imported, 2500);
    assert_eq!(env.active_rows().await, 2500);
    assert_eq!(env.index_count().await, 2500);
}

/// A failed store write must not leave db rows behind, or a retry would skip them as imported.
#[rstest]
#[tokio::test]
async fn failed_import_leaves_nothing_for_a_retry_to_skip() {
    let env = TestEnv::builder().db_timeout(Duration::from_millis(200)).build().await;
    let histories = three();

    let lock = env.lock_record_store().await;
    env.journal.import(histories.clone()).await.unwrap_err();
    assert_eq!(env.active_rows().await, 0);
    lock.release().await;

    assert_eq!(env.journal.import(reminted(&histories)).await.unwrap(), 3);
    assert_eq!(env.history_records().await.len(), 3);
    assert_eq!(env.index_count().await, 3);
}
