use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use atuin_desktop_capture::{Content, ContentHash, Record, Trigger};
use rstest::rstest;
use tempfile::TempDir;
use uuid::Uuid;

use super::schema::{StoredBody, StoredRecord, Timestamp};
use super::*;
use crate::desktop_capture::testing::records::{
    background, background_at, dialog, ended, everything, forged_background, hash, lines, noise,
    notification, notification_from, page, snapshot, started, unchanged, write_all, written,
};
use crate::desktop_capture::testing::{OpenedStore, dir, opened_store, store_in};

/// A moment in the past, `secs` seconds after a fixed start.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(1_700_000_000 + secs)
        + Duration::from_nanos(123_456_789)
}

async fn all_segments(store: &DesktopStore) -> Vec<SegmentSummary> {
    store.segments(everything()).await.expect("segments")
}

async fn only_segment(store: &DesktopStore) -> SegmentSummary {
    let mut segments = all_segments(store).await;
    assert_eq!(segments.len(), 1, "{segments:#?}");
    segments.remove(0)
}

async fn read(store: &DesktopStore, segment: SegmentKey, which: Which) -> SnapshotRead {
    store.read(segment, which).await.expect("read").expect("a snapshot")
}

fn titles(segments: &[SegmentSummary]) -> Vec<&str> {
    segments.iter().map(|segment| segment.title.as_str()).collect()
}

#[rstest]
#[tokio::test]
async fn identical_snapshots_store_one_content(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let mut run = RunState::default();
    let start = store.write(&mut run, started(1, "firefox", "Docs", at(0))).await.unwrap();
    assert_eq!(start.item, None);

    let first = store.write(&mut run, snapshot(1, at(1), lines(3))).await.unwrap();
    let item = first.item.expect("the first snapshot is a new content");
    assert_eq!(item.key.hash, hash(&lines(3)));
    assert_eq!((&*item.app, &*item.title), ("firefox", "Docs"));
    assert!(item.text.contains("line 3"), "{}", item.text);
    for second in 2..=3 {
        let again = store.write(&mut run, snapshot(1, at(second), lines(3))).await.unwrap();
        assert_eq!(again.item, None);
    }

    assert_eq!(store.keyspaces().stored_contents().len(), 1);
    let segment = only_segment(store).await;
    assert_eq!(segment.key, item.key.segment);
    assert_eq!(segment.snapshots, 3);
}

/// An unchanged snapshot carries only the hash of its content, which the segment holds.
#[rstest]
#[tokio::test]
async fn an_unchanged_snapshot_stores_an_observation(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let mut run = RunState::default();
    let _ = store.write(&mut run, started(1, "firefox", "Docs", at(0))).await.unwrap();
    let _ = store.write(&mut run, snapshot(1, at(1), lines(3))).await.unwrap();

    let again = store.write(&mut run, unchanged(1, at(2), &lines(3))).await.unwrap();

    assert_eq!(again.item, None);
    assert_eq!(store.keyspaces().stored_contents().len(), 1);
    let segment = only_segment(store).await;
    assert_eq!(segment.snapshots, 2);
    let latest = read(store, segment.key, Which::Latest).await;
    assert_eq!((latest.at, latest.root), (at(2), lines(3)));
}

/// The store skips an unchanged snapshot of a content that the segment does not hold, as
/// after a failed write of the changed snapshot before it, because the unchanged snapshot
/// doesn't carry a tree to store.
#[rstest]
#[tokio::test]
async fn an_unchanged_snapshot_of_a_missing_content_is_skipped(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let mut run = RunState::default();
    let _ = store.write(&mut run, started(1, "firefox", "Docs", at(0))).await.unwrap();
    let _ = store.write(&mut run, snapshot(1, at(1), lines(3))).await.unwrap();

    let missing = store.write(&mut run, unchanged(1, at(2), &lines(4))).await.unwrap();
    let after = store.write(&mut run, snapshot(1, at(3), lines(5))).await.unwrap();

    assert_eq!(missing.item, None);
    assert!(after.item.is_some());
    let segment = only_segment(store).await;
    assert_eq!(segment.snapshots, 2);
    assert_eq!(store.keyspaces().stored_contents().len(), 2);
}

#[rstest]
#[tokio::test]
async fn a_whole_tree_every_32_contents(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(
        store,
        std::iter::once(started(1, "firefox", "Log", at(0)))
            .chain((1..=70).map(|n| snapshot(1, at(n), lines(n.try_into().unwrap())))),
    )
    .await;

    let contents = store.keyspaces().stored_contents();
    assert_eq!(contents.len(), 70);
    let wholes: HashSet<ContentHash> =
        contents.iter().filter(|entry| entry.base.is_none()).map(|entry| entry.key.hash).collect();
    assert_eq!(wholes, HashSet::from([hash(&lines(1)), hash(&lines(33)), hash(&lines(65))]));

    let segment = only_segment(store).await;
    let latest = read(store, segment.key, Which::Latest).await;
    assert_eq!(latest.root, lines(70));
    assert_eq!(latest.at, at(70));

    // The 32nd and 64th contents end the longest chains: a whole tree and 31 deltas.
    let ids = store.keyspaces().record_ids(segment.key);
    for n in [32, 33, 64] {
        let chosen = read(store, segment.key, Which::Record(ids[n])).await;
        assert_eq!(chosen.root, lines(n), "content {n}");
    }
}

/// A read of several snapshots returns the texts that one read of each snapshot returns, across
/// a whole tree, and expands each content once.
#[rstest]
#[tokio::test]
async fn a_read_of_several_snapshots_expands_each_content_once(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(
        store,
        std::iter::once(started(1, "firefox", "Log", at(0)))
            .chain((1..=40).map(|n| snapshot(1, at(n), lines(n.try_into().unwrap()))))
            .chain([snapshot(1, at(41), lines(40))]),
    )
    .await;
    let segment = only_segment(store).await;
    let listed = store.snapshots(segment.key.0).await.expect("snapshots").expect("a segment");
    let records: Vec<_> = listed.snapshots.iter().map(|snapshot| snapshot.record).collect();
    assert_eq!(records.len(), 41);
    let mut expected = Vec::new();
    for &record in &records {
        let read = store.read_text(record, None, 64).await.expect("read").expect("a snapshot");
        expected.push(RecordText {
            record,
            text: read.text,
            truncated: read.truncated,
        });
    }
    let before = store.expansions();

    let read = store
        .read_snapshots(segment.key.0, records.iter().copied().collect(), budget(64))
        .await
        .expect("read")
        .expect("a segment");

    assert_eq!(read.left_out, 0);
    let texts = read.texts;
    assert_eq!(texts, expected);
    assert!(texts.iter().any(|text| text.truncated), "{texts:?}");
    assert_eq!(store.expansions() - before, 40);
}

/// A read of several snapshots leaves out the records that aren't snapshots of the segment.
#[rstest]
#[tokio::test]
async fn a_read_of_several_snapshots_takes_only_its_segment(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        started(2, "gedit", "Notes", at(2)),
        snapshot(2, at(3), lines(2)),
    ])
    .await;
    let segments = all_segments(store).await;
    let first = store.snapshots(segments[0].key.0).await.unwrap().unwrap().snapshots[0].record;
    let second = store.snapshots(segments[1].key.0).await.unwrap().unwrap().snapshots[0].record;

    let read = store
        .read_snapshots(first, HashSet::from([first, second]), budget(usize::MAX))
        .await
        .expect("read")
        .expect("a segment");

    assert_eq!(read.texts.iter().map(|text| text.record).collect::<Vec<_>>(), [first]);
    assert_eq!(read.left_out, 0);
    let unknown = store.read_snapshots(Uuid::now_v7(), HashSet::new(), budget(usize::MAX)).await;
    assert_eq!(unknown.expect("read"), None);
}

/// A budget that cuts each text to `per_text` bytes, without a limit on the total.
fn budget(per_text: usize) -> TextBudget {
    TextBudget {
        per_text,
        total: usize::MAX,
    }
}

