//! The search index is fed from two hostile sources: raw batches handed to
//! [`SearchIndex::add_histories`], and records replayed out of the store via
//! [`HistoryStore::incremental_build`] (what the sync worker does with freshly downloaded records).
//! Both must survive degenerate and adversarial input -- agent entries, foreign shells,
//! unparseable sessions, empty batches, endless duplicates -- and an environmental fault (a
//! write-locked database) without panicking, corrupting the index, or letting a non-indexable
//! command leak into interactive search. Every test states the user-facing invariant it defends;
//! none reaches into the private sync worker, only the reachable feeding surface it is built on.
#![cfg(unix)]

mod common;

use atuin_client::history::{AuthorKind, History, HistoryId};
use atuin_common::filter::OrFilter;
use atuin_daemon::search::{IndexFilterMode, SearchIndex};
use atuin_domain::record::{RecordId, RecordTag};
use common::{TestEnv, history, history_at};
use futures::StreamExt;
use rstest::*;

/// Ids the index returns for `query` under no filter and a generous limit -- what the TUI shows.
fn hits(index: &SearchIndex, query: &str) -> Vec<HistoryId> {
    index.search(query, &IndexFilterMode::Global, 200).collect()
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
    UnparseableSession,
}

fn poison(kind: Poison) -> History {
    let mut h = history("echo poison");
    match kind {
        Poison::Agent => h.author_kind = Some(AuthorKind::Agent),
        Poison::ForeignShell => h.shell = Some("fish".to_owned()),
        Poison::UnparseableSession => "not-a-uuid".clone_into(&mut h.session),
    }
    h
}

/// A non-indexable entry in a batch is silently dropped, but a valid entry sharing that batch is
/// still indexed and searchable: one poisoned command a shell (or a sync download) hands over never
/// takes its healthy neighbours down with it.
#[rstest]
#[case::agent(Poison::Agent)]
#[case::foreign_shell(Poison::ForeignShell)]
#[case::unparseable_session(Poison::UnparseableSession)]
fn non_indexable_entries_are_skipped_while_a_valid_sibling_indexes(#[case] kind: Poison) {
    // Bash-only so the foreign-shell entry is filtered while the valid bash entry passes.
    let index = SearchIndex::new(OrFilter::from_list(vec!["bash".to_owned()]).unwrap());
    let keep = history("echo keep");
    // The poison is added first: a "skip aborts the batch" bug would also drop `keep`.
    index.add_histories(&[poison(kind), keep.clone()]);

    assert_eq!(index.command_count(), 1, "only the valid command may be indexed");
    assert_eq!(hits(&index, "echo keep"), vec![keep.id], "valid command stays searchable");
    assert!(hits(&index, "echo poison").is_empty(), "{kind:?} entry must not surface in search");
}

/// An empty batch changes nothing: the worker handing `add_histories` an empty download is a no-op,
/// not a reset or a panic.
#[test]
fn an_empty_batch_leaves_the_index_untouched() {
    let index = SearchIndex::default();
    index.add_histories(&[history("echo one"), history("echo two")]);
    assert_eq!(index.command_count(), 2);

    index.add_histories(&[]);
    assert_eq!(index.command_count(), 2, "add_histories(&[]) must not disturb the index");
}

/// Replaying the same command many times -- a hook that fires twice, a re-sync of already-seen
/// records -- never inflates the unique-command count the search shows, and the index keeps
/// reporting the most-recent invocation.
#[test]
fn repeated_adds_never_inflate_the_unique_command_count() {
    let index = SearchIndex::default();
    let base = time::OffsetDateTime::now_utc();
    // Five invocations of one command, spaced by whole seconds (the index's recency granularity),
    // so the last is unambiguously most-recent.
    let dup: Vec<History> =
        (0..5i64).map(|i| history_at("echo dup", base + time::Duration::seconds(i))).collect();
    let other = history("echo other");
    let mut batch = dup.clone();
    batch.push(other.clone());

    index.add_histories(&batch);
    // Re-add the whole batch, as replaying already-downloaded records would.
    index.add_histories(&batch);

    assert_eq!(index.command_count(), 2, "duplicate invocations merge into one command each");
    let latest = dup.last().unwrap().id;
    assert_eq!(hits(&index, "echo dup"), vec![latest], "index reports the most-recent invocation");
    assert_eq!(hits(&index, "echo other"), vec![other.id]);
}

/// Replaying an empty record set builds nothing and touches neither the db nor the index.
#[tokio::test]
async fn incremental_build_over_no_records_indexes_nothing() {
    let env = TestEnv::builder().build().await;
    let index = SearchIndex::default();
    let no_ids: Vec<RecordId> = Vec::new();

    let mut batches = 0;
    let mut stream = std::pin::pin!(env.history_store.incremental_build(&env.history_db, &no_ids));
    while let Some(batch) = stream.next().await {
        index.add_histories(&batch.unwrap());
        batches += 1;
    }

    assert_eq!(batches, 0, "an empty id set yields no batches");
    assert_eq!(index.command_count(), 0, "index left empty");
    assert_eq!(env.active_rows().await, 0, "db left empty");
}

/// A replay whose db writes cannot commit (another process holds the write lock) surfaces an error
/// batch instead of panicking, and feeds nothing into the index -- no half-written command leaks
/// in. Once the lock is gone the same replay succeeds and reproduces exactly the active rows.
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
    let fault_index = SearchIndex::default();
    let lock = env.lock_history_db().await;
    let mut saw_err = false;
    {
        let mut stream = std::pin::pin!(env.history_store.incremental_build(&env.history_db, &ids));
        while let Some(batch) = stream.next().await {
            match batch {
                Ok(created) => fault_index.add_histories(&created),
                Err(_) => saw_err = true,
            }
        }
    }
    lock.release().await;
    assert!(saw_err, "a write-locked db must surface an error batch, not stall or panic");
    assert_eq!(fault_index.command_count(), 0, "no command may be torn into the index under fault");

    // After release the replay reproduces the index that matches the db's active rows.
    let good_index = SearchIndex::default();
    {
        let mut stream = std::pin::pin!(env.history_store.incremental_build(&env.history_db, &ids));
        while let Some(batch) = stream.next().await {
            good_index.add_histories(&batch.expect("replay after the lock is released"));
        }
    }
    let active = env.history_db.load_active(history_ids.clone()).await.unwrap();
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

    let index = SearchIndex::default();
    let lock = env.lock_record_store().await;
    {
        let mut stream = std::pin::pin!(env.history_store.incremental_build(&env.history_db, &ids));
        while let Some(batch) = stream.next().await {
            index.add_histories(&batch.expect("a store writer must not block a read-only replay"));
        }
    }
    lock.release().await;

    let active = env.history_db.load_active(history_ids.clone()).await.unwrap();
    assert_eq!(index.command_count(), commands.len());
    for h in &active {
        assert!(
            hits(&index, &h.command).contains(&h.id),
            "indexed despite the store lock: {}",
            h.command
        );
    }
}
