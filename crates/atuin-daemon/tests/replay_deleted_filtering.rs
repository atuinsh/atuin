//! The record-store replay path that the sync worker uses to fold downloaded history into the
//! search index. `worker::index_downloaded_records` rebuilds a batch of records with
//! [`HistoryStore::incremental_build`], then re-reads the built ids through the db's
//! [`Sqlite::load_active`] before indexing them. That extra read matters: `incremental_build`'s
//! stream carries no `deleted_at is null` filter and still yields a create whose delete lands in the
//! same batch, so indexing its output directly would pollute the index; `load_active` drops the
//! deleted row so only live rows reach the index. These tests pin, black-box, that the reconstructed
//! *database* and *index* agree. They mirror `index_downloaded_records` rather than call it (the
//! sync worker is a private module), so a true end-to-end guard would need a `pub(crate)` seam. This
//! is deleted-row *filtering*, not a reload race -- no concurrency is involved.
#![cfg(unix)]

mod common;

use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::history::HistoryId;
use atuin_client::settings::Search;
use atuin_daemon::search::{IndexFilterMode, SearchIndex};
use atuin_domain::record::{RecordId, RecordTag};
use common::{TestEnv, history, history_at};
use futures::TryStreamExt;
use time::OffsetDateTime;

/// Record `cmd` end-to-end through the journal (start + finish), the way a shell does, and return
/// its id. The command lands in the history db and, as a `Create`, in the record store.
async fn record_then(env: &TestEnv, cmd: &str) -> HistoryId {
    let id = env.journal.start_cmd(history(cmd));
    env.journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
    id
}

/// Every history record id in the store, in idx (i.e. sync/replay) order. One host per env, so idx
/// alone is the order another machine would apply them in.
async fn record_ids_in_order(env: &TestEnv) -> Vec<RecordId> {
    let mut records = env.store.all_tagged(&RecordTag::History).await.unwrap();
    records.sort_by_key(|r| r.idx);
    records.into_iter().map(|r| r.id).collect()
}

/// Mirror the sync worker's `index_downloaded_records`: apply `ids` to a fresh db with
/// `incremental_build`, then re-read the built ids through `load_active` and index only the live
/// rows -- exactly as the worker does, so a row deleted elsewhere in the same batch never reaches
/// the index. Returns the reconstructed db, the index, and the ids the stream yielded (the raw
/// material *before* the `load_active` filter, so callers can pin that `incremental_build` yields
/// even a same-batch-deleted create).
async fn replay_into_fresh(
    env: &TestEnv,
    ids: &[RecordId],
) -> (Sqlite, SearchIndex, Vec<HistoryId>) {
    let db = Sqlite::in_memory(Duration::from_secs(5)).await.unwrap();
    let index = SearchIndex::default();
    let mut yielded = Vec::new();
    {
        let stream = env.history_store.incremental_build(&db, ids);
        let mut stream = std::pin::pin!(stream);
        while let Some(batch) = stream.try_next().await.unwrap() {
            yielded.extend(batch.iter().map(|h| h.id));
        }
    }
    let active = db.load_active(yielded.iter().copied()).await.unwrap();
    index.add_histories(&active);
    (db, index, yielded)
}

fn index_hits(index: &SearchIndex, query: &str) -> Vec<HistoryId> {
    index.search(query, &IndexFilterMode::Global, 200).collect()
}

/// The database a machine reconstructs by replaying the store never keeps a command that was
/// created and then deleted before it ever synced: `load_active` -- the filter the live index-load
/// path is built on -- omits it, while an un-deleted command survives. This is the correct half of
/// the divergence its index-feeding sibling below violates: `incremental_build` still *yields* the
/// doomed create to its caller regardless of the later delete, which is precisely what the worker
/// forwards, unfiltered, into the search index.
#[tokio::test]
async fn replay_reconstructs_the_db_without_a_same_batch_deleted_row() {
    let env = TestEnv::builder().build().await;
    let keeper = record_then(&env, "keeper-survives-replay").await;
    let doomed = record_then(&env, "doomed-created-and-deleted").await;
    assert_eq!(env.journal.delete(&[doomed], &Search::default()).await.unwrap(), 1);

    let ids = record_ids_in_order(&env).await;
    let (db, _index, yielded) = replay_into_fresh(&env, &ids).await;

    // The reconstructed db is correct: the doomed row is gone, the keeper remains.
    assert!(
        db.load_active([doomed]).await.unwrap().is_empty(),
        "a command deleted in the same replay batch must not be an active db row"
    );
    assert_eq!(
        db.load_active([keeper]).await.unwrap().len(),
        1,
        "the un-deleted command must survive replay"
    );
    // The source of the divergence: incremental_build hands the doomed create to its caller even
    // though the same batch deletes it -- the raw material the worker feeds into the index.
    assert!(
        yielded.contains(&doomed),
        "incremental_build yields the create even for a row deleted in the same batch"
    );
    assert!(yielded.contains(&keeper), "and it yields the surviving command's create too");
}