/// A read of several snapshots ends before the first text that would pass the total budget,
/// counts the snapshots that it leaves out, and always returns its first text. The budget is
/// the length of the first `fitting` texts together.
#[rstest]
#[case::the_first_text_passes(0, 1)]
#[case::one_text_fits(1, 1)]
#[case::two_texts_fit(2, 2)]
#[case::every_text_fits(3, 3)]
#[tokio::test]
async fn a_read_of_several_snapshots_keeps_within_its_total(
    #[future(awt)] opened_store: OpenedStore,
    #[case] fitting: usize,
    #[case] returned: usize,
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(4)),
        snapshot(1, at(2), lines(5)),
        snapshot(1, at(3), lines(6)),
    ])
    .await;
    let segment = only_segment(store).await;
    let listed = store.snapshots(segment.key.0).await.expect("snapshots").expect("a segment");
    let records: Vec<_> = listed.snapshots.iter().map(|snapshot| snapshot.record).collect();
    let mut total = 0;
    for &record in &records[..fitting] {
        let read = store.read_text(record, None, usize::MAX).await.expect("read");
        total += read.expect("a snapshot").text.len();
    }
    let budget = TextBudget {
        per_text: usize::MAX,
        total,
    };

    let read = store
        .read_snapshots(segment.key.0, records.iter().copied().collect(), budget)
        .await
        .expect("read")
        .expect("a segment");

    let read_records: Vec<_> = read.texts.iter().map(|text| text.record).collect();
    assert_eq!(read_records, records[..returned]);
    assert_eq!(read.left_out, 3 - returned);
}

#[rstest]
#[tokio::test]
async fn deltas_never_cross_segments(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        snapshot(1, at(2), lines(2)),
        ended(1, at(3)),
        started(2, "firefox", "Log", at(4)),
        snapshot(2, at(5), lines(2)),
    ])
    .await;

    let segments = all_segments(store).await;
    assert_eq!(segments.len(), 2);
    let second = store
        .keyspaces()
        .stored_contents()
        .into_iter()
        .find(|entry| entry.key.segment == segments[1].key)
        .expect("the second segment's content");
    assert_eq!(second.key.hash, hash(&lines(2)));
    assert_eq!(second.base, None);
    assert_eq!(read(store, segments[1].key, Which::Latest).await.root, lines(2));
}

#[rstest]
#[tokio::test]
async fn a_dialog_is_a_closed_segment(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let items = written(store, [dialog("gedit", "Save changes?", at(5), lines(2))]).await;
    assert_eq!(items.len(), 1);

    let segment = only_segment(store).await;
    assert_eq!(segment.kind, SegmentKind::Dialog);
    assert_eq!((segment.app.as_str(), segment.title.as_str()), ("gedit", "Save changes?"));
    assert_eq!(segment.snapshots, 1);
    assert_eq!(segment.started, at(5));
    assert_eq!(segment.ended, Some(segment.started));
    assert_eq!(read(store, segment.key, Which::Latest).await.root, lines(2));
    assert_eq!(items[0].key.segment, segment.key);
}

#[rstest]
#[tokio::test]
async fn background_reads_share_a_segment_per_window(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        background(7, "kate", "notes.txt", at(1), lines(1)),
        background(7, "kate", "notes.txt", at(2), lines(2)),
        background(7, "kate", "notes.txt", at(3), lines(1)),
    ])
    .await;

    let segment = only_segment(store).await;
    assert_eq!(segment.kind, SegmentKind::Background);
    assert_eq!(segment.started, at(1));
    assert_eq!(segment.ended, None);
    assert_eq!(segment.snapshots, 3);
    assert_eq!(store.keyspaces().stored_contents().len(), 2);
    assert_eq!(read(store, segment.key, Which::Latest).await.root, lines(1));
}

/// A background read whose title or URL differs from its segment's start starts a new segment,
/// so that every segment says what its window showed.
#[rstest]
#[tokio::test]
async fn a_background_window_that_changes_starts_a_new_segment(
    #[future(awt)] opened_store: OpenedStore,
) {
    let OpenedStore { dir, store } = opened_store;
    let url = Some("https://example.com/b");
    let _ = written(&store, [
        background_at(7, "kate", "a.txt", None, at(1), lines(1)),
        background_at(7, "kate", "a.txt", None, at(2), lines(2)),
        background_at(7, "kate", "b.txt", None, at(3), lines(3)),
        background_at(7, "kate", "b.txt", url, at(4), lines(4)),
    ])
    .await;

    let segments = all_segments(&store).await;
    assert_eq!(titles(&segments), ["a.txt", "b.txt", "b.txt"]);
    let ends: Vec<_> = segments.iter().map(|segment| segment.ended).collect();
    assert_eq!(ends, [Some(at(3)), Some(at(4)), None]);
    let urls: Vec<_> = segments.iter().map(|segment| segment.url.as_deref()).collect();
    assert_eq!(urls, [None, None, url]);
    let snapshots: Vec<_> = segments.iter().map(|segment| segment.snapshots).collect();
    assert_eq!(snapshots, [2, 1, 1]);
    assert_eq!(read(&store, segments[2].key, Which::Latest).await.root, lines(4));

    // The sweep after a restart ends only the segment that was open, and a second sweep doesn't
    // end any more segments.
    for _ in 0..2 {
        store.close().await.expect("close");
        let store = store_in(&dir).await;
        store.close_open_segments().await.expect("sweep");
        let ends: Vec<_> = all_segments(&store).await.iter().map(|segment| segment.ended).collect();
        assert_eq!(ends, [Some(at(3)), Some(at(4)), Some(at(4))]);
        store.close().await.expect("close");
    }
}

/// A background segment ends where its next content would be a whole tree, and its successor
/// starts with that tree, so that an eviction can remove the older part of a long run.
#[rstest]
#[tokio::test]
async fn a_background_segment_rolls_over_at_a_whole_tree(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let reads = (1..=40).map(|n| background(7, "kate", "log", at(n), lines(n.try_into().unwrap())));
    let items = written(store, reads).await;
    assert_eq!(items.len(), 40);

    let segments = all_segments(store).await;
    let counts: Vec<_> = segments.iter().map(|segment| segment.snapshots).collect();
    assert_eq!(counts, [32, 8]);
    assert_eq!(segments[0].ended, Some(at(33)));
    assert_eq!((segments[1].started, segments[1].ended), (at(33), None));
    let wholes = store.keyspaces().stored_contents().into_iter().filter(|c| c.base.is_none());
    assert_eq!(wholes.count(), 2);
    assert_eq!(read(store, segments[1].key, Which::Latest).await.root, lines(40));

    let evicted = store.evict(u64::MAX).await.expect("evict");
    assert_eq!(evicted.segments, [segments[0].key]);
    assert_eq!(only_segment(store).await.key, segments[1].key);
}

#[rstest]
#[tokio::test]
async fn a_background_segment_rolls_over_after_an_hour(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        background(7, "kate", "log", at(0), lines(1)),
        background(7, "kate", "log", at(3599), lines(2)),
        background(7, "kate", "log", at(3600), lines(3)),
    ])
    .await;

    let segments = all_segments(store).await;
    let spans: Vec<_> = segments.iter().map(|segment| (segment.started, segment.ended)).collect();
    assert_eq!(spans, [(at(0), Some(at(3600))), (at(3600), None)]);
}

/// A background read that fails to store neither ends the window's segment nor starts an empty
/// one, before or after a reopen. A later read of the window rolls the segment over.
#[rstest]
#[tokio::test]
async fn a_background_read_that_fails_leaves_no_empty_segment(
    #[future(awt)] opened_store: OpenedStore,
) {
    let OpenedStore { dir, store } = opened_store;
    let mut run = RunState::default();
    write_all(&store, &mut run, [background(7, "kate", "a.txt", at(1), lines(1))]).await;

    for read in [forged_background(7, "b.txt", at(2)), forged_background(8, "c.txt", at(3))] {
        let written = store.write(&mut run, read).await;
        assert!(matches!(written, Err(StoreError::SentHash)), "{written:?}");
    }

    let first = only_segment(&store).await;
    assert_eq!((first.title.as_str(), first.ended, first.snapshots), ("a.txt", None, 1));
    write_all(&store, &mut run, [background(7, "kate", "b.txt", at(4), lines(2))]).await;
    let spans: Vec<_> = all_segments(&store)
        .await
        .into_iter()
        .map(|segment| (segment.title, segment.ended, segment.snapshots))
        .collect();
    assert_eq!(spans, [("a.txt".to_owned(), Some(at(4)), 1), ("b.txt".to_owned(), None, 1)]);
    store.close().await.expect("close");

    let store = store_in(&dir).await;
    store.close_open_segments().await.expect("sweep");
    let snapshots: Vec<_> =
        all_segments(&store).await.into_iter().map(|segment| segment.snapshots).collect();
    assert_eq!(snapshots, [1, 1]);
    store.close().await.expect("close");
}

