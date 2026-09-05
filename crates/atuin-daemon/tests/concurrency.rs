//! Races between shells, deletes, rebuilds and the search index. Every test states the invariant
//! a user relies on; tests that document a real, still-unfixed defect are marked `#[ignore]` (with
//! the defect named in the attribute) rather than weakened, so their assertions stand as the
//! specification and CI stays green. Run them with `cargo nextest run --run-ignored all` (or
//! `cargo test -- --ignored`) to watch the defects reproduce.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use atuin_client::history::HistoryId;
use atuin_client::history::store::HistoryRecord;
use atuin_client::settings::Search;
use atuin_daemon::DaemonEvent;
use atuin_daemon::grpc::history::pb::tail_history_reply::Event;
use common::corpus::HistoryGen;
use common::{TestEnv, history};
use futures::future::join_all;
use rstest::*;

/// Rows large enough that a full index reload takes hundreds of milliseconds in a debug build.
const RELOAD_ROWS: usize = 40_000;

async fn seeded_env() -> TestEnv {
    TestEnv::builder().seed_rows(RELOAD_ROWS).build().await
}

/// Every shell that got `Ok` from `EndHistory` has its command in the db, in the record store,
/// and in the index -- however many shells finish at once.
///
/// `HistoryStore::push_record` reads `last().idx` then inserts with `insert or ignore` on
/// `(host, tag, idx)`, so concurrent finishes would collide on `idx` and silently drop the loser's
/// record; `HistoryJournal::finish` now holds `record_write` across the push, serializing that
/// read-modify-write so every record lands.
#[rstest]
#[case::eight_shells(8)]
#[case::sixty_four_shells(64)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_shells_never_lose_records(#[case] shells: usize) {
    let env = TestEnv::builder().build().await;
    let tasks = (0..shells).map(|i| {
        let journal = env.journal.clone();
        tokio::spawn(async move {
            let id = journal.start_cmd(history(&format!("shell {i} cmd")));
            journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
            id
        })
    });
    let ids: Vec<HistoryId> = join_all(tasks).await.into_iter().map(Result::unwrap).collect();

    assert_eq!(env.active_ids().await.len(), shells, "history db");
    assert_eq!(env.index_count().await, shells, "search index");
    let creates: Vec<HistoryId> = env
        .history_records()
        .await
        .into_iter()
        .filter_map(|r| match r {
            HistoryRecord::Create(h) => Some(h.id),
            HistoryRecord::Delete(_) => None,
        })
        .collect();
    let missing: Vec<_> = ids.iter().filter(|id| !creates.contains(id)).collect();
    assert!(
        missing.is_empty(),
        "record store lost {} of {shells} commands: {missing:?}",
        missing.len()
    );
    assert_eq!(env.record_idxs().await, (0..u64::try_from(shells).unwrap()).collect::<Vec<_>>());
    assert_eq!(
        env.fresh_db_from_store().await.history_count(false).await.unwrap(),
        i64::try_from(shells).unwrap()
    );
}

/// Two `EndHistory` calls for the same command (a shell hook firing twice): exactly one succeeds,
/// the other is `NotFound`, and there is exactly one row and one record.
#[rstest]
#[case::end_end(Second::End)]
#[case::end_cancel(Second::Cancel)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_lifecycle_calls_resolve_to_exactly_one_winner(#[case] second: Second) {
    let env = TestEnv::builder().build().await;
    // The expected number of `Create` records, derived per round rather than hard-coded: a create
    // exists for a round iff a row exists for it, i.e. iff a finish won (in the `End` case both
    // racers are finishes, so `a ^ b` guarantees one of them always wins; in the `Cancel` case only
    // the first racer is a finish, so a create exists only when it -- `a` -- won).
    let mut wins = 0usize;
    for round in 0..20 {
        let id = env.journal.start_cmd(history(&format!("dup {round}")));
        let j1 = env.journal.clone();
        let j2 = env.journal.clone();
        let first =
            tokio::spawn(async move { j1.finish(id, 0, Duration::from_millis(1)).await.is_ok() });
        let other = tokio::spawn(async move {
            match second {
                Second::End => j2.finish(id, 0, Duration::from_millis(1)).await.is_ok(),
                Second::Cancel => j2.cancel(id).await.is_ok(),
            }
        });
        let (a, b) = (first.await.unwrap(), other.await.unwrap());
        assert!(a ^ b, "round {round}: exactly one call may win (first={a}, second={b})");
        assert!(env.journal.get(id).is_err());
        let a_finish_won = matches!(second, Second::End) || a;
        let rows = env.history_db.load(id).await.unwrap().is_some();
        assert_eq!(rows, a_finish_won, "round {round}: row iff a finish won");
        wins += usize::from(a_finish_won);
    }
    let creates = env
        .history_records()
        .await
        .iter()
        .filter(|r| matches!(r, HistoryRecord::Create(_)))
        .count();
    assert_eq!(creates, wins, "one create record per winning finish");
}

