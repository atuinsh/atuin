use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atuin_common::time::SystemTimeExt as _;
use atuin_desktop_capture::Record;
use rstest::{fixture, rstest};
use tonic::{Code, Request, Response};

use super::pb::{
    DeleteEverything, DeleteFilter, DeleteRequest, ListSegmentsRequest, ListSnapshotsRequest,
    LookupRequest, ReadSegmentRequest, ReadSnapshotsRequest, RecentActivityRequest, SearchRequest,
    Segment, SegmentKind as WireKind, State, StatusRequest, TallySegmentsRequest, Which,
    delete_request, search_limit,
};
use super::*;
use crate::desktop_capture::store::{RunState, SegmentFilter, SegmentKind as StoredKind};
use crate::desktop_capture::testing::records::{
    background, dialog, ended, everything, notification, snapshot, started, window, write_all,
    written,
};
use crate::desktop_capture::testing::{
    FakeSpawner, HOST, HeldOpen, Home, Script, eventually, home, paused_home, state_becomes,
};
use crate::desktop_capture::{
    CaptureState, DeleteError, DesktopCapture, RequestError, StopFileError, StoreNotOpen,
};

const UNKNOWN: &str = "0190a5c4-0000-7000-8000-000000000000";

fn delete_matching(filter: DeleteFilter) -> DeleteRequest {
    DeleteRequest {
        selection: Some(delete_request::Selection::Filter(filter)),
    }
}

fn delete_id(id: impl Into<String>) -> DeleteRequest {
    DeleteRequest {
        selection: Some(delete_request::Selection::Id(id.into())),
    }
}

fn delete_everything() -> DeleteRequest {
    DeleteRequest {
        selection: Some(delete_request::Selection::Everything(DeleteEverything {})),
    }
}

/// A service over a capture whose fake helper is connected.
async fn running(home: &Home) -> Service {
    let spawner = FakeSpawner::new([Script::Serve]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::Running).await;
    Service::new(Arc::new(capture))
}

/// A service over a capture that has no store.
#[fixture]
fn without_store() -> Service {
    Service::new(Arc::new(DesktopCapture::nop()))
}

/// Stores `records` as one run of the helper.
async fn store(service: &Service, records: impl IntoIterator<Item = Record>) {
    let store = service.capture.store().expect("the store is open");
    write_all(store, &mut RunState::default(), records).await;
}

/// The id of the only segment that the store holds, apart from notifications.
async fn only_segment_id(service: &Service) -> String {
    let store = service.capture.store().expect("the store is open");
    let mut segments = store.segments(everything()).await.expect("segments");
    segments.retain(|segment| segment.kind != StoredKind::Notification);
    assert_eq!(segments.len(), 1, "{segments:?}");
    segments[0].key.0.to_string()
}

fn ago(secs: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(secs)
}

fn titles(segments: &[Segment]) -> Vec<&str> {
    segments.iter().map(|segment| segment.title.as_str()).collect()
}

#[rstest]
#[tokio::test]
async fn recent_activity_lists_only_active_segments_since(home: Home) {
    let service = running(&home).await;
    store(&service, [
        started(1, "gedit", "earlier", ago(3600)),
        ended(1, ago(3000)),
        started(3, "gedit", "spanning", ago(900)),
        ended(3, ago(300)),
        started(2, "gedit", "now", ago(60)),
        snapshot(2, ago(50), window("draft")),
        dialog("gedit", "Save changes?", ago(30), window("Save before closing?")),
        dialog("nautilus", "", ago(25), window("Error while copying.")),
        background(7, "kate", "behind", ago(20), window("log")),
    ])
    .await;

    let request = RecentActivityRequest {
        since_ms: ago(600).saturating_unix_millis(),
    };
    let reply = service.recent_activity(Request::new(request)).await.expect("reply");
    let reply = reply.into_inner();

    assert_eq!(titles(&reply.segments), ["spanning", "now"]);
    let dialogs: Vec<_> = reply
        .dialogs
        .iter()
        .map(|dialog| {
            let title = &dialog.segment.as_ref().expect("the segment").title;
            (title.as_str(), dialog.text.as_str())
        })
        .collect();
    assert_eq!(dialogs, [("Save changes?", "Save before closing?"), ("", "Error while copying."),]);
}

#[rstest]
#[tokio::test]
async fn recent_activity_reports_capture_running(home: Home) {
    let service = running(&home).await;
    let capture_running = async || {
        let request = RecentActivityRequest { since_ms: 0 };
        let reply = service.recent_activity(Request::new(request)).await.expect("reply");
        reply.into_inner().capture_running
    };
    assert!(capture_running().await);

    service.capture.stop().await.expect("stop");
    assert_eq!(service.capture.state(), CaptureState::Stopped);
    assert!(!capture_running().await);
}

#[rstest]
#[tokio::test]
async fn read_segment_renders_the_chosen_snapshot(home: Home) {
    let service = running(&home).await;
    store(&service, [
        started(1, "firefox", "docs", ago(5)),
        snapshot(1, ago(4), window("first")),
        snapshot(1, ago(3), window("abcdefghij")),
    ])
    .await;
    let id = only_segment_id(&service).await;

    let read = async |which: Which, max_bytes| {
        let request = ReadSegmentRequest {
            id: id.clone(),
            which: which.into(),
            max_bytes,
            ..Default::default()
        };
        service.read_segment(Request::new(request)).await.expect("read").into_inner()
    };
    let first = read(Which::First, 0).await;
    assert_eq!(first.text, "first");
    assert_eq!(first.segment.expect("the segment").title, "docs");
    assert_eq!(read(Which::Latest, 0).await.text, "abcdefghij");
    let cut = read(Which::Latest, 6).await;
    assert_eq!(cut.text, "abc\n[…]\nhij");
    assert!(cut.truncated);
    for whole in [read(Which::Latest, 0).await, read(Which::Latest, 10).await] {
        assert!(!whole.truncated, "{whole:?}");
    }
}