/// A background segment that the run has not written to for an hour ends an hour after its last
/// write.
///
/// The next read of its window starts a new segment. The end is stored, so after a reopen an
/// eviction removes the ended segment and keeps the segments that the run left open.
#[rstest]
#[tokio::test]
async fn an_idle_background_segment_ends_an_hour_after_its_last_write(
    #[future(awt)] opened_store: OpenedStore,
) {
    let OpenedStore { dir, store } = opened_store;
    let mut run = RunState::default();
    write_all(&store, &mut run, [
        background(7, "kate", "quiet", at(0), lines(1)),
        background(8, "kate", "busy", at(1), lines(1)),
        background(7, "kate", "quiet", at(10), lines(2)),
        background(8, "kate", "busy", at(3000), lines(2)),
    ])
    .await;

    store.end_idle_background(&mut run, at(3610)).await.expect("end the idle segments");

    let spans = |segments: Vec<SegmentSummary>| -> Vec<(String, SystemTime, Option<SystemTime>)> {
        segments
            .into_iter()
            .map(|segment| (segment.title, segment.started, segment.ended))
            .collect()
    };
    assert_eq!(spans(all_segments(&store).await), [
        ("quiet".to_owned(), at(0), Some(at(3610))),
        ("busy".to_owned(), at(1), None),
    ]);
    store.end_idle_background(&mut run, at(3610)).await.expect("nothing more is idle");
    write_all(&store, &mut run, [background(7, "kate", "quiet", at(4000), lines(3))]).await;
    let segments = all_segments(&store).await;
    assert_eq!(spans(segments.clone())[2], ("quiet".to_owned(), at(4000), None));
    store.close().await.expect("close");

    let store = store_in(&dir).await;
    let evicted = store.evict(u64::MAX).await.expect("evict");
    assert_eq!(evicted.segments, [segments[0].key]);
    assert_eq!(spans(all_segments(&store).await), [
        ("busy".to_owned(), at(1), Some(at(3000))),
        ("quiet".to_owned(), at(4000), Some(at(4000))),
    ]);
    store.close().await.expect("close");
}

#[rstest]
#[tokio::test]
async fn first_and_latest(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        snapshot(1, at(2), lines(2)),
        snapshot(1, at(3), lines(3)),
    ])
    .await;

    let segment = only_segment(store).await;
    let first = read(store, segment.key, Which::First).await;
    assert_eq!((first.at, first.root), (at(1), lines(1)));
    let latest = read(store, segment.key, Which::Latest).await;
    assert_eq!((latest.at, latest.root), (at(3), lines(3)));
    assert_eq!(latest.segment, segment);

    // The records are the segment's start and its three snapshots.
    let middle = store.keyspaces().record_ids(segment.key)[2];
    let chosen = read(store, segment.key, Which::Record(middle)).await;
    assert_eq!((chosen.at, chosen.root), (at(2), lines(2)));
    let start = store.keyspaces().record_ids(segment.key)[0];
    assert_eq!(store.read(segment.key, Which::Record(start)).await.unwrap(), None);
}

#[rstest]
#[case::whole(0, "line 1\nline 2", false)]
#[case::cut(9, "line\n[…]\nne 2", true)]
#[tokio::test]
async fn read_text_renders_a_snapshot(
    #[future(awt)] opened_store: OpenedStore,
    #[case] max_bytes: usize,
    #[case] text: &str,
    #[case] truncated: bool,
) {
    let store = &opened_store.store;
    let _ =
        written(store, [started(1, "firefox", "Log", at(0)), snapshot(1, at(1), lines(2))]).await;
    let segment = only_segment(store).await;
    let max_bytes = if max_bytes == 0 {
        usize::MAX
    } else {
        max_bytes
    };

    let read = store.read_text(segment.key.0, Some(Which::Latest), max_bytes).await.expect("read");

    let read = read.expect("a snapshot");
    assert_eq!((read.text.as_str(), read.truncated), (text, truncated));
    assert_eq!((read.segment, read.at), (segment, at(1)));
}

#[rstest]
#[tokio::test]
async fn snapshots_list_in_order_and_mark_repeats(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        snapshot(1, at(2), lines(1)),
        snapshot(1, at(3), lines(2)),
        snapshot(1, at(4), lines(1)),
    ])
    .await;

    let segment = only_segment(store).await;
    let listed = store.snapshots(segment.key.0).await.expect("snapshots").expect("a segment");

    assert_eq!(listed.segment, segment);
    // The records are the segment's start and its four snapshots.
    let records = store.keyspaces().record_ids(segment.key);
    let expected =
        [(1, false), (2, true), (3, false), (4, false)].map(|(n, same)| SnapshotSummary {
            record: records[n],
            at: at(n.try_into().expect("a small index")),
            trigger: Some(Trigger::Polled),
            nodes: 10,
            same_as_previous: same,
        });
    assert_eq!(listed.snapshots, expected);
}

#[rstest]
#[tokio::test]
async fn a_dialog_snapshot_has_no_trigger(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [dialog("gedit", "Save?", at(1), lines(1))]).await;

    let segment = only_segment(store).await;
    let listed = store.snapshots(segment.key.0).await.expect("snapshots").expect("a segment");

    assert_eq!(listed.snapshots.len(), 1);
    assert_eq!(listed.snapshots[0].trigger, None);
}

#[rstest]
#[tokio::test]
async fn an_unknown_segment_has_no_snapshots(#[future(awt)] opened_store: OpenedStore) {
    let unknown = SegmentKey(Uuid::now_v7());
    assert_eq!(opened_store.store.snapshots(unknown.0).await.expect("snapshots"), None);
}

/// A segment that a crash left open ends at its last record, once.
#[rstest]
#[tokio::test]
async fn dangling_segments_end_at_their_last_record(#[future(awt)] opened_store: OpenedStore) {
    let OpenedStore { dir, store } = opened_store;
    let _ = written(&store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        snapshot(1, at(2), lines(2)),
        started(2, "gedit", "Empty", at(3)),
    ])
    .await;
    store.close().await.expect("close");

    for _ in 0..2 {
        let store = store_in(&dir).await;
        store.close_open_segments().await.expect("close");

        let segments = all_segments(&store).await;
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].ended, Some(at(2)), "the segment with snapshots");
        assert_eq!(segments[1].ended, Some(at(3)), "the segment without snapshots");
        let records = |segment: &SegmentSummary| store.keyspaces().record_ids(segment.key).len();
        assert_eq!(records(&segments[0]), 4, "start, snapshots, and end");
        assert_eq!(records(&segments[1]), 2, "start and end");
        store.close().await.expect("close");
    }
}

/// Reads show a segment of a helper run that is over as ended, even before
/// `close_open_segments` writes its end.
#[rstest]
#[tokio::test]
async fn dangling_segments_read_as_ended_before_the_sweep(
    #[future(awt)] opened_store: OpenedStore,
) {
    let OpenedStore { dir, store } = opened_store;
    let _ = written(&store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        snapshot(1, at(2), lines(2)),
        started(2, "gedit", "Empty", at(3)),
    ])
    .await;
    let open: Vec<_> = all_segments(&store).await.iter().map(|segment| segment.ended).collect();
    assert_eq!(open, [None, None], "this process's segments are open");
    store.close().await.expect("close");

    let store = store_in(&dir).await;
    let segments = all_segments(&store).await;
    let ended: Vec<_> = segments.iter().map(|segment| segment.ended).collect();
    assert_eq!(ended, [Some(at(2)), Some(at(3))]);
    assert_eq!(read(&store, segments[0].key, Which::Latest).await.segment.ended, Some(at(2)));

    let later = SegmentFilter {
        from: at(10),
        ..everything()
    };
    assert_eq!(store.segments(later).await.expect("segments"), []);
}

/// A UUIDv7 a day after now, as a daemon would make it with a clock that ran a day ahead.
fn a_day_ahead(offset: u8) -> Uuid {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("after 1970");
    let millis = u64::try_from(now.as_millis()).expect("a small time") + 86_400_000;
    uuid::Builder::from_unix_timestamp_millis(millis, &[offset; 10]).into_uuid()
}