/// A command created and then deleted before it ever syncs down to another machine must not be
/// searchable there: a delete is supposed to erase the command everywhere. The worker rebuilds a
/// downloaded batch with `incremental_build` and re-reads it through `load_active` before indexing,
/// so a create whose delete lands in the same batch -- which `incremental_build` still yields -- is
/// dropped by the `deleted_at is null` filter and never pollutes the index.
#[tokio::test]
async fn a_command_deleted_in_the_same_replay_batch_is_not_searchable() {
    let env = TestEnv::builder().build().await;
    // A keeper so the replay -- and the index -- is not vacuously empty.
    let keeper = record_then(&env, "keeper-survives-replay").await;
    let doomed = record_then(&env, "doomed-deleted-before-first-sync").await;
    assert_eq!(env.journal.delete(&[doomed], &Search::default()).await.unwrap(), 1);

    let ids = record_ids_in_order(&env).await;
    let (db, index, _yielded) = replay_into_fresh(&env, &ids).await;

    // True today: the reconstructed db has no active row for the deleted command, and the keeper is
    // searchable -- so a failure below is real index pollution, not an empty or broken replay.
    assert!(db.load_active([doomed]).await.unwrap().is_empty());
    assert!(
        index_hits(&index, "keeper-survives-replay").contains(&keeper),
        "the surviving command must be searchable"
    );

    // The deleted command is gone from search, just as it is from the db.
    assert!(
        !index_hits(&index, "doomed-deleted-before-first-sync").contains(&doomed),
        "a command deleted in the same replay batch must not be searchable"
    );
}

/// When one command string was run twice -- one invocation still live, a newer one since deleted --
/// search must still surface the live invocation. The index keys a command by its text and tracks
/// only the *most recent* invocation's id. Indexing through `load_active` drops the deleted (newer)
/// invocation, so the live one stays the command's representative; without that filter the deleted
/// id would become the representative and shadow the live invocation -- pointing search at a command
/// the user deleted (one `load_active` cannot even load).
#[tokio::test]
async fn a_deleted_duplicate_does_not_hide_the_live_command_from_search() {
    let env = TestEnv::builder().build().await;
    let now = OffsetDateTime::now_utc();
    // Same command text, two invocations spaced past the index's 1s recency granularity so the
    // newest (the one we delete) is unambiguously the most recent.
    let live = env.journal.start_cmd(history_at("shared-dup-cmd", now - Duration::from_secs(2)));
    env.journal.finish(live, 0, Duration::from_millis(1)).await.unwrap();
    let doomed = env.journal.start_cmd(history_at("shared-dup-cmd", now));
    env.journal.finish(doomed, 0, Duration::from_millis(1)).await.unwrap();
    assert_eq!(env.journal.delete(&[doomed], &Search::default()).await.unwrap(), 1);

    let ids = record_ids_in_order(&env).await;
    let (db, index, _yielded) = replay_into_fresh(&env, &ids).await;

    // True today: only the live invocation is an active row for this command.
    let active: Vec<HistoryId> =
        db.load_active([live, doomed]).await.unwrap().into_iter().map(|h| h.id).collect();
    assert_eq!(active, vec![live], "only the live invocation is an active db row");

    // Search still finds the command, resolved to the LIVE invocation -- not the deleted, newer one.
    let hits = index_hits(&index, "shared-dup-cmd");
    assert!(!hits.is_empty(), "a command with a still-live invocation must remain searchable");
    assert!(
        hits.contains(&live) && !hits.contains(&doomed),
        "search must resolve the shared command to its live invocation, not the deleted one; got \
         {hits:?}"
    );
}