#[rstest]
#[tokio::test]
async fn list_snapshots_names_each_snapshot_for_a_read(home: Home) {
    let service = running(&home).await;
    store(&service, [
        started(1, "firefox", "docs", ago(5)),
        snapshot(1, ago(4), window("first")),
        snapshot(1, ago(3), window("first")),
        snapshot(1, ago(2), window("second")),
    ])
    .await;
    let id = only_segment_id(&service).await;

    let request = ListSnapshotsRequest { id: id.clone() };
    let listed = service.list_snapshots(Request::new(request)).await.expect("list");
    let listed = listed.into_inner();

    assert_eq!(listed.segment.as_ref().expect("the segment").title, "docs");
    let repeats: Vec<_> = listed.snapshots.iter().map(|s| s.same_as_previous).collect();
    assert_eq!(repeats, [false, true, false]);
    assert!(listed.snapshots.iter().all(|s| s.trigger == "polled"), "{listed:?}");
    let last = listed.snapshots.last().expect("a snapshot");
    let request = ReadSegmentRequest {
        id,
        which: Which::Record.into(),
        record_id: last.record_id.clone(),
        ..Default::default()
    };
    let read = service.read_segment(Request::new(request)).await.expect("read").into_inner();
    assert_eq!((read.text.as_str(), read.at_ms), ("second", last.at_ms));
}

/// A snapshot's id lists the snapshots of its segment, as the id of the segment does.
#[rstest]
#[tokio::test]
async fn list_snapshots_takes_the_id_of_a_snapshot(home: Home) {
    let service = running(&home).await;
    store(&service, [
        started(1, "firefox", "docs", ago(5)),
        snapshot(1, ago(4), window("first")),
        snapshot(1, ago(3), window("second")),
    ])
    .await;
    let list = async |id: String| {
        let request = ListSnapshotsRequest { id };
        service.list_snapshots(Request::new(request)).await.expect("list").into_inner()
    };
    let by_segment = list(only_segment_id(&service).await).await;

    let by_snapshot = list(by_segment.snapshots[1].record_id.clone()).await;

    assert_eq!(by_snapshot, by_segment);
}

#[rstest]
#[case::malformed_id("segment", Code::InvalidArgument)]
#[case::unknown_segment(UNKNOWN, Code::NotFound)]
#[tokio::test]
async fn list_snapshots_refuses(home: Home, #[case] id: &str, #[case] code: Code) {
    let service = running(&home).await;
    let request = ListSnapshotsRequest { id: id.to_owned() };
    let status = service.list_snapshots(Request::new(request)).await.expect_err("refused");
    assert_eq!(status.code(), code, "{status:?}");
}