/// A daemon whose clock reads earlier than the newest stored record still writes each new
/// record after the stored ones, so that a sweep's end is its segment's last record.
#[rstest]
#[tokio::test]
async fn ids_follow_the_stored_ones_when_the_clock_runs_back(
    #[future(awt)] opened_store: OpenedStore,
) {
    let OpenedStore { dir, store } = opened_store;
    let keyspaces = store.keyspaces();
    let segment = SegmentKey(a_day_ahead(0));
    let record = |body| StoredRecord {
        host: keyspaces.host(),
        segment,
        at: Timestamp::from(at(1)),
        body,
    };
    let start = record(StoredBody::SegmentStarted(schema::SegmentStart {
        kind: SegmentKind::Active,
        app: "gedit".to_owned(),
        title: "Ahead".to_owned(),
        url: None,
    }));
    keyspaces.put_record(segment.0, &start);
    drop(keyspaces);
    store.close().await.expect("close");

    for _ in 0..2 {
        let store = store_in(&dir).await;
        store.close_open_segments().await.expect("sweep");
        let ids = store.keyspaces().record_ids(segment);
        assert_eq!(ids.len(), 2, "the start and one end");
        assert!(ids[1] > segment.0, "the end follows the start");
        assert_eq!(only_segment(&store).await.ended, Some(at(1)));
        store.close().await.expect("close");
    }

    let store = store_in(&dir).await;
    let _ = written(&store, [dialog("gedit", "Later", at(2), lines(1))]).await;
    let segments = all_segments(&store).await;
    assert_eq!(titles(&segments), ["Ahead", "Later"]);
    assert!(segments[1].key > segment, "a new segment follows the stored ones");
    assert_eq!(store.evict(u64::MAX).await.expect("evict").segments.len(), 2);
    assert!(store.keyspaces().is_empty());
}

#[rstest]
#[case::everything(0, 1_000, None, Kinds::All, &["A", "dialog", "bg", "B"])]
#[case::before_the_dialog(0, 120, None, Kinds::All, &["A"])]
#[case::after_the_dialog(160, 260, None, Kinds::All, &["A", "bg"])]
#[case::open_segments_reach_past_their_last_record(400, 500, None, Kinds::All, &["bg", "B"])]
#[case::one_app(0, 1_000, Some("firefox"), Kinds::All, &["A", "dialog"])]
#[case::one_app_in_any_case(0, 1_000, Some("FireFox"), Kinds::All, &["A", "dialog"])]
#[case::dialogs(0, 1_000, None, Kinds::Only(vec![SegmentKind::Dialog]), &["dialog"])]
#[case::windows(
    0,
    1_000,
    None,
    Kinds::Only(vec![SegmentKind::Active, SegmentKind::Background]),
    &["A", "bg", "B"],
)]
#[tokio::test]
async fn filter_by_time_app_and_kind(
    #[future(awt)] opened_store: OpenedStore,
    #[case] from: u64,
    #[case] to: u64,
    #[case] app: Option<&str>,
    #[case] kinds: Kinds,
    #[case] titles: &[&str],
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "A", at(100)),
        snapshot(1, at(110), lines(1)),
        dialog("firefox", "dialog", at(150), lines(2)),
        ended(1, at(200)),
        background(7, "kate", "bg", at(250), lines(3)),
        started(2, "gedit", "B", at(300)),
        snapshot(2, at(310), lines(4)),
    ])
    .await;

    let filter = SegmentFilter {
        from: at(from),
        to: at(to),
        app: app.map(str::to_owned),
        kinds,
    };
    let segments = store.segments(filter.clone()).await.expect("segments");
    let found: Vec<&str> = segments.iter().map(|segment| segment.title.as_str()).collect();
    assert_eq!(found, titles);

    for limit in 0..=segments.len() + 1 {
        let newest = store.newest_segments(filter.clone(), limit).await.expect("newest segments");
        let skipped = segments.len().saturating_sub(limit);
        assert_eq!(newest, segments[skipped..], "the newest {limit}");
    }
}

/// The newest segments of a range include a segment that started before the range and lasts
/// into it, and leave out a segment that ended before the range.
#[rstest]
#[tokio::test]
async fn newest_segments_include_a_segment_that_lasts_into_the_range(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let now = SystemTime::now;
    let mut run = RunState::default();
    write_all(store, &mut run, [
        started(1, "gedit", "over", now()),
        ended(1, now()),
        started(2, "gedit", "lasting", now()),
    ])
    .await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let from = now();
    tokio::time::sleep(Duration::from_millis(5)).await;
    write_all(store, &mut run, [snapshot(2, now(), lines(1)), started(3, "kate", "new", now())])
        .await;
    let filter = SegmentFilter {
        from,
        ..everything()
    };

    for limit in 0..=3 {
        let newest = store.newest_segments(filter.clone(), limit).await.expect("newest segments");
        let expected = ["lasting", "new"];
        let skipped = expected.len().saturating_sub(limit);
        assert_eq!(titles(&newest), expected[skipped..], "the newest {limit}");
    }
}

#[rstest]
#[case::with_body("hello", "body of hello")]
#[case::empty_body("hello", "")]
#[tokio::test]
async fn a_notification_is_stored_as_a_notification_segment(
    #[future(awt)] opened_store: OpenedStore,
    #[case] title: &str,
    #[case] body: &str,
) {
    let store = &opened_store.store;
    let Record::Notification(mut notice) = notification(title, at(3)) else {
        unreachable!("a notification");
    };
    notice.content = Content::of_notice(body.to_owned());

    let written = store.write(&mut RunState::default(), Record::Notification(notice)).await;
    assert!(written.is_ok());

    let segment = only_segment(store).await;
    assert_eq!(segment.kind, SegmentKind::Notification);
    assert_eq!(segment.title, title);
    assert_eq!(segment.started, at(3));
    assert_eq!(segment.ended, Some(at(3)));
    assert_eq!(segment.snapshots, 1);
    let latest = read(store, segment.key, Which::Latest).await;
    assert_eq!(atuin_desktop_capture::render(&latest.root, usize::MAX).text, body);
}

#[rstest]
#[tokio::test]
async fn recent_activity_reads_segments_dialogs_and_notifications(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "before", at(0)),
        ended(1, at(5)),
        started(2, "firefox", "A", at(10)),
        dialog("gedit", "Save?", at(20), page(["Save the file?".to_owned()])),
        background(7, "kate", "bg", at(25), lines(3)),
        notification("Backup failed", at(30)),
    ])
    .await;

    let activity = store.recent_activity(at(8), at(40), 9).await.expect("recent activity");

    assert_eq!(titles(&activity.segments), ["A", "Backup failed"]);
    assert_eq!(activity.segments[1].kind, SegmentKind::Notification);
    let dialogs: Vec<_> = activity
        .dialogs
        .iter()
        .map(|dialog| (dialog.segment.title.as_str(), dialog.text.as_str()))
        .collect();
    assert_eq!(dialogs, [("Save?", "Save\n[…]\nile?")]);
}

/// Eviction deletes the oldest ended segments whole, and the rest stay readable.
#[rstest]
#[tokio::test]
async fn eviction_deletes_the_oldest_segments_whole(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        dialog("gedit", "first", at(1), lines(10)),
        dialog("gedit", "second", at(2), lines(20)),
        dialog("gedit", "third", at(3), lines(30)),
    ])
    .await;
    let before = all_segments(store).await;

    let evicted = store.evict(1).await.expect("evict");
    assert_eq!(evicted.segments, [before[0].key]);
    assert!(evicted.freed > 0);
    assert_eq!(all_segments(store).await, before[1..]);
    assert!(store.keyspaces().record_ids(before[0].key).is_empty());
    assert!(
        store.keyspaces().stored_contents().iter().all(|entry| entry.key.segment != before[0].key)
    );
    assert_eq!(read(store, before[2].key, Which::Latest).await.root, lines(30));
}

/// A notification is a segment of its own, and eviction treats it as the oldest ended segment.
#[rstest]
#[tokio::test]
async fn eviction_deletes_the_oldest_segments_of_every_kind(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ =
        written(store, [notification("old", at(1)), dialog("gedit", "newer", at(2), lines(10))])
            .await;
    let before = all_segments(store).await;
    assert_eq!(before[0].kind, SegmentKind::Notification);

    let evicted = store.evict(1).await.expect("evict");

    assert_eq!(evicted.segments, [before[0].key]);
    assert_eq!(all_segments(store).await, before[1..]);
}

