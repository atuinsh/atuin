//! `atuin store rebuild history` now asks the daemon to rebuild: the history db is re-derived from
//! the record store and the search index reloaded, without disturbing commands still running.
#![cfg(unix)]

mod common;

use std::collections::HashSet;
use std::time::Duration;

use atuin_client::history::store::HistoryRecord;
use atuin_client::history::{HistoryId, Version};
use atuin_client::settings::Search;
use atuin_common::encryption::paseto_v4;
use atuin_daemon::search::SearchIndex;
use atuin_domain::record::{Host, Record, RecordSeriesKey, RecordTag, RecordVersion};
use common::corpus::seed_record_store;
use common::{TestEnv, history};
use rstest::*;

#[fixture]
async fn env() -> TestEnv {
    TestEnv::builder().build().await
}

/// Rows lost from the db (but still in the store) come back; tombstoned rows stay gone; a row
/// that only ever lived in the db (pre-store history) is left alone.
#[rstest]
#[tokio::test]
async fn rebuild_rederives_rows_from_the_store(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let lost = env.record(&mut client, "echo lost-from-db").await;
    let deleted = env.record(&mut client, "echo tombstoned").await;
    client.delete_history(vec![deleted]).await.unwrap();
    let db_only = history("echo db-only");
    env.history_db.save(&db_only).await.unwrap();
    env.history_db.delete_rows([lost]).await.unwrap();
    *env.index.write().await = SearchIndex::default();

    let reply = client.rebuild_history().await.unwrap();
    assert_eq!(reply.protocol, 2);

    assert_eq!(env.active_ids().await, HashSet::from([lost, db_only.id]));
    assert_eq!(env.index_hits("lost-from-db").await, vec![lost]);
    assert_eq!(env.index_hits("db-only").await, vec![db_only.id]);
    assert!(env.index_hits("tombstoned").await.is_empty());
}

#[rstest]
#[tokio::test]
async fn rebuild_is_idempotent(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    for i in 0..10 {
        env.record(&mut client, &format!("echo {i}")).await;
    }
    let before_ids = env.active_ids().await;
    let before_index = env.index_count().await;

    for _ in 0..3 {
        client.rebuild_history().await.unwrap();
        assert_eq!(env.active_ids().await, before_ids);
        assert_eq!(env.index_count().await, before_index);
    }
    assert_eq!(env.record_idxs().await, (0..10).collect::<Vec<u64>>(), "rebuild writes no records");
}

/// A rebuild must not touch commands that are still running: they stay in flight and finish
/// normally afterwards.
#[rstest]
#[tokio::test]
async fn rebuild_leaves_in_flight_commands_alone(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let running: HistoryId =
        client.start_history(history("sleep 30")).await.unwrap().id.unwrap().try_into().unwrap();

    client.rebuild_history().await.unwrap();

    assert!(env.journal.get(running).is_ok());
    assert!(env.history_db.load(running).await.unwrap().is_none(), "not persisted yet");
    client.end_history(running, Some(Duration::from_secs(30)), 0).await.unwrap();
    assert_eq!(env.active_ids().await, HashSet::from([running]));
    assert_eq!(env.index_hits("sleep 30").await, vec![running]);
}

/// A record the daemon cannot decrypt (written under another key) or decode is skipped, and every
/// other record is still rebuilt.
#[rstest]
#[case::foreign_key(true)]
#[case::garbage_payload(false)]
#[tokio::test]
async fn rebuild_skips_undecodable_records(#[future(awt)] env: TestEnv, #[case] foreign_key: bool) {
    let good: Vec<_> = (0..3).map(|i| history(&format!("echo good {i}"))).collect();
    seed_record_store(&env.history_store, &good).await;

    let series = RecordSeriesKey::new(env.host_id, RecordTag::History);
    let idx = env.store.last(&series).await.unwrap().unwrap().idx + 1;
    let bad = if foreign_key {
        let bytes = HistoryRecord::Create(history("echo evil")).serialize().unwrap();
        Record::builder()
            .host(Host::new(env.host_id))
            .version(RecordVersion::from(Version::LATEST.name()))
            .tag(RecordTag::History)
            .idx(idx)
            .data(bytes)
            .build()
            .encrypt(&paseto_v4::Key::generate())
    } else {
        Record::builder()
            .host(Host::new(env.host_id))
            .version(RecordVersion::from(Version::LATEST.name()))
            .tag(RecordTag::History)
            .idx(idx)
            .data(atuin_domain::record::DecryptedData::from(vec![0xFF; 40]))
            .build()
            .encrypt(&env.history_store.encryption_key)
    };
    env.store.push(&bad).await.unwrap();

    env.journal.rebuild(&Search::default()).await.unwrap();

    let expected: HashSet<HistoryId> = good.iter().map(|h| h.id).collect();
    assert_eq!(env.active_ids().await, expected);
    assert_eq!(env.index_count().await, 3);
}
