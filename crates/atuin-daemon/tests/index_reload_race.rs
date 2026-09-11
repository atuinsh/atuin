//! The lost-update race between rows arriving from sync -- fed straight into the live search index
//! via `SearchIndex::add_histories` under a *read* guard, exactly as the sync worker does -- and an
//! index reload (a delete or a rebuild) that scans the db into a fresh index and swaps it in.
//!
//! `reload_search_index` reads the shell filter, drops the guard, scans the db newest-first with a
//! keyset cursor (`order by id desc`, then `id < last_id`), and finally takes the write guard to
//! swap the new index in. Two windows swallow a synced row:
//!   * it is added to the *old* index, which the swap then discards; and
//!   * its id is the largest in the db (uuid_v7 is time-ordered), so once the scan has read its
//!     first page the keyset cursor has already moved below it and it is never visited.
//!
//! The row is still committed to the history db, so the durable layer is fine -- only the live
//! index loses it.
//!
//! Tests that document this still-unfixed defect (report M2, #4052) are `#[ignore]`d with the
//! defect named; their assertions state the *correct* behavior, so they fail under
//! `cargo nextest run --run-ignored all` (or `cargo test -- --ignored`) rather than passing
//! vacuously. These attack angles are distinct from `concurrency.rs`, which covers the single-batch
//! delete-vs-add case; here the adds are continuous across the whole reload, cover the rebuild
//! trigger too, and separate the durable db layer from the live index.
#![cfg(unix)]

mod common;

use std::time::Duration;

use atuin_client::history::History;
use atuin_client::settings::Search;
use common::TestEnv;
use common::corpus::{HistoryGen, index_eligible};
use rstest::*;

/// Rows large enough that a full index reload takes hundreds of milliseconds in a debug build, so
/// the continuously-added synced rows genuinely overlap the scan. Mirrors `concurrency.rs`.
const RELOAD_ROWS: usize = 40_000;

#[derive(Debug, Clone, Copy)]
enum Reload {
    Delete,
    Rebuild,
}

/// Drive a reload of the given kind to completion on a spawned task.
fn spawn_reload(env: &TestEnv, reload: Reload) -> tokio::task::JoinHandle<()> {
    let journal = env.journal.clone();
    let victim = env.seeded.ids[0];
    tokio::spawn(async move {
        match reload {
            Reload::Delete => {
                journal.delete(&[victim], &Search::default()).await.unwrap();
            }
            Reload::Rebuild => journal.rebuild(&Search::default()).await.unwrap(),
        }
    })
}

/// History that streamed in from sync *throughout* an index reload -- not one batch at one instant,
/// but a batch every scheduler tick for the whole reload -- is all searchable once the reload
/// settles. This is the invariant a user relies on the morning after enabling sync on a second
/// machine: nothing that downloaded while a delete or a rebuild happened to be running silently
/// vanishes from search.
///
/// EXPECTED TO FAIL: every batch is added to whichever index is live under a read guard; the swap
/// discards the old one, and the fresh index's keyset scan never revisits rows saved after its
/// first page (their ids are the largest in the db). Both the delete and the rebuild trigger the
/// same reload path.
#[rstest]
#[case::delete(Reload::Delete)]
#[case::rebuild(Reload::Rebuild)]
#[ignore = "documents an unfixed defect (synced adds dropped by a racing reload; see report M2, \
            #4052); run with --run-ignored. Continuous add_histories under a read guard is lost by \
            the index swap and the keyset scan. See module docs."]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn synced_batches_across_an_index_reload_stay_searchable(#[case] reload: Reload) {
    let env = TestEnv::builder().seed_rows(RELOAD_ROWS).with_search_component().build().await;
    let reload_task = spawn_reload(&env, reload);

    // Feed the index the way the sync worker does: save the batch to the db, then hand it straight
    // to the live index under a read guard. Keep going for the whole reload.
    let mut history_gen = HistoryGen::new(0xF00D);
    let mut synced_unique: Vec<History> = Vec::new();
    let mut batches = 0usize;
    while !reload_task.is_finished() && synced_unique.len() < 300 {
        let batch: Vec<History> =
            std::iter::repeat_with(|| history_gen.next()).filter(index_eligible).take(8).collect();
        env.history_db.save_bulk(&batch).await.unwrap();
        env.index.read().await.add_histories(&batch);
        synced_unique.extend(batch.into_iter().filter(HistoryGen::is_unique));
        batches += 1;
        tokio::task::yield_now().await;
    }
    reload_task.await.unwrap();
    assert!(batches >= 2, "reload finished before any batch overlapped it; raise RELOAD_ROWS");
    assert!(!synced_unique.is_empty(), "corpus produced no unique synced commands");

    // Assert on unique ("job-*") commands only: a synced *common* command could look searchable via
    // an older invocation even if this one was dropped, which would let the test pass vacuously.
    let mut missing = Vec::new();
    for h in &synced_unique {
        if !env.index_hits(&h.command).await.contains(&h.id) {
            missing.push(h.command.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "index lost {} of {} commands synced during the reload: {missing:?}",
        missing.len(),
        synced_unique.len()
    );
    assert_eq!(
        env.index_count().await,
        env.expected_command_count().await,
        "index size after synced adds + reload"
    );
}

/// A single row that arrives from sync mid-reload lands durably in the history db *and* stays
/// searchable. Splitting the two layers is the point: the durable write is never the casualty --
/// the user's data is safe on disk -- but the live index silently loses it, so `atuin search`
/// can't find a command that is provably present. A guard assertion on the db layer keeps the test
/// honest (it fails loudly rather than vacuously if the row was never persisted).
///
/// EXPECTED TO FAIL on the index assertion: the row is added to the old index (discarded by the
/// swap) and, saved after the scan's first page, sits past the keyset cursor forever.
#[rstest]
#[ignore = "documents an unfixed defect (synced row durable in the db but dropped from the index; \
            see report M2, #4052); run with --run-ignored. See module docs."]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_synced_row_survives_in_the_db_but_the_reload_drops_it_from_the_index() {
    let env = TestEnv::builder().seed_rows(RELOAD_ROWS).with_search_component().build().await;
    let reload_task = spawn_reload(&env, Reload::Delete);
    // Let the reload's paged scan get past its first page before the row lands, so the row's id
    // (the largest in the db) sits above the descending keyset cursor and is never revisited.
    tokio::time::sleep(Duration::from_millis(30)).await;

    let synced = std::iter::repeat_with({
        let mut history_gen = HistoryGen::new(0xBEEF);
        move || history_gen.next()
    })
    .find(|h| index_eligible(h) && HistoryGen::is_unique(h))
    .expect("corpus yields a unique index-eligible row");
    env.history_db.save_bulk([&synced]).await.unwrap();
    env.index.read().await.add_histories(std::slice::from_ref(&synced));

    reload_task.await.unwrap();

    // Durable layer -- a guard, not the defect. If this ever fails the premise (a durably-synced
    // row) is broken and the index assertion below would be meaningless.
    assert!(
        env.history_db.load(synced.id).await.unwrap().is_some(),
        "the synced row must be durably persisted in the history db"
    );
    // Live index -- the user-facing invariant this test defends.
    assert!(
        env.index_hits(&synced.command).await.contains(&synced.id),
        "synced row is durably in the db but was dropped from the search index: {}",
        synced.command
    );
}