/// A segment that a run still writes to is kept until the run ends it.
#[rstest]
#[tokio::test]
async fn eviction_keeps_open_segments(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let mut run = RunState::default();
    write_all(store, &mut run, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        dialog("gedit", "Save changes?", at(2), lines(2)),
    ])
    .await;
    let dialog = all_segments(store).await[1].key;

    assert_eq!(store.evict(u64::MAX).await.expect("evict").segments, [dialog]);
    assert_eq!(store.evict(u64::MAX).await.expect("evict"), Evicted::default());
    write_all(store, &mut run, [snapshot(1, at(3), lines(3)), ended(1, at(4))]).await;
    let open = only_segment(store).await;
    assert_eq!((open.snapshots, open.ended), (2, Some(at(4))));

    // The end of the segment gives the eviction something new to remove.
    assert_eq!(store.evict(u64::MAX).await.expect("evict").segments, [open.key]);
    assert!(store.keyspaces().is_empty());
}

/// A segment that an earlier process left without an end is kept until the sweep ends it.
#[rstest]
#[tokio::test]
async fn eviction_keeps_dangling_segments_until_the_sweep(
    #[future(awt)] opened_store: OpenedStore,
) {
    let OpenedStore { dir, store } = opened_store;
    let _ =
        written(&store, [started(1, "firefox", "Log", at(0)), snapshot(1, at(1), lines(1))]).await;
    store.close().await.expect("close");
    let store = store_in(&dir).await;

    assert!(store.evict(u64::MAX).await.expect("evict").segments.is_empty());
    let dangling = only_segment(&store).await.key;
    store.close_open_segments().await.expect("sweep");
    assert_eq!(store.evict(u64::MAX).await.expect("evict").segments, [dangling]);
    assert!(store.keyspaces().is_empty());
}

/// A record that does not decode leaves the store's maintenance and listings running.
///
/// An eviction removes the record, and a sweep ends its segment at the segment's last intact
/// record. A listing leaves the record out.
#[rstest]
#[tokio::test]
async fn maintenance_carries_on_past_a_damaged_record(#[future(awt)] opened_store: OpenedStore) {
    let OpenedStore { dir, store } = opened_store;
    let _ = written(&store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        snapshot(1, at(2), lines(2)),
    ])
    .await;
    let segment = only_segment(&store).await.key;
    let damaged = store.keyspaces().record_ids(segment)[2];
    store.put_raw_record(damaged, b"not a record");
    store.put_raw_record(Uuid::now_v7(), b"not a record either");
    store.close().await.expect("close");

    for _ in 0..2 {
        let store = store_in(&dir).await;
        store.close_open_segments().await.expect("sweep");
        assert_eq!(store.keyspaces().record_ids(segment).len(), 4, "start, snapshots, and end");
        let listed = only_segment(&store).await;
        assert_eq!((listed.key, listed.snapshots), (segment, 1));
        let since = SystemTime::UNIX_EPOCH;
        let activity = store.recent_activity(since, SystemTime::now(), 100).await;
        assert_eq!(activity.expect("recent activity").segments, [listed]);
        store.close().await.expect("close");
    }

    let store = store_in(&dir).await;
    assert_eq!(store.evict(u64::MAX).await.expect("evict").segments, [segment]);
    assert!(store.keyspaces().is_empty());
}

/// An eviction removes a segment whose records are all damaged, with its contents.
#[rstest]
#[tokio::test]
async fn eviction_removes_a_segment_whose_records_are_all_damaged(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        ended(1, at(2)),
    ])
    .await;
    let segment = only_segment(store).await.key;
    for id in store.keyspaces().record_ids(segment) {
        store.put_raw_record(id, b"not a record");
    }

    let evicted = store.evict(u64::MAX).await.expect("evict");

    assert_eq!(evicted.segments, [segment]);
    assert!(store.keyspaces().is_empty());
    assert_eq!(store.counts().await.expect("counts").segments, 0);
}

/// An eviction removes a malformed key of the segment records, and evicts the segments around
/// the key.
#[rstest]
#[tokio::test]
async fn eviction_removes_a_malformed_segment_key(
    #[future(awt)] opened_store: OpenedStore,
    #[values(&[0x01; 4][..], &[0xff; 4][..])] malformed: &[u8],
) {
    let store = &opened_store.store;
    let _ = written(store, [dialog("gedit", "first", at(0), lines(1))]).await;
    store.put_raw_segment_record_key(malformed);
    let _ = written(store, [dialog("gedit", "second", at(1), lines(1))]).await;

    let evicted = store.evict(u64::MAX).await.expect("evict");

    assert_eq!(evicted.segments.len(), 2);
    assert!(store.keyspaces().is_empty());
    assert_eq!(store.evict(u64::MAX).await.expect("evict again"), Evicted::default());
}

/// An active segment whose start failed to store starts with the segment's next snapshot, so
/// that the visit loses only the snapshots before a start that succeeds.
#[rstest]
#[tokio::test]
async fn a_failed_start_is_tried_again_with_the_next_snapshot(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let mut run = RunState::default();
    // A start that failed leaves the segment in the run as unstarted.
    let failed = super::run::Unstarted {
        at: at(0),
        start: super::schema::SegmentStart {
            kind: SegmentKind::Active,
            app: "gedit".to_owned(),
            title: "notes".to_owned(),
            url: None,
        },
    };
    run.add_segment(atuin_desktop_capture::SegmentId(1), super::run::RunSegment::Unstarted(failed));

    write_all(store, &mut run, [snapshot(1, at(1), lines(1)), ended(1, at(2))]).await;

    let segment = only_segment(store).await;
    assert_eq!((segment.title.as_str(), segment.snapshots), ("notes", 1));
    assert_eq!((segment.started, segment.ended), (at(0), Some(at(2))));
}

/// The end of a segment whose start never stored leaves the store unchanged.
#[rstest]
#[tokio::test]
async fn the_end_of_an_unstarted_segment_stores_nothing(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let mut run = RunState::default();
    let failed = super::run::Unstarted {
        at: at(0),
        start: super::schema::SegmentStart {
            kind: SegmentKind::Active,
            app: "gedit".to_owned(),
            title: "notes".to_owned(),
            url: None,
        },
    };
    run.add_segment(atuin_desktop_capture::SegmentId(1), super::run::RunSegment::Unstarted(failed));

    write_all(store, &mut run, [ended(1, at(2))]).await;

    assert!(store.keyspaces().is_empty());
}

/// The estimate grows with every write and shrinks by as much when the writes are evicted,
/// so that a GC does not count the same space twice.
#[rstest]
#[tokio::test]
async fn the_usage_estimate_follows_writes_and_evictions(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let base = store.estimated_disk_space();
    let _ = written(store, [
        started(1, "firefox", "Log", at(0)),
        snapshot(1, at(1), lines(100)),
        snapshot(1, at(2), lines(101)),
        ended(1, at(3)),
        notification("first", at(4)),
    ])
    .await;
    let written = store.estimated_disk_space();
    assert!(written > base + 500, "{written} after {base}");

    let evicted = store.evict(u64::MAX).await.expect("evict");
    assert!(store.keyspaces().is_empty());
    assert_eq!(store.estimated_disk_space(), base);
    assert_eq!(evicted.freed, written - base);
}

/// The count of segments follows writes, deletions, and evictions, and survives a restart.
#[rstest]
#[tokio::test]
async fn counts_follow_the_segments(#[future(awt)] opened_store: OpenedStore) {
    let OpenedStore { dir, store } = opened_store;
    let _ = written(&store, [
        dialog("gedit", "first", at(1), lines(1)),
        background(7, "kate", "notes", at(2), lines(2)),
        dialog("kate", "second", at(3), lines(3)),
        notification("mail", at(4)),
    ])
    .await;
    let segments = async |store: &DesktopStore| store.counts().await.expect("counts").segments;
    assert_eq!(segments(&store).await, 4);
    store.close().await.expect("close");

    let store = store_in(&dir).await;
    assert_eq!(segments(&store).await, 4);
    let kate = SegmentFilter {
        app: Some("kate".to_owned()),
        ..everything()
    };
    assert_eq!(store.delete(kate).await.expect("delete").segments.len(), 2);
    assert_eq!(segments(&store).await, 2);
    store.evict(u64::MAX).await.expect("evict");
    assert_eq!(segments(&store).await, 0);
    store.close().await.expect("close");

    let store = store_in(&dir).await;
    assert_eq!(segments(&store).await, 0);
}

