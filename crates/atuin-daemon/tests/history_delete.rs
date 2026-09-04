//! What a user gets from `atuin history prune/dedup`, `atuin search --delete` and the TUI delete
//! key now that the daemon owns deletion: exact row removal, a search index that forgets
//! immediately, tombstones that reach other machines, and rows that survive a failed attempt.
#![cfg(unix)]

mod common;

use std::collections::HashSet;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_client::history::store::HistoryRecord;
use atuin_client::settings::Search;
use atuin_daemon::grpc::history::pb::tail_history_reply::Event;
use atuin_daemon::search::SearchIndex;
use atuin_daemon::{CmdDeleteError, CmdFinishError};
use common::{TestEnv, history};
use rstest::*;

#[fixture]
async fn env() -> TestEnv {
    TestEnv::builder().build().await
}

/// Which ids a delete batch names, relative to what the daemon knows.
#[derive(Debug, Clone, Copy)]
enum Target {
    Persisted,
    InFlight,
    Unknown,
    /// The same persisted id twice in one batch.
    PersistedTwice,
}

/// `deleted` counts every id the daemon processed, in-flight ones are cancelled instead of
/// tombstoned, and unknown ids get a tombstone anyway (idempotent from the user's view).
#[rstest]
#[case::persisted(&[Target::Persisted], 1, 1, 0)]
#[case::in_flight(&[Target::InFlight], 1, 0, 1)]
#[case::unknown(&[Target::Unknown], 1, 1, 0)]
#[case::duplicate_in_batch(&[Target::PersistedTwice], 2, 2, 0)]
#[case::mixed(&[Target::Persisted, Target::InFlight, Target::Unknown], 3, 2, 1)]
#[tokio::test]
async fn delete_reports_every_id_it_processed(
    #[future(awt)] env: TestEnv,
    #[case] targets: &[Target],
    #[case] expected_deleted: u64,
    #[case] expected_tombstones: usize,
    #[case] expected_cancelled: usize,
) {
    let mut client = env.history_client().await;
    let mut tail = client.tail_history().await.unwrap();

    let mut ids = Vec::new();
    let mut persisted = Vec::new();
    for target in targets {
        match target {
            Target::Persisted => {
                let id = env.record(&mut client, "echo persisted").await;
                persisted.push(id);
                ids.push(id);
            }
            Target::PersistedTwice => {
                let id = env.record(&mut client, "echo twice").await;
                persisted.push(id);
                ids.extend([id, id]);
            }
            Target::InFlight => {
                let reply = client.start_history(history("sleep 999")).await.unwrap();
                ids.push(reply.id.unwrap().try_into().unwrap());
            }
            Target::Unknown => ids.push(HistoryId::from_bytes([0xAB; 16])),
        }
    }
    // Drain the Started/Ended events the setup produced.
    let setup_events = targets
        .iter()
        .map(|t| match t {
            Target::Persisted | Target::PersistedTwice => 2,
            Target::InFlight => 1,
            Target::Unknown => 0,
        })
        .sum::<usize>();
    for _ in 0..setup_events {
        tail.message().await.unwrap().unwrap();
    }

    let reply = client.delete_history(ids.clone()).await.unwrap();

    assert_eq!(reply.deleted, expected_deleted);
    assert_eq!(reply.protocol, 2);
    assert_eq!(env.active_rows().await, 0, "no named row may survive");
    for id in &persisted {
        assert!(env.history_db.load(*id).await.unwrap().is_none());
    }
    let tombstones = env
        .history_records()
        .await
        .into_iter()
        .filter(|r| matches!(r, HistoryRecord::Delete(_)))
        .count();
    assert_eq!(tombstones, expected_tombstones);

    for _ in 0..expected_cancelled {
        let event = tokio::time::timeout(Duration::from_secs(2), tail.message())
            .await
            .expect("cancelled event for an in-flight delete")
            .unwrap()
            .unwrap();
        assert!(matches!(event.event, Some(Event::Cancelled(_))), "{event:?}");
    }
    // No extra `Cancelled` event beyond the expected count -- e.g. a delete double-cancelling an
    // in-flight id would show up here instead of just happening to match the round count above.
    let extra = tokio::time::timeout(Duration::from_millis(200), tail.message()).await;
    assert!(
        extra.is_err(),
        "unexpected extra tail event after {expected_cancelled} cancellations: {extra:?}"
    );
    for id in &ids {
        assert!(env.journal.get(*id).is_err(), "{id} must no longer be in flight");
    }
}

