//! The search index is fed from two hostile sources: raw batches handed to
//! [`SearchIndex::add_history_at`], and records replayed out of the store through the chunks of
//! [`HistoryStore::incremental_build`]. The sync worker replays freshly downloaded records the same
//! way.
//! Both must survive degenerate and adversarial input -- agent entries, foreign shells,
//! unparsable sessions, empty batches, endless duplicates -- and an environmental fault (a
//! write-locked database) without panicking, corrupting the index, or letting a non-indexable
//! command leak into interactive search. Every test states the user-facing invariant it defends;
//! none reaches into the private sync worker, only the reachable feeding surface it is built on.
#![cfg(unix)]

mod common;

use std::borrow::Borrow;

use atuin_client::history::store::BuildChunk;
use atuin_client::history::{AuthorKind, History, HistoryId};
use atuin_common::filter::OrFilter;
use atuin_daemon::search::{IndexFilterMode, SearchIndex, SearchIndexSettings};
use atuin_domain::record::{RecordId, RecordTag};
use common::{TestEnv, history, history_at};
use futures::StreamExt;
use rstest::*;
use time::OffsetDateTime;

/// Add `histories` to `index`. Frecency is computed as of now.
fn add<H>(index: &mut SearchIndex, histories: H)
where
    H: IntoIterator<Item: Borrow<History>>,
{
    index.add_history_at(histories, OffsetDateTime::now_utc());
}

/// Ids the index returns for `query` under no filter and a generous limit -- what the TUI shows.
fn hits(index: &SearchIndex, query: &str) -> Vec<HistoryId> {
    index.search(query, &IndexFilterMode::Global, 200).unwrap().collect()
}

/// Replay the records of `ids` into the history db chunk by chunk, the way the sync worker does.
///
/// Returns one result per chunk. A create chunk's result holds the entries that it saved. A delete
/// chunk's result holds no entries. A failed chunk's result holds the error from its commit.
async fn replay(env: &TestEnv, ids: &[RecordId]) -> Vec<eyre::Result<Vec<History>>> {
    let mut results = Vec::new();
    let mut chunks = std::pin::pin!(env.history_store.incremental_build(&env.history_db, ids));
    while let Some(chunk) = chunks.next().await {
        results.push(match chunk {
            BuildChunk::Create(chunk) => chunk.commit().await.map(|()| chunk.histories().to_vec()),
            BuildChunk::Delete(chunk) => chunk.commit().await.map(|_| Vec::new()),
        });
    }
    results
}

/// Record store record ids for this host, in idx order: the input the worker replays.
async fn record_ids(env: &TestEnv) -> Vec<RecordId> {
    let mut records = env.store.all_tagged(&RecordTag::History).await.unwrap();
    records.retain(|r| r.host.id == env.host_id);
    records.sort_by_key(|r| r.idx);
    records.into_iter().map(|r| r.id).collect()
}

#[derive(Debug, Clone, Copy)]
enum Poison {
    /// An AI agent ran it; agent history must never surface in interactive search.
    Agent,
    /// Its shell is not in the configured set.
    ForeignShell,
    /// Its session is not a valid UUID, so the index has no key to store it under.
    UnparsableSession,
    /// A tombstone from an older version, which has `deleted_at` set. A sync can still download
    /// one.
    Tombstone,
}

fn poison(kind: Poison) -> History {
    let mut h = history("echo poison");
    match kind {
        Poison::Agent => h.author_kind = Some(AuthorKind::Agent),
        Poison::ForeignShell => h.shell = Some("fish".to_owned()),
        Poison::UnparsableSession => "not-a-uuid".clone_into(&mut h.session),
        Poison::Tombstone => h.deleted_at = Some(h.timestamp),
    }
    h
}

/// A non-indexable entry in a batch is silently dropped, but a valid entry sharing that batch is
/// still indexed and searchable: one poisoned command a shell (or a sync download) hands over never
/// takes its healthy neighbours down with it.
#[rstest]
#[case::agent(Poison::Agent)]
#[case::foreign_shell(Poison::ForeignShell)]
#[case::unparsable_session(Poison::UnparsableSession)]
#[case::tombstone(Poison::Tombstone)]
fn non_indexable_entries_are_skipped_while_a_valid_sibling_indexes(#[case] kind: Poison) {
    // Bash-only so the foreign-shell entry is filtered while the valid bash entry passes.
    let mut index = SearchIndex::new(SearchIndexSettings {
        shells: OrFilter::from_list(vec!["bash".to_owned()]).unwrap(),
        ..SearchIndexSettings::default()
    });
    let keep = history("echo keep");
    // The poison is added first: a "skip aborts the batch" bug would also drop `keep`.
    add(&mut index, &[poison(kind), keep.clone()]);

    assert_eq!(index.command_count(), 1, "only the valid command may be indexed");
    assert_eq!(hits(&index, "echo keep"), vec![keep.id], "valid command stays searchable");
    assert!(hits(&index, "echo poison").is_empty(), "{kind:?} entry must not surface in search");
}