#[rstest]
#[tokio::test]
async fn a_growing_log_page_stays_small(#[future(awt)] opened_store: OpenedStore) {
    const MESSAGES: [&str; 12] = [
        "jo: hi",
        "kim: brb",
        "jo: lunch?",
        "kim: sure",
        "alex: morning all",
        "sam: anyone seen the flaky test in ci?",
        "alex: looking at it now",
        "riley: reminder, the migration deadline is Friday 18:00 UTC",
        "sam: thanks, noted",
        "alex: deploy is green again",
        "riley: pushing the schema change after lunch",
        "sam: ok",
    ];
    let store = &opened_store.store;
    let mut run = RunState::default();
    let _ = store.write(&mut run, started(1, "firefox", "Chat", at(0))).await.expect("write");
    let mut log: Vec<String> = (0..40).map(|k| format!("old: message {k}")).collect();
    for i in 0..25 {
        log.push(format!("{} ({i})", MESSAGES[i % MESSAGES.len()]));
        let root = page(log.iter().cloned());
        let snapshot = snapshot(1, at(i.try_into().unwrap()), root);
        let _ = store.write(&mut run, snapshot).await.expect("write");
    }

    let contents = store.keyspaces().stored_contents();
    assert_eq!(contents.len(), 25);
    let bytes: usize = contents.iter().map(|entry| entry.size).sum();
    assert!(bytes < 6_000, "25 distinct snapshots take {bytes} bytes in contents");
}

#[rstest]
#[case::by_app(Some("gedit"), 0, 1000, &["kate"])]
#[case::by_range(None, 100, 200, &["gedit-early"])]
#[case::by_app_and_range(Some("gedit"), 100, 200, &["gedit-early", "kate"])]
#[tokio::test]
async fn delete_selects_by_app_and_range(
    #[future(awt)] opened_store: OpenedStore,
    #[case] app: Option<&str>,
    #[case] from: u64,
    #[case] to: u64,
    #[case] kept_titles: &[&str],
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "gedit", "gedit-early", at(0)),
        snapshot(1, at(5), lines(1)),
        ended(1, at(10)),
        started(2, "kate", "kate", at(120)),
        snapshot(2, at(125), lines(2)),
        ended(2, at(130)),
        started(3, "gedit", "gedit-late", at(150)),
        snapshot(3, at(155), lines(3)),
        ended(3, at(160)),
    ])
    .await;

    let filter = SegmentFilter {
        from: at(from),
        to: at(to),
        app: app.map(str::to_owned),
        kinds: Kinds::All,
    };
    store.delete(filter).await.expect("delete");

    let kept = all_segments(store).await;
    let mut kept = titles(&kept);
    kept.sort_unstable();
    assert_eq!(kept, kept_titles);
}

#[rstest]
#[tokio::test]
async fn delete_removes_a_segment_that_overlaps_the_range_only_partly(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "gedit", "Log", at(0)),
        snapshot(1, at(10), lines(1)),
        snapshot(1, at(90), lines(2)),
        ended(1, at(100)),
    ])
    .await;

    let filter = SegmentFilter {
        from: at(80),
        to: at(200),
        ..everything()
    };
    let deleted = store.delete(filter).await.expect("delete");

    assert_eq!(deleted.segments.len(), 1);
    assert!(all_segments(store).await.is_empty());
    assert!(store.keyspaces().is_empty());
}

#[rstest]
#[tokio::test]
async fn delete_removes_a_segment_that_an_earlier_process_left_open(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(store, [started(1, "gedit", "Log", at(0)), snapshot(1, at(1), lines(1))]).await;

    let deleted = store.delete(everything()).await.expect("delete");

    assert_eq!(deleted.segments.len(), 1);
    assert!(store.keyspaces().is_empty());
}

#[rstest]
#[tokio::test]
async fn delete_matches_the_app_in_any_case(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        dialog("mail", "Dialog", at(1), lines(1)),
        notification("hello", at(2)),
        notification_from("kate", "other", at(3)),
    ])
    .await;

    let filter = SegmentFilter {
        app: Some("MAIL".to_owned()),
        ..everything()
    };
    let deleted = store.delete(filter).await.expect("delete");

    assert_eq!(deleted.segments.len(), 2);
    assert_eq!(titles(&all_segments(store).await), ["other"]);
}

/// A deletion of every application removes a segment that has a damaged start, snapshot, or
/// end, and a damaged record that belongs to no segment.
#[rstest]
#[tokio::test]
async fn a_delete_of_every_app_removes_damaged_segments_and_records(
    #[future(awt)] opened_store: OpenedStore,
    #[values(0, 1, 2)] damaged_record: usize,
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "gedit", "damaged", at(0)),
        snapshot(1, at(1), lines(1)),
        ended(1, at(2)),
        started(2, "gedit", "intact", at(3)),
        snapshot(2, at(4), lines(2)),
        ended(2, at(5)),
    ])
    .await;
    let damaged = all_segments(store).await[0].key;
    let record = store.keyspaces().record_ids(damaged)[damaged_record];
    store.put_raw_record(record, b"not a record");
    let orphan = Uuid::now_v7();
    store.put_raw_record(orphan, b"not a record either");

    let deleted = store.delete(everything()).await.expect("delete");

    assert_eq!(deleted.segments.len(), 2, "{deleted:?}");
    assert!(deleted.segments.contains(&damaged));
    assert_eq!((deleted.orphans, deleted.undecided), (vec![orphan], vec![]));
    assert!(store.keyspaces().is_empty());
}

/// A deletion of one application removes a segment of the application whose end is damaged.
/// The deletion reports a segment whose start is damaged and a damaged record that belongs to
/// no segment, because the damage hides their application.
#[rstest]
#[tokio::test]
async fn a_delete_of_one_app_reports_what_damage_hides(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "gedit", "start damaged", at(0)),
        ended(1, at(1)),
        started(2, "gedit", "end damaged", at(2)),
        snapshot(2, at(3), lines(1)),
        ended(2, at(4)),
        started(3, "kate", "other", at(5)),
        ended(3, at(6)),
    ])
    .await;
    let [start_damaged, end_damaged, other] =
        <[SegmentSummary; 3]>::try_from(all_segments(store).await)
            .expect("three segments")
            .map(|segment| segment.key);
    store.put_raw_record(start_damaged.0, b"not a record");
    let end = *store.keyspaces().record_ids(end_damaged).last().expect("an end");
    store.put_raw_record(end, b"not a record");
    let orphan = Uuid::now_v7();
    store.put_raw_record(orphan, b"not a record either");
    let filter = SegmentFilter {
        app: Some("gedit".to_owned()),
        ..everything()
    };

    let deleted = store.delete(filter).await.expect("delete");

    assert_eq!(deleted.segments, [end_damaged]);
    assert!(deleted.orphans.is_empty());
    assert_eq!(deleted.undecided, [orphan, start_damaged.0]);
    assert_eq!(all_segments(store).await.iter().map(|s| s.key).collect::<Vec<_>>(), [other]);
}

#[rstest]
#[tokio::test]
async fn a_second_delete_finds_nothing(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [dialog("gedit", "Dialog", at(1), lines(1))]).await;

    let first = store.delete(everything()).await.expect("delete");
    let second = store.delete(everything()).await.expect("delete");

    assert_eq!(first.segments.len(), 1);
    assert_eq!(second, Deleted::default());
}

#[rstest]
#[tokio::test]
async fn rewrite_after_delete_keeps_the_store_usable(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        dialog("gedit", "gone", at(1), lines(10)),
        dialog("kate", "kept", at(2), lines(20)),
    ])
    .await;
    let filter = SegmentFilter {
        app: Some("gedit".to_owned()),
        ..everything()
    };
    store.delete(filter).await.expect("delete");

    store.rewrite().await.expect("rewrite");

    let kept = only_segment(store).await;
    assert_eq!(kept.title, "kept");
    assert_eq!(read(store, kept.key, Which::Latest).await.root, lines(20));
}

#[rstest]
#[tokio::test]
async fn rewrite_after_delete_shrinks_the_tables(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        dialog("gedit", "gone", at(1), noise("gone")),
        dialog("kate", "kept", at(2), noise("kept")),
    ])
    .await;
    store.rewrite().await.expect("rewrite");
    let before = store.keyspaces().contents_table_space();
    let filter = SegmentFilter {
        app: Some("gedit".to_owned()),
        ..everything()
    };
    store.delete(filter).await.expect("delete");

    store.rewrite().await.expect("rewrite");

    let after = store.keyspaces().contents_table_space();
    assert!(after < before * 3 / 4, "{before} bytes before, {after} bytes after");
}