/// An empty batch is a no-op: nothing written, and the index is not rebuilt (a sentinel command
/// that exists only in the index survives).
#[rstest]
#[tokio::test]
async fn empty_delete_does_nothing(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    env.record(&mut client, "echo real").await;
    env.index.read().await.add_history(&history("index-only sentinel"));
    assert_eq!(env.index_count().await, 2);

    let reply = client.delete_history(vec![]).await.unwrap();

    assert_eq!(reply.deleted, 0);
    assert_eq!(env.index_count().await, 2, "no rebuild may run for an empty batch");
    assert!(env.history_records().await.iter().all(|r| matches!(r, HistoryRecord::Create(_))));
}

/// Deleting the newest invocation of a command makes search point at the next newest one, and
/// deleting the last invocation makes the command disappear.
#[rstest]
#[tokio::test]
async fn search_follows_the_surviving_invocations(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let mut search = env.search_client().await;
    let now = time::OffsetDateTime::now_utc();
    let first =
        env.record_at(&mut client, "cargo build --release", now - Duration::from_secs(3)).await;
    let second =
        env.record_at(&mut client, "cargo build --release", now - Duration::from_secs(2)).await;
    let third =
        env.record_at(&mut client, "cargo build --release", now - Duration::from_secs(1)).await;
    assert_eq!(env.rpc_hits(&mut search, "cargo build").await, vec![third]);

    client.delete_history(vec![third]).await.unwrap();
    assert_eq!(env.rpc_hits(&mut search, "cargo build").await, vec![second]);

    client.delete_history(vec![second, first]).await.unwrap();
    assert!(env.rpc_hits(&mut search, "cargo build").await.is_empty());
    assert_eq!(env.index_count().await, 0);
}

/// Every delete appends exactly one tombstone per id, contiguously after the creates, and a
/// second machine replaying the store ends up with the same rows as this one.
#[rstest]
#[case::one_at_a_time(false)]
#[case::one_batch(true)]
#[tokio::test]
async fn tombstones_are_contiguous_and_replay_identically(
    #[future(awt)] env: TestEnv,
    #[case] batched: bool,
) {
    let mut client = env.history_client().await;
    let ids: Vec<HistoryId> = {
        let mut v = Vec::new();
        for i in 0..5 {
            v.push(env.record(&mut client, &format!("echo {i}")).await);
        }
        v
    };
    let doomed = [ids[0], ids[2], ids[4]];

    if batched {
        client.delete_history(doomed.to_vec()).await.unwrap();
    } else {
        for id in doomed {
            client.delete_history(vec![id]).await.unwrap();
        }
    }

    assert_eq!(env.record_idxs().await, (0..8).collect::<Vec<u64>>());
    let deletes: Vec<HistoryId> = env
        .history_records()
        .await
        .into_iter()
        .filter_map(|r| match r {
            HistoryRecord::Delete(id) => Some(id),
            HistoryRecord::Create(_) => None,
        })
        .collect();
    assert_eq!(deletes, doomed.to_vec());

    let survivors: HashSet<HistoryId> = [ids[1], ids[3]].into();
    assert_eq!(env.active_ids().await, survivors);
    let other_machine = env.fresh_db_from_store().await;
    let mut pager = other_machine.all_paged(100, false, false);
    let mut replayed = HashSet::new();
    while let Some(page) = pager.next().await.unwrap() {
        replayed.extend(page.into_iter().map(|h| h.id));
    }
    assert_eq!(replayed, survivors, "another machine must see the same survivors");
}

