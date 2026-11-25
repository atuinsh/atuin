//! The JSON styles of the listings: one array of objects, or one object per line.

use std::io;

use atuin_common::json::framed::{Framing, write_framed};
use atuin_daemon::grpc::desktop_capture::pb::{ListSnapshotsReply, Segment};
use serde::Serialize;

/// One segment, as the JSON styles of `log` write it.
#[derive(Serialize)]
struct JsonSegment<'a> {
    segment: &'a str,
    kind: &'static str,
    app: &'a str,
    title: &'a str,
    url: Option<&'a str>,
    started_unix_ms: i64,
    /// `None` while the segment is open.
    ended_unix_ms: Option<i64>,
    snapshots: u32,
}

/// Writes each of `segments` as a JSON object.
pub fn write_segments<'a>(
    out: &mut impl io::Write,
    segments: impl IntoIterator<Item = &'a Segment>,
    framing: Framing,
) -> io::Result<()> {
    let segments = segments.into_iter().map(|segment| JsonSegment {
        segment: &segment.id,
        kind: segment.kind().name(),
        app: &segment.app,
        title: &segment.title,
        url: segment.url.as_deref().filter(|url| !url.is_empty()),
        started_unix_ms: segment.started_ms,
        ended_unix_ms: segment.ended_ms,
        snapshots: segment.snapshots,
    });
    write_framed(out, segments, framing)
}

/// One snapshot, as the JSON styles of `snapshots` write it.
#[derive(Serialize)]
struct JsonSnapshot<'a> {
    segment: &'a str,
    snapshot: &'a str,
    at_unix_ms: i64,
    /// `None` for a dialog, which capture reads when it appears.
    trigger: Option<&'a str>,
    nodes: u64,
    same_as_previous: bool,
}

/// Writes each snapshot of `reply` as a JSON object that names its segment.
pub fn write_snapshots(
    out: &mut impl io::Write,
    reply: &ListSnapshotsReply,
    framing: Framing,
) -> io::Result<()> {
    let segment = reply.segment.as_ref().map_or("", |segment| segment.id.as_str());
    let snapshots = reply.snapshots.iter().map(|snapshot| JsonSnapshot {
        segment,
        snapshot: &snapshot.record_id,
        at_unix_ms: snapshot.at_ms,
        trigger: Some(snapshot.trigger.as_str()).filter(|trigger| !trigger.is_empty()),
        nodes: snapshot.nodes,
        same_as_previous: snapshot.same_as_previous,
    });
    write_framed(out, snapshots, framing)
}

#[cfg(test)]
mod tests {
    use atuin_daemon::grpc::desktop_capture::pb::{SegmentKind, Snapshot};
    use rstest::rstest;

    use super::*;

    /// An open segment has no end, and an empty URL is absent.
    #[rstest]
    #[case::ended(
        Some(1_700_000_060_000),
        Some("https://example.com/"),
        "1700000060000",
        r#""https://example.com/""#
    )]
    #[case::open(None, Some(""), "null", "null")]
    fn a_segment_names_everything_needed_to_read_it(
        #[case] ended_ms: Option<i64>,
        #[case] url: Option<&str>,
        #[case] ended: &str,
        #[case] url_json: &str,
    ) {
        let segment = Segment {
            id: "018f0000-0000-7000-8000-000000000001".to_owned(),
            app: "firefox".to_owned(),
            title: "Docs".to_owned(),
            url: url.map(str::to_owned),
            started_ms: 1_700_000_000_000,
            ended_ms,
            snapshots: 3,
            kind: SegmentKind::Active.into(),
        };
        let mut out = Vec::new();
        write_segments(&mut out, [&segment], Framing::Lines).expect("write");

        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                r#"{{"segment":"018f0000-0000-7000-8000-000000000001","kind":"active","app":"firefox","title":"Docs","url":{url_json},"started_unix_ms":1700000000000,"ended_unix_ms":{ended},"snapshots":3}}"#
            ) + "\n"
        );
    }

    /// A dialog's snapshot has no trigger.
    #[rstest]
    #[case::window("edited", r#""edited""#)]
    #[case::dialog("", "null")]
    fn a_snapshot_names_its_segment(#[case] trigger: &str, #[case] trigger_json: &str) {
        let reply = ListSnapshotsReply {
            segment: Some(Segment {
                id: "018f0000-0000-7000-8000-000000000001".to_owned(),
                ..Segment::default()
            }),
            snapshots: vec![Snapshot {
                record_id: "018f0000-0000-7000-8000-000000000002".to_owned(),
                at_ms: 1_700_000_000_000,
                trigger: trigger.to_owned(),
                nodes: 46,
                same_as_previous: true,
            }],
        };
        let mut out = Vec::new();
        write_snapshots(&mut out, &reply, Framing::Lines).expect("write");

        assert_eq!(
            String::from_utf8(out).expect("UTF-8"),
            format!(
                r#"{{"segment":"018f0000-0000-7000-8000-000000000001","snapshot":"018f0000-0000-7000-8000-000000000002","at_unix_ms":1700000000000,"trigger":{trigger_json},"nodes":46,"same_as_previous":true}}"#
            ) + "\n"
        );
    }
}