/// An empty batch changes nothing. When the worker hands the index an empty download, nothing
/// happens. The index isn't reset, and nothing panics.
#[rstest]
fn an_empty_batch_leaves_the_index_untouched() {
    let mut index = SearchIndex::default();
    add(&mut index, &[history("echo one"), history("echo two")]);
    assert_eq!(index.command_count(), 2);

    add(&mut index, &[]);
    assert_eq!(index.command_count(), 2, "an empty batch must not disturb the index");
}

/// Replaying the same command many times -- a hook that fires twice, a re-sync of already-seen
/// records -- never inflates the unique-command count the search shows, and the index keeps
/// reporting the most-recent invocation.
#[rstest]
fn repeated_adds_never_inflate_the_unique_command_count() {
    let mut index = SearchIndex::default();
    let base = time::OffsetDateTime::now_utc();
    // Five invocations of one command, spaced by whole seconds (the index's recency granularity),
    // so the last is unambiguously most-recent.
    let dup: Vec<History> =
        (0..5i64).map(|i| history_at("echo dup", base + time::Duration::seconds(i))).collect();
    let other = history("echo other");
    let mut batch = dup.clone();
    batch.push(other.clone());

    add(&mut index, &batch);
    // Re-add the whole batch, as replaying already-downloaded records would.
    add(&mut index, &batch);

    assert_eq!(index.command_count(), 2, "duplicate invocations merge into one command each");
    let latest = dup.last().unwrap().id;
    assert_eq!(hits(&index, "echo dup"), vec![latest], "index reports the most-recent invocation");
    assert_eq!(hits(&index, "echo other"), vec![other.id]);
}

/// Replaying an empty record set builds nothing and touches neither the db nor the index.
#[rstest]
#[tokio::test]
async fn incremental_build_over_no_records_indexes_nothing() {
    let env = TestEnv::builder().build().await;
    let mut index = SearchIndex::default();
    let no_ids: Vec<RecordId> = Vec::new();

    let batches = replay(&env, &no_ids).await;
    assert!(batches.is_empty(), "an empty id set yields no batches");
    for batch in batches {
        add(&mut index, batch.unwrap());
    }

    assert_eq!(index.command_count(), 0, "index left empty");
    assert_eq!(env.active_rows().await, 0, "db left empty");
}

/// A replay whose db writes cannot commit (another process holds the write lock) surfaces an error
/// batch instead of panicking, and feeds nothing into the index -- no half-written command leaks
/// in. Once the lock is gone the same replay succeeds and reproduces exactly the active rows.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_locked_history_db_faults_the_replay_without_leaving_a_torn_index() {
    let env = TestEnv::builder().build().await;
    let mut client = env.history_client().await;
    let commands: Vec<String> = (0..5).map(|i| format!("echo replay {i}")).collect();
    let mut history_ids = Vec::new();
    for cmd in &commands {
        history_ids.push(env.record(&mut client, cmd).await);
    }
    let ids = record_ids(&env).await;

    // Mirror the worker: feed a fresh index only from the batches the replay actually applies.
    let mut fault_index = SearchIndex::default();
    let lock = env.lock_history_db().await;
    let mut saw_err = false;
    for batch in replay(&env, &ids).await {
        match batch {
            Ok(created) => add(&mut fault_index, &created),
            Err(_) => saw_err = true,
        }
    }
    lock.release().await;
    assert!(saw_err, "a write-locked db must surface an error batch, not stall or panic");
    assert_eq!(fault_index.command_count(), 0, "no command may be torn into the index under fault");

    // After release the replay reproduces the index that matches the db's active rows.
    let mut good_index = SearchIndex::default();
    for batch in replay(&env, &ids).await {
        add(&mut good_index, batch.expect("replay after the lock is released"));
    }
    let mut active = env.active_histories().await;
    active.retain(|h| history_ids.contains(&h.id));
    assert_eq!(active.len(), commands.len(), "all recorded rows are active");
    assert_eq!(good_index.command_count(), commands.len());
    for h in &active {
        assert!(
            hits(&good_index, &h.command).contains(&h.id),
            "reindexed after release: {}",
            h.command
        );
    }
}

/// A concurrent process holding the record store's write lock does not block or corrupt a
/// read-only replay: WAL readers run alongside a writer, so every record is still read, built, and
/// indexed.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_locked_record_store_does_not_block_a_read_only_replay() {
    let env = TestEnv::builder().build().await;
    let mut client = env.history_client().await;
    let commands: Vec<String> = (0..5).map(|i| format!("echo store {i}")).collect();
    let mut history_ids = Vec::new();
    for cmd in &commands {
        history_ids.push(env.record(&mut client, cmd).await);
    }
    let ids = record_ids(&env).await;

    let mut index = SearchIndex::default();
    let lock = env.lock_record_store().await;
    for batch in replay(&env, &ids).await {
        add(&mut index, batch.expect("a store writer must not block a read-only replay"));
    }
    lock.release().await;

    let mut active = env.active_histories().await;
    active.retain(|h| history_ids.contains(&h.id));
    assert_eq!(index.command_count(), commands.len());
    for h in &active {
        assert!(
            hits(&index, &h.command).contains(&h.id),
            "indexed despite the store lock: {}",
            h.command
        );
    }
}