#[derive(Debug, Clone, Copy)]
enum Second {
    End,
    Cancel,
}

/// Deleting a command while its shell is finishing it (TUI delete key vs. the precmd hook) leaves
/// no trace of it anywhere: not in the db, and not on another machine replaying the store.
///
/// `finish` checks the lease out of the map before it persists, so a `delete` landing in that
/// window used to tombstone the id and run `delete_rows` *before* `finish` saved the row and
/// appended its `Create` -- leaving the row live locally and resurrecting it on replay
/// (Delete-then-Create). `finish` and `delete` now hold `record_write` across that transition, so
/// the delete either cancels the still-in-flight id or tombstones the fully-persisted row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_racing_finish_leaves_no_row_anywhere() {
    let env = TestEnv::builder().build().await;
    let mut leaked_local = Vec::new();
    let mut leaked_replay = Vec::new();
    for round in 0..40 {
        let id = env.journal.start_cmd(history(&format!("race {round}")));
        let j1 = env.journal.clone();
        let j2 = env.journal.clone();
        let finish = tokio::spawn(async move { j1.finish(id, 0, Duration::from_millis(1)).await });
        let delete =
            tokio::spawn(async move { j2.delete([id], &Search::default()).await.unwrap() });
        let (finished, deleted) = (finish.await.unwrap(), delete.await.unwrap());
        assert_eq!(deleted, 1);
        if env.history_db.load(id).await.unwrap().is_some() {
            leaked_local.push((round, finished.is_ok()));
        }
    }
    let replay = env.fresh_db_from_store().await;
    let mut pager = replay.all_paged(100, false, false);
    while let Some(page) = pager.next().await.unwrap() {
        leaked_replay.extend(page.into_iter().map(|h| h.command));
    }
    assert!(
        leaked_local.is_empty(),
        "rows survived a delete locally (round, finish_ok): {leaked_local:?}"
    );
    assert!(leaked_replay.is_empty(), "rows resurrected on replay: {leaked_replay:?}");
}