/// A deletion is durable across an index reset and rebuild: resetting the in-memory search index
/// and reloading it from the same history db and record store neither resurfaces the deleted row
/// nor lets it come back. This does not simulate a true daemon process restart (the journal, the
/// db handle and the record store all stay the same Rust objects across the call) -- only the
/// in-memory index is torn down and rebuilt.
#[rstest]
#[tokio::test]
async fn deletion_survives_restart_and_rebuild(#[future(awt)] env: TestEnv) {
    let mut client = env.history_client().await;
    let doomed = env.record(&mut client, "echo doomed").await;
    let kept = env.record(&mut client, "echo kept").await;
    client.delete_history(vec![doomed]).await.unwrap();

    // Reset the in-memory index and rebuild it from the same db and store -- not a real process
    // restart (no new journal, db, or store connections), but it does exercise the codepath a
    // restart's fresh `rebuild()` call would take.
    *env.index.write().await = SearchIndex::default();
    env.journal.rebuild(&Search::default()).await.unwrap();

    assert_eq!(env.active_ids().await, HashSet::from([kept]));
    assert!(env.index_hits("doomed").await.is_empty());
    assert_eq!(env.index_hits("kept").await, vec![kept]);
}

/// When the record store cannot be written, the delete fails, no row is touched, the index is
/// untouched, and a retry after the store recovers completes the deletion.
#[rstest]
#[tokio::test]
async fn delete_failure_keeps_rows_until_retry() {
    let env = TestEnv::builder().db_timeout(Duration::from_millis(200)).build().await;
    let mut client = env.history_client().await;
    let a = env.record(&mut client, "echo a").await;
    let b = env.record(&mut client, "echo b").await;

    let lock = env.lock_record_store().await;
    let err = env.journal.delete([a, b], &Search::default()).await.unwrap_err();
    assert!(matches!(err, CmdDeleteError::HistoryStoreFailed(_)), "{err}");
    assert_eq!(env.active_ids().await, HashSet::from([a, b]));
    assert_eq!(env.index_count().await, 2);
    lock.release().await;

    assert_eq!(env.journal.delete([a, b], &Search::default()).await.unwrap(), 2);
    assert!(env.active_ids().await.is_empty());
    assert_eq!(env.index_count().await, 0);
}

/// A command whose persistence fails stays in flight (the lease rolls it back), so the shell's
/// retry, or a later delete, still finds it.
#[rstest]
#[case::history_db_down(true)]
#[case::record_store_down(false)]
#[tokio::test]
async fn failed_finish_keeps_the_command_in_flight(#[case] lock_history_db: bool) {
    let env = TestEnv::builder().db_timeout(Duration::from_millis(200)).build().await;
    let id = env.journal.start_cmd(history("echo flaky"));

    let lock = if lock_history_db {
        env.lock_history_db().await
    } else {
        env.lock_record_store().await
    };
    let Err(err) = env.journal.finish(id, 0, Duration::from_millis(1)).await else {
        panic!("finish must fail while the write is locked")
    };
    match (lock_history_db, &err) {
        (true, CmdFinishError::HistoryDbFailed(_))
        | (false, CmdFinishError::HistoryStoreFailed(_)) => {}
        other => panic!("unexpected error class: {other:?}"),
    }
    assert!(env.journal.get(id).is_ok(), "the command must still be in flight");
    assert_eq!(env.index_count().await, 0, "nothing may be indexed before persistence succeeds");
    lock.release().await;

    env.journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
    assert!(env.journal.get(id).is_err());
    assert_eq!(env.active_ids().await, HashSet::from([id]));
    let creates = env
        .history_records()
        .await
        .iter()
        .filter(|r| matches!(r, HistoryRecord::Create(_)))
        .count();
    assert_eq!(creates, 1, "exactly one create record after the retry");
    assert_eq!(env.index_count().await, 1);
}