#[rstest]
#[tokio::test]
async fn list_segments_filters_by_kind_and_limit_up_to_now(home: Home) {
    let service = running(&home).await;
    store(&service, [
        started(1, "gedit", "notes", ago(60)),
        dialog("gedit", "Save changes?", ago(30), window("save")),
        background(7, "kate", "behind", ago(20), window("log")),
    ])
    .await;
    let list = async |kinds: Vec<i32>, limit: Option<u32>| {
        let request = ListSegmentsRequest {
            from_ms: ago(600).saturating_unix_millis(),
            kinds,
            limit,
            ..Default::default()
        };
        service.list_segments(Request::new(request)).await.map(Response::into_inner)
    };

    let wanted = vec![WireKind::Dialog.into(), WireKind::Background.into()];
    let listed = list(wanted, None).await.expect("list");
    assert_eq!(titles(&listed.segments), ["Save changes?", "behind"]);
    assert_eq!(titles(&list(Vec::new(), None).await.expect("list").segments), [
        "notes",
        "Save changes?",
        "behind"
    ]);
    let newest = list(Vec::new(), Some(2)).await.expect("list");
    assert_eq!(titles(&newest.segments), ["Save changes?", "behind"]);
    for kinds in [vec![99], vec![WireKind::Unspecified.into()]] {
        let refused = list(kinds, None).await.expect_err("refused");
        assert_eq!(refused.code(), Code::InvalidArgument, "{refused:?}");
    }
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_removes_one_app_and_keeps_the_rest(home: Home) {
    let service = running(&home).await;
    store(&service, [started(1, "gedit", "notes", ago(60)), started(2, "kate", "code", ago(50))])
        .await;

    let request = delete_matching(DeleteFilter {
        app: Some("GEDIT".to_owned()),
        ..Default::default()
    });
    let reply = service.delete(Request::new(request)).await.expect("delete").into_inner();
    assert_eq!((reply.segments, reply.undecided), (1, 0));

    let request = ListSegmentsRequest::default();
    let listed = service.list_segments(Request::new(request)).await.expect("list");
    assert_eq!(titles(&listed.into_inner().segments), ["code"]);
    service.capture.shutdown().await;
}

/// A deletion of one application counts the damaged record that it leaves in the store,
/// because the damage hides the record's application.
///
/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_reports_the_damage_that_it_leaves(home: Home) {
    let service = running(&home).await;
    store(&service, [started(1, "gedit", "notes", ago(60))]).await;
    let store = service.capture.store().expect("the store is open");
    store.put_raw_record(uuid::Uuid::now_v7(), b"not a record");

    let request = delete_matching(DeleteFilter {
        app: Some("gedit".to_owned()),
        ..Default::default()
    });
    let reply = service.delete(Request::new(request)).await.expect("delete").into_inner();

    assert_eq!((reply.segments, reply.undecided), (1, 1));
    let listed = service.list_segments(Request::new(ListSegmentsRequest::default())).await;
    assert!(listed.expect("list").into_inner().segments.is_empty());
    service.capture.shutdown().await;
}

/// A tally counts the damaged records that a deletion with the same range and application removes.
///
/// A deletion of everything removes damaged records, and a deletion by application leaves them.
#[rstest]
#[case::every_app(None, 1)]
#[case::one_app(Some("gedit"), 0)]
#[tokio::test]
async fn tally_counts_the_damage_that_a_deletion_removes(
    home: Home,
    #[case] app: Option<&str>,
    #[case] damaged: u64,
) {
    let service = running(&home).await;
    let store = service.capture.store().expect("the store is open");
    store.put_raw_record(uuid::Uuid::now_v7(), b"not a record");
    let request = TallySegmentsRequest {
        app: app.map(str::to_owned),
        ..Default::default()
    };

    let reply = service.tally_segments(Request::new(request)).await.expect("tally").into_inner();

    assert_eq!(reply.damaged, damaged);
    assert!(reply.apps.is_empty());
    assert_eq!(reply.from_ms, None);
    service.capture.shutdown().await;
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_without_an_upper_end_removes_what_starts_after_the_request(
    home: Home,
    #[values(
        delete_everything(),
        delete_matching(DeleteFilter { from_ms: 1, ..Default::default() })
    )]
    request: DeleteRequest,
) {
    let service = running(&home).await;
    let later = SystemTime::now() + Duration::from_secs(3600);
    store(&service, [
        started(1, "gedit", "notes", ago(60)),
        started(2, "gedit", "later", later),
        notification("later mail", later),
    ])
    .await;

    let reply = service.delete(Request::new(request)).await.expect("delete").into_inner();

    assert_eq!(reply.segments, 3);
    let store = service.capture.store().expect("the store is open");
    let up_to_later = SegmentFilter {
        to: later,
        ..everything()
    };
    assert!(store.segments(up_to_later).await.expect("segments").is_empty());
    service.capture.shutdown().await;
}

/// Which of the stored items a test names by id.
#[derive(Debug, Clone, Copy)]
enum IdOf {
    Segment,
    Snapshot,
    Notification,
}

/// The ids of what `store_named_items` stored.
struct Ids {
    segment: String,
    snapshot: String,
    notification: String,
}

impl Ids {
    fn of(&self, id_of: IdOf) -> String {
        match id_of {
            IdOf::Segment => self.segment.clone(),
            IdOf::Snapshot => self.snapshot.clone(),
            IdOf::Notification => self.notification.clone(),
        }
    }
}