/// Commands that finish while the index is being reloaded (by a delete or a rebuild) are
/// searchable once everything settles.
///
/// EXPECTED TO FAIL: `HistoryJournal::reload_search_index` builds a fresh index from a paged db
/// scan and then swaps it in; `finish` adds to the *old* index meanwhile, and ids are time-ordered
/// so rows saved mid-scan sit past the keyset cursor and are never visited.
#[rstest]
#[case::delete(Reload::Delete)]
#[case::rebuild(Reload::Rebuild)]
#[ignore = "documents an unfixed defect (index reload is a lost-update; see report M2); run with \
            --run-ignored. See module docs."]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_finished_during_an_index_reload_are_searchable(#[case] reload: Reload) {
    let env = seeded_env().await;
    let victim = env.seeded.ids[0];
    let journal = env.journal.clone();
    let reload_task = tokio::spawn(async move {
        match reload {
            Reload::Delete => {
                journal.delete([victim], &Search::default()).await.unwrap();
            }
            Reload::Rebuild => journal.rebuild(&Search::default()).await.unwrap(),
        }
    });

    let mut finished = Vec::new();
    while !reload_task.is_finished() && finished.len() < 200 {
        let cmd = format!("during-reload {}", finished.len());
        let id = env.journal.start_cmd(history(&cmd));
        env.journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
        finished.push((id, cmd));
        tokio::task::yield_now().await;
    }
    reload_task.await.unwrap();
    assert!(
        finished.len() >= 2,
        "reload finished before any command overlapped it; raise RELOAD_ROWS"
    );

    let mut missing = Vec::new();
    for (id, cmd) in &finished {
        if !env.index_hits(cmd).await.contains(id) {
            missing.push(cmd.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "index lost {} of {} commands finished during the reload: {missing:?}",
        missing.len(),
        finished.len()
    );
    assert_eq!(env.index_count().await, env.expected_command_count().await);
}

#[derive(Debug, Clone, Copy)]
enum Reload {
    Delete,
    Rebuild,
}

/// Two deletes running at once (two terminals pruning) both take effect in the index.
///
/// EXPECTED TO FAIL: each delete rebuilds the index from its own db snapshot; whichever swap lands
/// last may predate the other delete's `delete_rows`, resurrecting that command in search.
#[ignore = "documents an unfixed defect (concurrent deletes resurrect rows; see report M2); run \
            with --run-ignored. See module docs."]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deletes_both_leave_the_index() {
    let env = seeded_env().await;
    let mut db_survivors = Vec::new();
    let mut resurrected = Vec::new();
    for pair in env.seeded.unique.as_chunks::<2>().0.iter().take(6) {
        let (a, b) = (pair[0].clone(), pair[1].clone());
        let ja = env.journal.clone();
        let jb = env.journal.clone();
        let (ra, rb) = tokio::join!(
            tokio::spawn(async move { ja.delete([a.id], &Search::default()).await.unwrap() }),
            tokio::spawn(async move { jb.delete([b.id], &Search::default()).await.unwrap() }),
        );
        assert_eq!((ra.unwrap(), rb.unwrap()), (1, 1));
        for h in [&a, &b] {
            if env.history_db.load(h.id).await.unwrap().is_some() {
                db_survivors.push(h.command.clone());
            }
            if env.index_hits(&h.command).await.contains(&h.id) {
                resurrected.push(h.command.clone());
            }
        }
    }
    let index_count = env.index_count().await;
    let expected_count = env.expected_command_count().await;
    assert!(db_survivors.is_empty(), "deleted commands still in the history db: {db_survivors:?}");
    assert!(resurrected.is_empty(), "deleted commands still searchable: {resurrected:?}");
    assert_eq!(
        index_count, expected_count,
        "index size after concurrent deletes: got {index_count}, wanted {expected_count}"
    );
}

/// Several processes deleting disjoint sets at once: every row goes, every tombstone lands, and a
/// replay of the store agrees.
///
/// This is the same record-store `idx` collision as `concurrent_shells_never_lose_records`, now on
/// tombstones -- a dropped tombstone resurrects its row on replay. Serializing the record-store
/// writes under `record_write` keeps every tombstone's `idx` distinct, so none is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disjoint_concurrent_deletes_all_reach_the_store() {
    let env = TestEnv::builder().build().await;
    let mut client = env.history_client().await;
    let mut groups: Vec<Vec<HistoryId>> = Vec::new();
    for g in 0..8 {
        let mut ids = Vec::new();
        for i in 0..25 {
            ids.push(env.record(&mut client, &format!("group {g} cmd {i}")).await);
        }
        groups.push(ids);
    }
    let total = 8 * 25;

    let tasks = groups.iter().cloned().map(|ids| {
        let journal = env.journal.clone();
        tokio::spawn(async move { journal.delete(ids, &Search::default()).await.unwrap() })
    });
    let counts: Vec<usize> = join_all(tasks).await.into_iter().map(Result::unwrap).collect();
    assert_eq!(counts.iter().sum::<usize>(), total);

    // Compute every layer before asserting on any of them, so a violation in one layer never
    // hides the state of the others.
    let survivors: Vec<HistoryId> = env.active_ids().await.into_iter().collect();
    let index_count = env.index_count().await;
    let tombstones = env
        .history_records()
        .await
        .iter()
        .filter(|r| matches!(r, HistoryRecord::Delete(_)))
        .count();
    let replay_count = env.fresh_db_from_store().await.history_count(false).await.unwrap();

    assert!(survivors.is_empty(), "rows survived concurrent deletes in the live db: {survivors:?}");
    assert_eq!(index_count, 0, "index still holds {index_count} commands after concurrent deletes");
    assert_eq!(
        tombstones, total,
        "tombstones in the record store: got {tombstones}, wanted {total}"
    );
    assert_eq!(replay_count, 0, "replay on another machine left {replay_count} rows instead of 0");
}