/// An eviction checks that a damaged record is still there before the eviction removes it, so
/// a deletion that removed the record first makes the eviction's commit conflict.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: each commit consumes its transaction"
)]
#[rstest]
#[tokio::test]
async fn a_removal_that_read_a_record_conflicts_with_an_earlier_removal(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(store, [notification("mail", at(1))]).await;
    let keyspaces = store.keyspaces();
    let reader = keyspaces.read();
    let record = reader.records().next().expect("a record").expect("reads");
    drop(reader);

    let mut slow = keyspaces.write().expect("write");
    assert!(slow.has_record(record.id).expect("read"));
    slow.remove_record(&record);
    let mut fast = keyspaces.write().expect("write");
    fast.remove_record(&record);
    fast.commit().expect("the first commit wins");

    assert!(matches!(slow.commit(), Err(StoreError::Conflict)));
}

#[derive(Debug, Clone, Copy)]
enum Part {
    Start,
    Snapshot,
    End,
    Notification,
}

/// Writes a finished segment titled "Log" and a notification titled "hello", and returns the id
/// of `part`.
async fn write_parts(store: &DesktopStore, part: Part) -> Uuid {
    let _ = written(store, [
        started(1, "gedit", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        ended(1, at(2)),
        notification("hello", at(3)),
    ])
    .await;
    let segments = all_segments(store).await;
    let key = segments.iter().find(|segment| segment.title == "Log").expect("the log").key;
    match part {
        Part::Start => key.0,
        Part::Snapshot => store.snapshots(key.0).await.unwrap().unwrap().snapshots[0].record,
        Part::End => *store.keyspaces().record_ids(key).last().expect("an end record"),
        Part::Notification => {
            segments
                .iter()
                .find(|segment| segment.title == "hello")
                .expect("the notification")
                .key
                .0
        }
    }
}

#[rstest]
#[case::start(Part::Start)]
#[case::snapshot(Part::Snapshot)]
#[case::end(Part::End)]
#[case::notification(Part::Notification)]
#[tokio::test]
async fn lookup_names_what_an_id_belongs_to(
    #[future(awt)] opened_store: OpenedStore,
    #[case] part: Part,
) {
    let store = &opened_store.store;
    let id = write_parts(store, part).await;

    let summary = store.lookup(id).await.expect("lookup").expect("a summary");
    let (title, kind) = match part {
        Part::Notification => ("hello", SegmentKind::Notification),
        Part::Start | Part::Snapshot | Part::End => ("Log", SegmentKind::Active),
    };
    assert_eq!((summary.title.as_str(), summary.kind), (title, kind));
}

#[rstest]
#[case::start(Part::Start)]
#[case::snapshot(Part::Snapshot)]
#[case::end(Part::End)]
#[case::notification(Part::Notification)]
#[tokio::test]
async fn segment_of_names_the_key_that_lookup_names(
    #[future(awt)] opened_store: OpenedStore,
    #[case] part: Part,
) {
    let store = &opened_store.store;
    let id = write_parts(store, part).await;

    let key = store.segment_of(id).await.expect("segment of").expect("a key");

    assert_eq!(key, store.lookup(id).await.expect("lookup").expect("a summary").key);
}

#[rstest]
#[tokio::test]
async fn lookup_of_an_unknown_id_is_none(#[future(awt)] opened_store: OpenedStore) {
    assert_eq!(opened_store.store.lookup(Uuid::now_v7()).await.expect("lookup"), None);
    assert_eq!(opened_store.store.segment_of(Uuid::now_v7()).await.expect("segment of"), None);
}

#[rstest]
#[tokio::test]
async fn delete_of_a_segment_keeps_its_neighbours(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "gedit", "a", at(0)),
        snapshot(1, at(1), lines(1)),
        ended(1, at(2)),
        started(2, "gedit", "b", at(3)),
        snapshot(2, at(4), lines(2)),
        ended(2, at(5)),
        started(3, "gedit", "c", at(6)),
        snapshot(3, at(7), lines(3)),
        ended(3, at(8)),
        notification("hello", at(9)),
    ])
    .await;
    let middle = all_segments(store).await[1].key;

    let deleted = store.delete(middle).await.expect("delete");

    assert_eq!(deleted.segments, [middle]);
    assert_eq!(titles(&all_segments(store).await), ["a", "c", "hello"]);
}

#[rstest]
#[tokio::test]
async fn delete_of_a_notification_segment_keeps_the_other_segments(
    #[future(awt)] opened_store: OpenedStore,
) {
    let store = &opened_store.store;
    let _ = written(store, [
        started(1, "gedit", "Log", at(0)),
        snapshot(1, at(1), lines(1)),
        ended(1, at(2)),
        notification("first", at(10)),
        notification("second", at(20)),
        notification("third", at(30)),
    ])
    .await;
    let second = all_segments(store).await[2].key;

    let deleted = store.delete(second).await.expect("delete");

    assert_eq!(deleted.segments, [second]);
    assert_eq!(titles(&all_segments(store).await), ["Log", "first", "third"]);
}

#[rstest]
#[tokio::test]
async fn delete_of_an_open_segment_removes_it(#[future(awt)] opened_store: OpenedStore) {
    let store = &opened_store.store;
    let _ = written(store, [started(1, "gedit", "Log", at(0)), snapshot(1, at(1), lines(1))]).await;
    let key = only_segment(store).await.key;

    let deleted = store.delete(key).await.expect("delete");

    assert_eq!(deleted.segments.len(), 1);
    assert!(store.keyspaces().is_empty());
}

#[rstest]
#[case::segment(Part::Start)]
#[case::notification(Part::Notification)]
#[tokio::test]
async fn deleting_a_segment_twice_finds_nothing_the_second_time(
    #[future(awt)] opened_store: OpenedStore,
    #[case] part: Part,
) {
    let store = &opened_store.store;
    let id = write_parts(store, part).await;
    let key = store.lookup(id).await.unwrap().expect("found").key;

    let first = store.delete(key).await.expect("first delete");
    let second = store.delete(key).await.expect("second delete");

    assert!(!first.is_empty());
    assert_eq!(second, Deleted::default());
}

