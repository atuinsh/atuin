//! Bodies of the size-tiered daemon tests, shared between `tests/scale.rs` (the tiers that run on
//! every push) and `tests/nightly.rs` (the 100k-write and 1M-row tiers the nightly workflow runs).
//! Each body takes the row count; the test binaries wrap them one tier per function.

use std::collections::HashSet;
use std::time::Instant;

use atuin_client::history::HistoryId;
use atuin_daemon::client::classify_error;

use super::TestEnv;
use super::corpus::{self, HistoryGen, seed_record_store};

pub fn report(label: &str, rows: usize, started: Instant) {
    eprintln!("[scale] {label}: {rows} rows in {:?}", started.elapsed());
}

/// After seeding, the index holds exactly the distinct indexable commands the db does.
pub async fn index_matches_db_after_seeding_body(rows: usize) {
    let started = Instant::now();
    let env = TestEnv::builder().seed_rows(rows).build().await;
    report("seed", rows, started);
    assert_eq!(env.active_rows().await, i64::try_from(rows).unwrap());
    assert_eq!(env.index_count().await, env.expected_command_count().await);
}

/// Deleting a 1% sample (what `history dedup`/`prune` typically remove) leaves exactly the other
/// 99% -- in the db, in the index, and through the Search RPC -- and the reply counts every id.
pub async fn deleting_a_sample_leaves_everything_else_body(rows: usize) {
    let env = TestEnv::builder().seed_rows(rows).build().await;
    let mut client = env.history_client().await;
    let mut search = env.search_client().await;
    let doomed: Vec<HistoryId> = env.seeded.ids.iter().copied().step_by(100).collect();
    let doomed_set: HashSet<HistoryId> = doomed.iter().copied().collect();
    // `seeded.unique` is sampled by command text alone (see `HistoryGen::is_unique`) and, like the
    // full corpus, includes the ~5% agent-run entries the search index deliberately skips (see
    // `corpus::index_eligible`). Restrict the RPC-search assertions below to eligible
    // entries, the same way `concurrency.rs` does, so this test only claims things the index
    // actually promises.
    let unique_doomed: Vec<_> = env
        .seeded
        .unique
        .iter()
        .filter(|h| doomed_set.contains(&h.id) && corpus::index_eligible(h))
        .cloned()
        .collect();
    let unique_kept: Vec<_> = env
        .seeded
        .unique
        .iter()
        .filter(|h| !doomed_set.contains(&h.id) && corpus::index_eligible(h))
        .take(20)
        .cloned()
        .collect();
    assert!(!unique_doomed.is_empty() && !unique_kept.is_empty());
    for h in &unique_doomed {
        assert_eq!(
            env.rpc_hits(&mut search, &h.command).await.first(),
            Some(&h.id),
            "before delete"
        );
    }

    let started = Instant::now();
    let reply = client.delete_history(doomed.clone()).await.unwrap();
    report("delete 1%", doomed.len(), started);

    assert_eq!(reply.deleted, u64::try_from(doomed.len()).unwrap());
    let survivors = env.active_ids().await;
    assert_eq!(survivors.len(), rows - doomed.len());
    assert!(survivors.is_disjoint(&doomed_set));
    assert_eq!(env.index_count().await, env.expected_command_count().await);
    for h in &unique_doomed {
        // At this corpus size a long, highly specific query can occasionally fuzzy-match an
        // unrelated surviving command (frizbee is an approximate matcher, not an exact-substring
        // one), so the property that matters is that the deleted id itself is gone from the
        // results -- not that the query returns nothing at all.
        let hits = env.rpc_hits(&mut search, &h.command).await;
        assert!(
            !hits.contains(&h.id),
            "deleted {} (id {}) still returned by search: {hits:?}",
            h.command,
            h.id
        );
    }
    for h in &unique_kept {
        assert_eq!(
            env.rpc_hits(&mut search, &h.command).await.first(),
            Some(&h.id),
            "kept {} lost",
            h.command
        );
    }
}

/// `atuin search --delete-it-all --include-duplicates` (or a big `history prune`) on a large
/// history: every row goes in one request, and the daemon is still healthy afterwards.
///
/// EXPECTED TO FAIL for 200k and 1M: the client sends every id in one `DeleteHistoryRequest`
/// (22 bytes each) and the server keeps tonic's 4 MiB decode limit, so anything above ~190k ids is
/// rejected with `OutOfRange` before a single row is deleted. Both failing tiers are `#[ignore]`d
/// in `tests/scale.rs` (defect H2) so CI stays green while the assertion stands; once chunking
/// lands, 200k moves back into the default run and 1M joins `tests/nightly.rs`.
pub async fn deleting_everything_in_one_request_body(rows: usize) {
    let env = TestEnv::builder().seed_rows(rows).build().await;
    let mut client = env.history_client().await;

    let started = Instant::now();
    let reply = client.delete_history(env.seeded.ids.clone()).await;
    report("delete all", rows, started);

    let healthy = env.history_client().await.status().await.unwrap().healthy;
    assert!(healthy, "daemon must survive a delete of {rows} ids");
    let reply = match reply {
        Ok(reply) => reply,
        Err(e) => {
            assert_eq!(
                env.active_rows().await,
                i64::try_from(rows).unwrap(),
                "a rejected delete must delete nothing"
            );
            panic!("delete of {rows} ids rejected ({:?}): {e:#}", classify_error(&e))
        }
    };
    assert_eq!(reply.deleted, u64::try_from(rows).unwrap());
    assert_eq!(env.active_rows().await, 0);
    assert_eq!(env.index_count().await, 0);
}

/// `atuin store rebuild history` on a large store restores every row and the whole index.
pub async fn rebuild_from_a_large_store_body(rows: usize) {
    let env = TestEnv::builder().build().await;
    let mut history_gen = HistoryGen::new(7);
    let started = Instant::now();
    let mut expected_ids = HashSet::new();
    let mut remaining = rows;
    while remaining > 0 {
        let batch: Vec<_> =
            (0..remaining.min(corpus::SEED_BATCH)).map(|_| history_gen.next()).collect();
        seed_record_store(&env.history_store, &batch).await;
        expected_ids.extend(batch.iter().map(|h| h.id));
        remaining -= batch.len();
    }
    report("seed store", rows, started);
    assert_eq!(env.active_rows().await, 0);

    let started = Instant::now();
    env.history_client().await.rebuild_history().await.unwrap();
    report("rebuild", rows, started);

    assert_eq!(env.active_ids().await, expected_ids);
    assert_eq!(env.index_count().await, env.expected_command_count().await);
}