/// Stores segments "notes" and "code", each with a snapshot, and notifications "a" and "b".
async fn store_named_items(service: &Service) -> Ids {
    store(service, [
        started(1, "gedit", "notes", ago(60)),
        snapshot(1, ago(59), window("draft")),
        started(2, "kate", "code", ago(50)),
        snapshot(2, ago(49), window("fn main")),
        notification("a", ago(40)),
        notification("b", ago(30)),
    ])
    .await;
    let store = service.capture.store().expect("the store is open");
    let notes = store
        .segments(everything())
        .await
        .expect("segments")
        .into_iter()
        .find(|segment| segment.title == "notes")
        .expect("notes");
    let snapshots = store.snapshots(notes.key.0).await.expect("snapshots").expect("listed");
    let a = store
        .segments(everything())
        .await
        .expect("segments")
        .into_iter()
        .find(|segment| segment.title == "a")
        .expect("a");
    Ids {
        segment: notes.key.0.to_string(),
        snapshot: snapshots.snapshots[0].record.to_string(),
        notification: a.key.0.to_string(),
    }
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[case::segment(IdOf::Segment)]
#[case::snapshot(IdOf::Snapshot)]
#[case::notification(IdOf::Notification)]
#[tokio::test]
async fn delete_by_id_removes_what_the_id_names(home: Home, #[case] id_of: IdOf) {
    let service = running(&home).await;
    let ids = store_named_items(&service).await;

    let request = delete_id(ids.of(id_of));
    let reply = service.delete(Request::new(request)).await.expect("delete").into_inner();

    assert_eq!(reply.segments, 1);
    let listed = service.list_segments(Request::new(ListSegmentsRequest::default())).await;
    let kept_segments = match id_of {
        IdOf::Notification => vec!["notes", "code", "b"],
        IdOf::Segment | IdOf::Snapshot => vec!["code", "a", "b"],
    };
    let mut titles_left = titles(&listed.expect("list").into_inner().segments)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    titles_left.sort();
    let mut kept_segments: Vec<_> = kept_segments.into_iter().map(str::to_owned).collect();
    kept_segments.sort();
    assert_eq!(titles_left, kept_segments);
    service.capture.shutdown().await;
}

/// A deletion that doesn't say what it deletes is refused, and the store keeps everything.
///
/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[case::no_selection(DeleteRequest { selection: None })]
#[case::an_empty_filter(delete_matching(DeleteFilter::default()))]
#[case::a_malformed_id(delete_id("notes"))]
#[tokio::test]
async fn a_deletion_without_a_selection_is_refused(home: Home, #[case] request: DeleteRequest) {
    let service = running(&home).await;
    let _ = store_named_items(&service).await;

    let refused = service.delete(Request::new(request)).await.expect_err("refused");

    assert_eq!(refused.code(), Code::InvalidArgument, "{refused:?}");
    let listed = service.list_segments(Request::new(ListSegmentsRequest::default())).await;
    assert_eq!(listed.expect("list").into_inner().segments.len(), 4);
    service.capture.shutdown().await;
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn delete_by_an_unknown_id_is_not_found(home: Home) {
    let service = running(&home).await;
    let refused = service.delete(Request::new(delete_id(UNKNOWN))).await.expect_err("refused");
    assert_eq!(refused.code(), Code::NotFound);
    assert_eq!(refused.message(), format!("there is no segment or snapshot {UNKNOWN}"));
    service.capture.shutdown().await;
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[case::segment(IdOf::Segment)]
#[case::snapshot(IdOf::Snapshot)]
#[case::notification(IdOf::Notification)]
#[tokio::test]
async fn lookup_answers_what_an_id_names(home: Home, #[case] id_of: IdOf) {
    let service = running(&home).await;
    let ids = store_named_items(&service).await;

    let request = LookupRequest { id: ids.of(id_of) };
    let reply = service.lookup(Request::new(request)).await.expect("lookup").into_inner();

    let Some(segment) = reply.segment else {
        panic!("{id_of:?} was not answered with a segment");
    };
    let (title, id) = match id_of {
        IdOf::Segment | IdOf::Snapshot => ("notes", &ids.segment),
        IdOf::Notification => ("a", &ids.notification),
    };
    assert_eq!((segment.title.as_str(), &segment.id), (title, id));
    service.capture.shutdown().await;
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn lookup_of_an_unknown_id_is_not_found(home: Home) {
    let service = running(&home).await;
    let request = LookupRequest {
        id: UNKNOWN.to_owned(),
    };
    let refused = service.lookup(Request::new(request)).await.expect_err("refused");
    assert_eq!(refused.code(), Code::NotFound);
    service.capture.shutdown().await;
}

#[rstest]
#[tokio::test]
async fn delete_without_a_store_is_unavailable(without_store: Service) {
    let request = delete_everything();
    let refused = without_store.delete(Request::new(request)).await.expect_err("refused");
    assert_eq!(refused.code(), Code::Unavailable);
}

/// A hit names the snapshot that matched, which a read returns even after the window
/// changed.
///
/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn search_hits_read_back_as_the_snapshot_that_matched(home: Home) {
    let spawner = FakeSpawner::new([Script::Serve]);
    let service = Service::new(Arc::new(home.open(&home.settings(&[]), &spawner).await));
    let store = service.capture.store().expect("store");
    let matched_at = ago(3);
    let items = written(store, [
        started(1, "gedit", "notes", ago(5)),
        snapshot(1, ago(4), window("quokka")),
        snapshot(1, matched_at, window("quokka")),
        snapshot(1, ago(2), window("wallaby")),
    ])
    .await;
    service.capture.index().expect("index").add(items).await.expect("index");

    let request = SearchRequest {
        query: "quokka".to_owned(),
        ..SearchRequest::default()
    };
    let reply = service.search(Request::new(request)).await.expect("search").into_inner();
    assert_eq!(reply.hits.len(), 1, "{:?}", reply.hits);
    let hit = &reply.hits[0];
    assert_eq!(hit.kind(), WireKind::Active);
    assert_eq!(hit.at_ms, matched_at.saturating_unix_millis(), "the latest snapshot that matched");

    let read = async |which: Which, record_id: &str| {
        let request = ReadSegmentRequest {
            id: hit.segment.clone(),
            which: which.into(),
            record_id: record_id.to_owned(),
            ..Default::default()
        };
        service.read_segment(Request::new(request)).await.expect("read").into_inner()
    };
    let matched = read(Which::Record, &hit.record_id).await;
    assert_eq!(matched.text, "quokka");
    assert_eq!((matched.record_id.as_str(), matched.at_ms), (hit.record_id.as_str(), hit.at_ms));
    let latest = read(Which::Latest, "").await;
    assert_eq!(latest.text, "wallaby");
    assert_ne!(latest.record_id, hit.record_id);

    let by_snapshot = ReadSegmentRequest {
        id: hit.record_id.clone(),
        ..ReadSegmentRequest::default()
    };
    let by_snapshot = service.read_segment(Request::new(by_snapshot)).await.expect("read");
    assert_eq!(by_snapshot.into_inner(), matched);
}

/// A read by a snapshot's id with a choice of snapshot chooses among the snapshots of the
/// snapshot's segment.
#[rstest]
#[case::first(Which::First, "one")]
#[case::latest(Which::Latest, "two")]
#[case::record(Which::Record, "two")]
#[case::unspecified(Which::Unspecified, "one")]
#[tokio::test]
async fn a_snapshot_id_chooses_from_its_segment(
    home: Home,
    #[case] which: Which,
    #[case] expected: &str,
) {
    let service = running(&home).await;
    store(&service, [
        started(1, "gedit", "notes", ago(5)),
        snapshot(1, ago(4), window("one")),
        snapshot(1, ago(3), window("two")),
    ])
    .await;
    let store = service.capture.store().expect("store");
    let segment = store.segments(everything()).await.expect("segments")[0].key;
    let snapshots = store.snapshots(segment.0).await.expect("snapshots").expect("a segment");
    let request = ReadSegmentRequest {
        id: snapshots.snapshots[0].record.to_string(),
        which: which.into(),
        record_id: snapshots.snapshots[1].record.to_string(),
        ..Default::default()
    };

    let read = service.read_segment(Request::new(request)).await.expect("read").into_inner();

    assert_eq!(read.text, expected);
}

/// Which id names the segment of a read of several snapshots.
#[derive(Debug, Clone, Copy)]
enum NamedBy {
    Segment,
    Snapshot,
}

/// A read of several snapshots returns their texts in the segment's order, whichever id names
/// the segment.
#[rstest]
#[tokio::test]
async fn read_snapshots_returns_the_texts_in_order(
    home: Home,
    #[values(NamedBy::Segment, NamedBy::Snapshot)] named_by: NamedBy,
) {
    let service = running(&home).await;
    store(&service, [
        started(1, "gedit", "notes", ago(5)),
        snapshot(1, ago(4), window("one")),
        snapshot(1, ago(3), window("two")),
        snapshot(1, ago(2), window("three")),
    ])
    .await;
    let segment = only_segment_id(&service).await;
    let request = ListSnapshotsRequest {
        id: segment.clone(),
    };
    let listed = service.list_snapshots(Request::new(request)).await.expect("list").into_inner();
    let records: Vec<_> = listed.snapshots.iter().map(|s| s.record_id.clone()).collect();
    let id = match named_by {
        NamedBy::Segment => segment,
        NamedBy::Snapshot => records[1].clone(),
    };
    let request = ReadSnapshotsRequest {
        id,
        record_ids: vec![records[2].clone(), records[0].clone()],
        max_bytes: 4,
        max_total_bytes: 0,
    };

    let read = service.read_snapshots(Request::new(request)).await.expect("read").into_inner();

    assert_eq!(read.left_out, 0);
    let texts: Vec<_> = read
        .texts
        .iter()
        .map(|text| (&text.record_id, text.text.as_str(), text.truncated))
        .collect();
    assert_eq!(texts, [(&records[0], "one", false), (&records[2], "th\n[…]\nee", true)]);
}

/// A read of several snapshots keeps its texts within `max_total_bytes`, and counts the
/// snapshots that it leaves out.
#[rstest]
#[tokio::test]
async fn read_snapshots_keeps_within_the_total(home: Home) {
    let service = running(&home).await;
    store(&service, [
        started(1, "gedit", "notes", ago(5)),
        snapshot(1, ago(4), window("one")),
        snapshot(1, ago(3), window("two")),
        snapshot(1, ago(2), window("three")),
    ])
    .await;
    let segment = only_segment_id(&service).await;
    let request = ListSnapshotsRequest {
        id: segment.clone(),
    };
    let listed = service.list_snapshots(Request::new(request)).await.expect("list").into_inner();
    let request = ReadSnapshotsRequest {
        id: segment,
        record_ids: listed.snapshots.iter().map(|s| s.record_id.clone()).collect(),
        max_bytes: 0,
        max_total_bytes: 6,
    };

    let read = service.read_snapshots(Request::new(request)).await.expect("read").into_inner();

    let texts: Vec<_> = read.texts.iter().map(|text| text.text.as_str()).collect();
    assert_eq!(texts, ["one", "two"]);
    assert_eq!(read.left_out, 1);
}

/// What a refused read of several snapshots names.
#[derive(Debug, Clone, Copy)]
enum Refused {
    UnknownSegment,
    UnknownRecord,
    MalformedRecord,
    MalformedId,
    TooManyRecords,
}

#[rstest]
#[case::unknown_segment(Refused::UnknownSegment, Code::NotFound)]
#[case::unknown_record(Refused::UnknownRecord, Code::NotFound)]
#[case::malformed_record(Refused::MalformedRecord, Code::InvalidArgument)]
#[case::malformed_id(Refused::MalformedId, Code::InvalidArgument)]
#[case::too_many_records(Refused::TooManyRecords, Code::InvalidArgument)]
#[tokio::test]
async fn read_snapshots_refuses(home: Home, #[case] refused: Refused, #[case] code: Code) {
    let service = running(&home).await;
    store(&service, [started(1, "gedit", "notes", ago(5)), snapshot(1, ago(4), window("one"))])
        .await;
    let segment = only_segment_id(&service).await;
    let listed = ListSnapshotsRequest {
        id: segment.clone(),
    };
    let listed = service.list_snapshots(Request::new(listed)).await.expect("list").into_inner();
    let known = listed.snapshots[0].record_id.as_str();
    let (id, records) = match refused {
        Refused::UnknownSegment => (UNKNOWN.to_owned(), vec![]),
        Refused::UnknownRecord => (segment, vec![UNKNOWN]),
        Refused::MalformedRecord => (segment, vec!["record"]),
        Refused::MalformedId => ("segment".to_owned(), vec![]),
        Refused::TooManyRecords => (segment, vec![known; pb::MAX_SNAPSHOT_RECORDS + 1]),
    };
    let request = ReadSnapshotsRequest {
        id,
        record_ids: records.into_iter().map(str::to_owned).collect(),
        max_bytes: 0,
        max_total_bytes: 0,
    };

    let status = service.read_snapshots(Request::new(request)).await.expect_err("refused");

    assert_eq!(status.code(), code, "{status:?}");
}

/// A tally counts each application's windows and notifications, and ends an open segment at
/// the end of the range.
#[rstest]
#[tokio::test]
async fn tally_counts_per_application(home: Home) {
    let service = running(&home).await;
    // The request carries milliseconds, so the range ends at a whole millisecond.
    let to =
        SystemTime::checked_from_unix_millis(ago(10).saturating_unix_millis()).expect("a time");
    let first = ago(60);
    store(&service, [
        started(1, "gedit", "notes", first),
        snapshot(1, ago(50), window("one")),
        snapshot(1, ago(40), window("two")),
        ended(1, ago(30)),
        started(2, "kate", "code", to - Duration::from_secs(10)),
        notification("mail", ago(15)),
        started(3, "gedit", "later", ago(5)),
    ])
    .await;
    let request = TallySegmentsRequest {
        from_ms: ago(100).saturating_unix_millis(),
        to_ms: to.saturating_unix_millis(),
        app: None,
    };

    let reply = service.tally_segments(Request::new(request)).await.expect("tally").into_inner();

    let apps: Vec<_> = reply
        .apps
        .iter()
        .map(|app| (app.app.as_str(), app.windows, app.notifications, app.snapshots))
        .collect();
    assert_eq!(apps, [("gedit", 1, 0, 2), ("kate", 1, 0, 0), ("mail", 0, 1, 0)]);
    let duration_secs = |name: &str| {
        let app = reply.apps.iter().find(|app| app.app == name).expect(name);
        app.duration_ms / 1000
    };
    assert_eq!((duration_secs("gedit"), duration_secs("kate")), (30, 10));
    assert_eq!(reply.from_ms, Some(first.saturating_unix_millis()));
    assert_eq!(reply.to_ms, Some(to.saturating_unix_millis()));
}

#[rstest]
#[tokio::test]
async fn an_empty_tally_has_no_span(home: Home) {
    let service = running(&home).await;
    let request = TallySegmentsRequest {
        app: Some("gedit".to_owned()),
        ..TallySegmentsRequest::default()
    };

    let reply = service.tally_segments(Request::new(request)).await.expect("tally").into_inner();

    assert!(reply.apps.is_empty());
    assert_eq!((reply.from_ms, reply.to_ms), (None, None));
}

/// A search keeps the segments that overlap its time range and that pass its application
/// and kinds.
///
/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[case::everything(None, vec![], 0, 0, &["early", "late", "Save?"])]
#[case::overlapping_from(None, vec![], 40, 0, &["late", "Save?"])]
#[case::overlapping_to(None, vec![], 0, 40, &["early"])]
#[case::app(Some("kate"), vec![], 0, 0, &["late"])]
#[case::app_in_any_case(Some("KATE"), vec![], 0, 0, &["late"])]
#[case::kinds(None, vec![WireKind::Dialog], 0, 0, &["Save?"])]
#[tokio::test]
async fn search_filters_its_hits(
    home: Home,
    #[case] app: Option<&str>,
    #[case] kinds: Vec<WireKind>,
    #[case] from_secs_ago: u64,
    #[case] to_secs_ago: u64,
    #[case] expected: &[&str],
) {
    let service = running(&home).await;
    let store = service.capture.store().expect("store");
    let records = [
        started(1, "gedit", "early", ago(60)),
        snapshot(1, ago(55), window("needle")),
        ended(1, ago(50)),
        started(2, "kate", "late", ago(30)),
        snapshot(2, ago(25), window("needle")),
        dialog("gedit", "Save?", ago(20), window("needle")),
    ];
    let items = written(store, records).await;
    service.capture.index().expect("index").add(items).await.expect("index");
    let millis_ago = |secs| match secs {
        0 => 0,
        secs => ago(secs).saturating_unix_millis(),
    };
    let request = SearchRequest {
        query: "needle".to_owned(),
        app: app.map(str::to_owned),
        kinds: kinds.into_iter().map(Into::into).collect(),
        from_ms: millis_ago(from_secs_ago),
        to_ms: millis_ago(to_secs_ago),
        ..SearchRequest::default()
    };

    let reply = service.search(Request::new(request)).await.expect("search").into_inner();

    let mut titles: Vec<_> = reply.hits.iter().map(|hit| hit.title.as_str()).collect();
    titles.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(titles, expected);
}

#[rstest]
#[case::unset(0, 20)]
#[case::within(7, 7)]
#[case::above_the_cap(1000, 100)]
fn search_limits(#[case] limit: u32, #[case] expected: u32) {
    assert_eq!(search_limit(limit), expected);
}

#[rstest]
#[tokio::test]
async fn status_counts(home: Home) {
    let service = running(&home).await;
    store(&service, [
        started(1, "gedit", "notes", ago(60)),
        snapshot(1, ago(50), window("one")),
        snapshot(1, ago(40), window("two")),
        ended(1, ago(30)),
        dialog("gedit", "Save changes?", ago(20), window("save")),
        notification("mail", ago(10)),
    ])
    .await;

    let reply = service.status(Request::new(StatusRequest {})).await.expect("status");
    let reply = reply.into_inner();

    assert_eq!(reply.state(), State::Running);
    assert_eq!(reply.segments, Some(3));
    // The segment's start, two snapshots, and end, and three for each of the dialog and the
    // notification.
    assert_eq!(reply.records, Some(10));
}

/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn search_hits_name_their_segment_and_quote_the_text(home: Home) {
    let spawner = FakeSpawner::new([Script::Segment]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    let service = Service::new(Arc::new(capture));

    let search = async || {
        let request = SearchRequest {
            query: "ZEBRA".to_owned(),
            ..SearchRequest::default()
        };
        let reply = service.search(Request::new(request)).await.expect("search");
        reply.into_inner().hits
    };
    let hits = eventually(search, |hits| !hits.is_empty()).await;

    let segment = only_segment_id(&service).await;
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].segment, segment);
    assert_eq!((hits[0].app.as_str(), hits[0].title.as_str()), ("gedit", "notes"));
    assert_eq!(hits[0].snippet, "zebra");
}

#[rstest]
#[case::malformed_id("segment", Which::Latest, "", Code::InvalidArgument)]
#[case::unknown_segment(UNKNOWN, Which::Latest, "", Code::NotFound)]
#[case::unknown_unspecified(UNKNOWN, Which::Unspecified, "", Code::NotFound)]
#[case::malformed_record_id(UNKNOWN, Which::Record, "record", Code::InvalidArgument)]
#[tokio::test]
async fn read_segment_refuses(
    home: Home,
    #[case] id: &str,
    #[case] which: Which,
    #[case] record_id: &str,
    #[case] code: Code,
) {
    let service = running(&home).await;
    let request = ReadSegmentRequest {
        id: id.to_owned(),
        which: which.into(),
        record_id: record_id.to_owned(),
        max_bytes: 0,
    };
    let status = service.read_segment(Request::new(request)).await.expect_err("refused");
    assert_eq!(status.code(), code, "{status:?}");
}

/// A read of the store, as each RPC that reads the store makes it.
#[derive(Debug, Clone, Copy)]
enum StoreRead {
    RecentActivity,
    ListSegments,
    TallySegments,
    Search,
    Lookup,
    ReadSegment,
    ListSnapshots,
    ReadSnapshots,
    DeleteId,
    DeleteMatching,
}

impl StoreRead {
    /// Sends the read to `service`. The reads that name a segment name one that doesn't exist.
    async fn send(self, service: &Service) -> Result<(), Status> {
        match self {
            Self::RecentActivity => {
                let request = RecentActivityRequest { since_ms: 0 };
                service.recent_activity(Request::new(request)).await.map(drop)
            }
            Self::ListSegments => {
                let request = ListSegmentsRequest::default();
                service.list_segments(Request::new(request)).await.map(drop)
            }
            Self::TallySegments => {
                let request = TallySegmentsRequest::default();
                service.tally_segments(Request::new(request)).await.map(drop)
            }
            Self::Search => {
                let request = SearchRequest {
                    query: "zebra".to_owned(),
                    ..SearchRequest::default()
                };
                service.search(Request::new(request)).await.map(drop)
            }
            Self::Lookup => {
                let request = LookupRequest {
                    id: UNKNOWN.to_owned(),
                };
                service.lookup(Request::new(request)).await.map(drop)
            }
            Self::ReadSegment => {
                let request = ReadSegmentRequest {
                    id: UNKNOWN.to_owned(),
                    ..ReadSegmentRequest::default()
                };
                service.read_segment(Request::new(request)).await.map(drop)
            }
            Self::ListSnapshots => {
                let request = ListSnapshotsRequest {
                    id: UNKNOWN.to_owned(),
                };
                service.list_snapshots(Request::new(request)).await.map(drop)
            }
            Self::ReadSnapshots => {
                let request = ReadSnapshotsRequest {
                    id: UNKNOWN.to_owned(),
                    ..ReadSnapshotsRequest::default()
                };
                service.read_snapshots(Request::new(request)).await.map(drop)
            }
            Self::DeleteId => service.delete(Request::new(delete_id(UNKNOWN))).await.map(drop),
            Self::DeleteMatching => {
                let request = delete_matching(DeleteFilter {
                    app: Some("gedit".to_owned()),
                    ..DeleteFilter::default()
                });
                service.delete(Request::new(request)).await.map(drop)
            }
        }
    }
}

/// A read that arrives while the store opens, as when the read started the daemon, waits for
/// the open and then answers from the store.
///
/// Runs in real time, because the index's pool times out while the clock is paused.
#[rstest]
#[tokio::test]
async fn a_read_during_the_open_waits_for_the_store(
    home: Home,
    #[values(
        StoreRead::RecentActivity,
        StoreRead::ListSegments,
        StoreRead::TallySegments,
        StoreRead::Search,
        StoreRead::Lookup,
        StoreRead::ReadSegment,
        StoreRead::ListSnapshots,
        StoreRead::ReadSnapshots,
        StoreRead::DeleteId,
        StoreRead::DeleteMatching
    )]
    read: StoreRead,
) {
    let settings = home.settings(&["gedit"]);
    let held = HeldOpen::new(settings.desktop_capture_dir());
    let capture = home.open_unsettled(&settings, &FakeSpawner::new([Script::Serve])).await;
    let service = Service::new(Arc::new(capture));
    let reply = read.send(&service);
    tokio::pin!(reply);

    let waited = tokio::time::timeout(Duration::from_millis(100), &mut reply).await;
    assert!(waited.is_err(), "the read waits for the store: {waited:?}");
    drop(held);
    let reply = reply.await;

    if let Err(status) = reply {
        assert_eq!(status.code(), Code::NotFound, "{status:?}");
    }
    service.capture.shutdown().await;
}

/// Why a store stays closed.
#[derive(Debug, Clone, Copy)]
enum Closed {
    /// The settings disable capture, and no earlier run left a store.
    Disabled,
    /// Another process holds the store, so the store fails to open.
    Held,
}

/// A read answers at once while the store stays closed.
#[rstest]
#[tokio::test]
async fn a_read_of_a_store_that_stays_closed_answers_at_once(
    home: Home,
    #[values(Closed::Disabled, Closed::Held)] closed: Closed,
    #[values(StoreRead::RecentActivity, StoreRead::Search, StoreRead::Lookup, StoreRead::DeleteId)]
    read: StoreRead,
) {
    let mut settings = home.settings(&["gedit"]);
    let _holder = match closed {
        Closed::Disabled => {
            settings.desktop_capture.enabled = false;
            None
        }
        Closed::Held => {
            Some(DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open"))
        }
    };
    let capture = home.open_unsettled(&settings, &FakeSpawner::new([Script::Serve])).await;
    let service = Service::new(Arc::new(capture));

    let reply = tokio::time::timeout(Duration::from_secs(5), read.send(&service)).await;

    let status = reply.expect("the read answers at once").expect_err("refused");
    assert_eq!(status.code(), Code::Unavailable, "{status:?}");
    service.capture.shutdown().await;
}

#[rstest]
#[tokio::test]
async fn without_a_store_only_status_answers(without_store: Service) {
    let status = without_store.status(Request::new(StatusRequest {})).await.expect("status");
    let status = status.into_inner();
    assert_eq!(status.state(), State::Disabled);
    assert_eq!((status.segments, status.records), (None, None));
    let request = RecentActivityRequest { since_ms: 0 };
    let refused = without_store.recent_activity(Request::new(request)).await.expect_err("refused");
    assert_eq!(refused.code(), Code::Unavailable);
}

/// A login that the client reports starts a helper at once, after the quick retries ended.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_reported_login_starts_a_helper(#[from(paused_home)] home: Home) {
    let spawner = FakeSpawner::new([Script::NoDesktop]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| *state == CaptureState::NoDesktop).await;
    let service = Service::new(Arc::new(capture));
    tokio::time::sleep(Duration::from_hours(1)).await;
    spawner.script_next([Script::Serve]);

    service.session_started(Request::new(SessionStartedRequest {})).await.expect("answered");

    state_becomes(&service.capture, |state| *state == CaptureState::Running).await;
}

/// A start replies once the helper that the start ran has an outcome, if the request asks to wait
/// for the outcome. Otherwise, the start replies at once.
#[rstest]
#[case::waits(10_000, State::NoDesktop)]
#[case::replies_at_once(0, State::Starting)]
#[case::waits_at_most_its_wait(1_000, State::Starting)]
#[tokio::test(start_paused = true)]
async fn a_start_replies_with_the_outcome(
    #[from(paused_home)] home: Home,
    #[case] outcome_wait_ms: u32,
    #[case] shown: State,
) {
    let spawner = FakeSpawner::new([Script::Missing]);
    let capture = home.open(&home.settings(&["gedit"]), &spawner).await;
    state_becomes(&capture, |state| matches!(state, CaptureState::Failed { .. })).await;
    let service = Service::new(Arc::new(capture));
    // The helper of the start finds no desktop 5 s after its start.
    spawner.script_next([Script::SlowNoDesktop]);

    let request = StartRequest { outcome_wait_ms };
    service.start(Request::new(request)).await.expect("started");

    let status = service.status(Request::new(StatusRequest {})).await.expect("status");
    assert_eq!(status.into_inner().state(), shown);
}

#[rstest]
#[tokio::test]
async fn without_a_store_a_reported_login_answers(without_store: Service) {
    let request = Request::new(SessionStartedRequest {});
    without_store.session_started(request).await.expect("answered");
}

#[rstest]
#[tokio::test]
async fn status_reports_a_stop_without_apps(home: Home) {
    let service = running(&home).await;
    let stopped = async || {
        let reply = service.status(Request::new(StatusRequest {})).await.expect("status");
        reply.into_inner().stopped
    };
    assert!(!stopped().await);

    service.capture.stop().await.expect("stop");
    assert!(stopped().await);
}

/// A request that the daemon can't carry out at the moment is unavailable, and a request that
/// failed is internal.
#[rstest]
#[case::request_after_the_supervisor(RequestError::SupervisorEnded.into(), Code::Unavailable)]
#[case::stop_file(
    RequestError::StopFile(StopFileError::Create {
        path: "stopped".into(),
        source: std::io::Error::other("read-only"),
    })
    .into(),
    Code::Internal
)]
#[case::deletion_after_the_supervisor(DeleteError::SupervisorEnded.into(), Code::Unavailable)]
#[case::deletion_without_a_store(DeleteError::NotOpen(StoreNotOpen).into(), Code::Unavailable)]
#[case::store_not_open(StoreNotOpen.into(), Code::Unavailable)]
fn errors_have_their_codes(#[case] status: Status, #[case] code: Code) {
    assert_eq!(status.code(), code, "{status:?}");
}

#[rstest]
#[case::running(CaptureState::Running, State::Running, "")]
#[case::failed(CaptureState::Failed { error: "broken".to_owned() }, State::Failed, "broken")]
#[case::stopped(CaptureState::Stopped, State::Stopped, "")]
fn status_reports_the_state(#[case] state: CaptureState, #[case] wire: State, #[case] error: &str) {
    let reply = StateReply::from(state);
    assert_eq!((reply.state, reply.error.as_str()), (wire, error));
}