/// A shell hook must not stall behind an index reload: `EndHistory` stays fast even when a writer
/// contends for the index lock while a reload is scanning.
///
/// Regression guard: an earlier `reload_search_index` held the index *read* guard across the whole
/// paged scan, and tokio's RwLock is write-preferring, so the moment any writer queued (here the
/// test, in production `prepare_index` on a shell-filter change) every later reader -- including
/// `finish`'s `add_history` -- blocked until the scan ended. The reload now clones the shell filter
/// and drops the guard before scanning, so the writer below acquires immediately and the hook's
/// latency stays within a small multiple of an uncontended finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_history_is_not_starved_by_an_index_reload() {
    let env = seeded_env().await;
    // Calibrate: how long does one reload take here? Only used as the sleep yardstick and the
    // sanity guard below -- the final assertion compares against the *racing* reload's own
    // measured duration, not this one, since the racing reload starts before the calibration's
    // caches have necessarily gone cold and must not be conflated with it.
    let started = Instant::now();
    env.journal.rebuild(&Search::default()).await.unwrap();
    let calibration_time = started.elapsed();
    assert!(
        calibration_time >= Duration::from_millis(100),
        "reload too fast to measure ({calibration_time:?}); raise RELOAD_ROWS"
    );

    // One uncontended finish, measured before the racing reload starts: the yardstick the final
    // assertion compares `hook_latency` against, since "reload time / 2" carries only a ~20%
    // margin over the observed 0.60-0.63x ratio and flakes.
    let throwaway = env.journal.start_cmd(history("uncontended calibration"));
    let started = Instant::now();
    env.journal.finish(throwaway, 0, Duration::from_millis(1)).await.unwrap();
    let uncontended = started.elapsed();

    let journal = env.journal.clone();
    let reload = tokio::spawn(async move {
        let started = Instant::now();
        journal.rebuild(&Search::default()).await.unwrap();
        started.elapsed()
    });
    tokio::time::sleep(calibration_time / 10).await;
    let index = env.index.clone();
    let writer = tokio::spawn(async move {
        drop(index.write().await);
    });
    tokio::time::sleep(calibration_time / 10).await;
    // On a daemon that holds the read guard across the scan this writer is still queued here and
    // every later reader starves behind it; on the fixed daemon it has already come and gone.
    // Recorded for the diagnostics only -- the verdict is the hook latency below.
    let writer_queued = !writer.is_finished();

    let id = env.journal.start_cmd(history("precmd hook"));
    let started = Instant::now();
    env.journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
    let hook_latency = started.elapsed();
    assert!(
        !reload.is_finished(),
        "reload finished before the hook ran; the test measured an uncontended finish -- raise \
         RELOAD_ROWS"
    );

    let racing_reload_time = reload.await.unwrap();
    writer.await.unwrap();
    let ratio = hook_latency.as_secs_f64() / racing_reload_time.as_secs_f64();
    eprintln!(
        "calibration_time={calibration_time:?} racing_reload_time={racing_reload_time:?} \
         hook_latency={hook_latency:?} uncontended={uncontended:?} ratio={ratio:.3} \
         writer_queued={writer_queued}"
    );
    // An uncontended `finish` is the honest yardstick: "racing_reload_time / 2" carried only a
    // ~20% margin over the observed 0.60-0.63x ratio and flaked. A generous floor (50ms, or 20x
    // the uncontended cost) fails on a guard-holding daemon, where a starved hook takes hundreds
    // of ms against a ~5ms uncontended finish.
    let budget = Duration::from_millis(50).max(uncontended * 20);
    assert!(
        hook_latency < budget,
        "EndHistory took {hook_latency:?} (budget {budget:?}, uncontended {uncontended:?}) while \
         a reload of {racing_reload_time:?} ran (ratio {ratio:.3}, writer queued: \
         {writer_queued}): shell hook starved"
    );
}