/// The store holds what capture read from every window, so other users can't open its
/// directory, even when the directory existed before.
#[cfg(unix)]
#[rstest]
#[case::new(None)]
#[case::open_to_everyone(Some(0o755))]
#[tokio::test]
async fn only_the_user_can_open_the_store_directory(dir: TempDir, #[case] existing: Option<u32>) {
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.path().join("store");
    if let Some(mode) = existing {
        std::fs::create_dir(&path).expect("create");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    let store =
        DesktopStore::open(&path, crate::desktop_capture::testing::HOST).await.expect("open");

    let mode = std::fs::metadata(&path).expect("metadata").permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    store.close().await.expect("close");
}

/// How the store's generations meet crashes and failed rewrites.
mod generations {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::desktop_capture::testing::records::window;
    use crate::desktop_capture::testing::{HOST, files_holding};

    const SECRET: &str = "zanzibar quokka 7319";

    fn root(dir: &TempDir) -> PathBuf {
        dir.path().join("store")
    }

    /// The names in the store directory, apart from the lock of this build.
    fn names(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .expect("read the store directory")
            .map(|entry| entry.expect("an entry").file_name().into_string().expect("utf-8"))
            .collect();
        names.sort();
        names
    }

    /// A new store starts in `db-1`, and each deletion moves the store to the next generation.
    /// Writes, evictions, reopens, and a crash before a rewrite's commit keep the records that
    /// the store holds.
    #[rstest]
    #[tokio::test]
    async fn generations_follow_each_other_across_reopens(dir: TempDir) {
        let root = root(&dir);
        let kate = || SegmentFilter {
            app: Some("kate".to_owned()),
            ..everything()
        };
        let store = store_in(&dir).await;
        assert_eq!(names(&root), ["db-1", "store.lock"]);
        store.close().await.expect("close");
        let expected: [&[&str]; 2] = [&["kept 0"], &["evicted 1", "kept 1"]];

        for (round, expected) in (0..2_u64).zip(expected) {
            let store = store_in(&dir).await;
            let time = |n| at(round * 10 + n);
            let _ = written(&store, [
                dialog("gedit", &format!("evicted {round}"), time(1), window("evicted")),
                dialog("kate", SECRET, time(2), window(SECRET)),
                dialog("gedit", &format!("kept {round}"), time(3), window("kept")),
            ])
            .await;
            store.delete(kate()).await.expect("delete");
            store.rewrite_if_owed().await.expect("rewrite");
            store.close().await.expect("close");

            let store = store_in(&dir).await;
            store.evict(1).await.expect("evict");
            let _ = written(&store, [dialog("kate", SECRET, time(4), window(SECRET))]).await;
            store.delete(kate()).await.expect("delete again");
            store.rewrite_if_owed().await.expect("rewrite again");
            store.close().await.expect("close");
            let generation = format!("db-{}", 3 + 2 * round);
            assert_eq!(names(&root), [generation.as_str(), "store.lock"]);
            assert_eq!(files_holding(&root, SECRET), Vec::<PathBuf>::new());

            // A crash between the copy and the rename leaves the copy behind.
            let partial = root.join(format!("db-{}.partial", 4 + 2 * round));
            std::fs::create_dir(&partial).expect("create the partial generation");
            std::fs::write(partial.join("0.jnl"), b"a partial journal").expect("write");

            let store = store_in(&dir).await;
            assert_eq!(names(&root), [generation.as_str(), "store.lock"]);
            assert_eq!(titles(&all_segments(&store).await), expected);
            store.close().await.expect("close");
        }
    }

    /// A deletion notes that the store owes a rewrite, so the next open rewrites a store whose
    /// daemon stopped between the deletion and the rewrite.
    #[rstest]
    #[tokio::test]
    async fn the_next_open_rewrites_a_store_that_stopped_before_its_rewrite(dir: TempDir) {
        let root = root(&dir);
        let store = store_in(&dir).await;
        let _ = written(&store, [
            dialog("kate", SECRET, at(1), window(SECRET)),
            dialog("gedit", "kept", at(2), window("kept")),
        ])
        .await;
        let filter = SegmentFilter {
            app: Some("kate".to_owned()),
            ..everything()
        };
        store.delete(filter).await.expect("delete");
        store.close().await.expect("close");
        assert!(!files_holding(&root, SECRET).is_empty(), "the journal holds the text");

        let store = store_in(&dir).await;

        assert_eq!(files_holding(&root, SECRET), Vec::<PathBuf>::new());
        assert!(!store.keyspaces().read().owes_rewrite().expect("read"));
        assert_eq!(titles(&all_segments(&store).await), ["kept"]);
        store.close().await.expect("close");
    }

    /// A rewrite that fails leaves the rewrite owed, and a later rewrite of the owed store
    /// removes the deleted text.
    #[rstest]
    #[tokio::test]
    async fn a_failed_rewrite_stays_owed_until_a_rewrite_succeeds(dir: TempDir) {
        let root = root(&dir);
        let store = store_in(&dir).await;
        let _ = written(&store, [dialog("kate", SECRET, at(1), window(SECRET))]).await;
        // A file where the rewrite builds its copy makes the rewrite fail.
        let blocker = root.join("db-2.partial");
        std::fs::write(&blocker, b"").expect("block the rewrite");
        store.delete(everything()).await.expect("delete");

        store.rewrite_if_owed().await.expect_err("the rewrite fails");
        assert!(store.keyspaces().read().owes_rewrite().expect("read"));
        assert!(!files_holding(&root, SECRET).is_empty());
        std::fs::remove_file(&blocker).expect("unblock the rewrite");
        store.rewrite_if_owed().await.expect("rewrite");

        assert!(!store.keyspaces().read().owes_rewrite().expect("read"));
        assert_eq!(files_holding(&root, SECRET), Vec::<PathBuf>::new());
        store.close().await.expect("close");
    }

    /// A rewritten store that fails to open at first opens on one of the rewrite's later
    /// attempts.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_rewrite_tries_again_to_open_the_rewritten_store(dir: TempDir) {
        let store = store_in(&dir).await;
        let _ = written(&store, [dialog("gedit", "kept", at(1), window("kept"))]).await;
        store.fail_reopens(2);

        store.rewrite().await.expect("rewrite");

        assert!(store.failure().is_none());
        assert_eq!(titles(&all_segments(&store).await), ["kept"]);
        store.close().await.expect("close");
    }

    /// A rewritten store that keeps failing to open keeps its directory locked and reports its
    /// failure. A later operation opens the store once an attempt is due.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_rewritten_store_that_fails_to_open_reports_the_failure(dir: TempDir) {
        let store = store_in(&dir).await;
        let _ = written(&store, [dialog("gedit", "kept", at(1), window("kept"))]).await;
        store.fail_reopens(1 + u32::try_from(REOPEN_DELAYS.len()).expect("few delays"));

        let rewritten = store.rewrite().await;

        assert!(matches!(rewritten, Err(StoreError::Reopen(_))), "{rewritten:?}");
        assert!(store.failure().is_some());
        let segments = store.segments(everything()).await;
        assert!(matches!(segments, Err(StoreError::Reopen(_))), "{segments:?}");
        let second = DesktopStore::open(root(&dir), HOST).await;
        assert!(matches!(second, Err(StoreError::Locked)), "{second:?}");

        tokio::time::advance(REOPEN_INTERVAL).await;

        assert_eq!(titles(&all_segments(&store).await), ["kept"]);
        assert!(store.failure().is_none());
        store.close().await.expect("close");
    }

    /// A rewrite that can't remove the generation that it replaced reports the failure, and
    /// the store removes the generation at the next attempt.
    #[rstest]
    #[tokio::test]
    async fn a_generation_that_a_rewrite_could_not_remove_stays_owed(dir: TempDir) {
        use std::os::unix::fs::PermissionsExt as _;

        let root = root(&dir);
        let store = store_in(&dir).await;
        let _ = written(&store, [
            dialog("kate", SECRET, at(1), window(SECRET)),
            dialog("gedit", "kept", at(2), window("kept")),
        ])
        .await;
        store.rewrite().await.expect("first rewrite");
        let first = root.join("db-2");
        assert!(first.is_dir());
        let kate = SegmentFilter {
            app: Some("kate".to_owned()),
            ..everything()
        };
        store.delete(kate).await.expect("delete");
        let mode = |mode| std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&first, mode(0o500)).expect("protect the generation");

        let rewritten = store.rewrite().await;

        std::fs::set_permissions(&first, mode(0o700)).expect("unprotect the generation");
        assert!(matches!(rewritten, Err(StoreError::Files(_))), "{rewritten:?}");
        assert!(!files_holding(&first, SECRET).is_empty(), "the old generation holds the text");
        store.rewrite_if_owed().await.expect("remove the old generation");
        let holding = files_holding(&root, SECRET);
        assert_eq!(titles(&all_segments(&store).await), ["kept"]);
        store.close().await.expect("close");
        assert_eq!(holding, Vec::<PathBuf>::new(), "{:?}", names(&root));
    }

    /// A removal notes that the search index owes the removal, and a rewrite carries the note
    /// on.
    #[rstest]
    #[tokio::test]
    async fn a_removal_notes_the_index_removal_through_a_rewrite(dir: TempDir) {
        let store = store_in(&dir).await;
        let _ = written(&store, [dialog("kate", SECRET, at(1), window(SECRET))]).await;
        assert!(!store.owes_index_removal().await.expect("read the note"));

        store.delete(everything()).await.expect("delete");

        assert!(store.owes_index_removal().await.expect("read the note"));
        store.close().await.expect("close");
        let store = store_in(&dir).await;
        assert!(store.owes_index_removal().await.expect("read the note"));
        store.settle_index_removal().await.expect("settle");
        assert!(!store.owes_index_removal().await.expect("read the note"));
        store.close().await.expect("close");
    }

    /// Evictions count the bytes that they free across restarts, until a rewrite.
    #[rstest]
    #[tokio::test]
    async fn evicted_bytes_count_until_a_rewrite(dir: TempDir) {
        let store = store_in(&dir).await;
        let _ = written(&store, [
            dialog("kate", "first", at(1), window("first")),
            dialog("kate", "second", at(2), window("second")),
        ])
        .await;
        let freed = store.evict(1).await.expect("evict").freed;
        assert!(freed > 0);
        assert_eq!(store.evicted_since_rewrite().await.expect("read"), freed);
        store.close().await.expect("close");

        let store = store_in(&dir).await;
        let more = store.evict(1).await.expect("evict").freed;
        assert_eq!(store.evicted_since_rewrite().await.expect("read"), freed + more);
        assert!(!store.keyspaces().read().owes_rewrite().expect("read"), "evictions owe nothing");
        store.rewrite().await.expect("rewrite");
        assert_eq!(store.evicted_since_rewrite().await.expect("read"), 0);
        store.close().await.expect("close");
    }
}