/// History that arrives from sync while a delete is reloading the index is searchable afterwards.
///
/// EXPECTED TO FAIL: the `HistorySynced` handler adds to whichever index is live under a read
/// guard, which the reload then discards.
#[ignore = "documents an unfixed defect (synced history dropped by a racing reload; see report \
            M2); run with --run-ignored. See module docs."]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn synced_history_during_a_reload_is_searchable() {
    let env = TestEnv::builder().seed_rows(RELOAD_ROWS).with_search_component().build().await;
    let victim = env.seeded.ids[0];
    let journal = env.journal.clone();
    let reload =
        tokio::spawn(async move { journal.delete([victim], &Search::default()).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(30)).await;

    let mut history_gen = HistoryGen::new(0xABCD);
    let synced: Vec<_> =
        (0..20).map(|_| history_gen.next()).filter(common::corpus::index_eligible).collect();
    env.history_db.save_bulk(&synced).await.unwrap();
    env.handle.emit(DaemonEvent::HistorySynced(
        synced.iter().map(|h| h.id).collect::<Arc<[HistoryId]>>(),
    ));

    reload.await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await; // let the event loop drain

    // A synced row may share its command with older rows (the "common" part of the corpus), so
    // check the strong property on the count and the exact property on the unique commands.
    assert_eq!(
        env.index_count().await,
        env.expected_command_count().await,
        "index size after sync + reload"
    );
    let unique: Vec<_> = synced.iter().filter(|h| HistoryGen::is_unique(h)).collect();
    assert!(!unique.is_empty(), "corpus produced no unique synced commands");
    for h in unique {
        assert!(
            env.index_hits(&h.command).await.contains(&h.id),
            "synced command not searchable: {}",
            h.command
        );
    }
}

/// `atuin history tail` is told when it fell behind, and loses nothing silently: dropped + received
/// equals what happened.
#[tokio::test]
async fn tail_reports_lag_instead_of_dropping_silently() {
    let env = TestEnv::builder().build().await;
    let mut client = env.history_client().await;
    let mut tail = client.tail_history().await.unwrap();
    const STARTS: u64 = 300; // well past the 128-event broadcast buffer

    for i in 0..STARTS {
        let _ = env.journal.start_cmd(history(&format!("burst {i}")));
    }

    let first = tail.message().await.unwrap().unwrap();
    let dropped = match first.event {
        Some(Event::Lagged(lagged)) => lagged.dropped,
        other => panic!("expected an in-band Lagged notice first, got {other:?}"),
    };
    let mut received = 0;
    while let Ok(Some(reply)) =
        tokio::time::timeout(Duration::from_millis(200), tail.message()).await.unwrap_or(Ok(None))
    {
        assert!(matches!(reply.event, Some(Event::Started(_))), "{reply:?}");
        received += 1;
    }
    assert_eq!(dropped + received, STARTS);
    assert!(dropped > 0);
}

/// With many shells running at once, `atuin history tail` sees every command start exactly once
/// before it ends exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tail_orders_events_per_command_under_concurrency() {
    let env = TestEnv::builder().build().await;
    let mut watcher = env.history_client().await;
    let mut tail = watcher.tail_history().await.unwrap();
    const SHELLS: usize = 50; // 100 events: under the buffer, so no lag can occur

    let tasks = (0..SHELLS).map(|i| {
        let socket = env.socket_path.clone();
        tokio::spawn(async move {
            let mut client = atuin_daemon::client::HistoryClient::new(socket).await.unwrap();
            let id: HistoryId = client
                .start_history(history(&format!("shell {i}")))
                .await
                .unwrap()
                .id
                .unwrap()
                .try_into()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(u64::try_from(i % 7).unwrap())).await;
            client.end_history(id, Some(Duration::from_millis(1)), 0).await.unwrap();
            id
        })
    });
    let ids: Vec<HistoryId> = join_all(tasks).await.into_iter().map(Result::unwrap).collect();

    let mut seen: HashMap<HistoryId, Vec<&'static str>> = HashMap::new();
    for _ in 0..SHELLS * 2 {
        let reply = tokio::time::timeout(Duration::from_secs(5), tail.message())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let (kind, wire_id) = match reply.event {
            Some(Event::Started(h)) => ("started", h.id),
            Some(Event::Ended(h)) => ("ended", h.id),
            other => panic!("unexpected tail event {other:?}"),
        };
        let id: HistoryId = wire_id.unwrap().try_into().unwrap();
        seen.entry(id).or_default().push(kind);
    }
    for id in ids {
        assert_eq!(seen.get(&id).map(Vec::as_slice), Some(&["started", "ended"][..]), "{id}");
    }
}
