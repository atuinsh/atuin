//! `atuin_desktop_read`: the text of one snapshot of a captured window, dialog, or notification.

use std::fmt::{self, Write as _};
use std::num::NonZeroU64;

use atuin_client::settings::Settings;
use atuin_common::time::UnixMillisDisplay;
use atuin_daemon::grpc::desktop_capture::pb::{self, ReadSegmentReply, Segment};
use atuin_daemon::{DesktopCaptureClient, SnapshotChoice, SnapshotsBudget};
use imara_diff::{Algorithm, Diff, InternedInput};
use jiff::tz::TimeZone;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{SegmentLine, failed, rendered};
use crate::tools::ToolOutcome;

/// The longest text that one read returns, in bytes. A longer text keeps its beginning and end.
const MAX_BYTES: NonZeroU64 = NonZeroU64::new(32 * 1024).unwrap();

/// The most changed lines that a read of changes shows for one snapshot.
const MAX_CHANGED_LINES: usize = 60;

/// The most characters of one changed line that a read of changes shows.
const MAX_LINE_CHARS: usize = 300;

/// The most snapshots that a read of changes reads.
const MAX_CHANGE_SNAPSHOTS: usize = 64;

/// The most bytes of snapshot text that a read of changes reads, together. The log names the
/// snapshots past it as left out.
const MAX_CHANGE_TEXT_BYTES: NonZeroU64 = NonZeroU64::new(8 * MAX_BYTES.get()).unwrap();

/// The most snapshots that a cut read of changes names as left out.
const MAX_LEFT_OUT: usize = 100;

/// The most removed lines that one change shows.
///
/// A change that removes more lines only counts them, because the earlier snapshots show them. For
/// example, a switch to another page removes every line of the page before.
const MAX_REMOVED_LINES: usize = 8;

/// Which snapshot of a segment to read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(try_from = "String")]
#[schemars(with = "String")]
pub enum Snapshot {
    /// The one snapshot that the choice names.
    One(SnapshotChoice),
    /// Every snapshot, each with the lines that changed since the snapshot before it.
    Changes,
}

impl TryFrom<String> for Snapshot {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        match text.as_str() {
            "first" => Ok(Self::One(SnapshotChoice::First)),
            "latest" => Ok(Self::One(SnapshotChoice::Latest)),
            "changes" => Ok(Self::Changes),
            _ => match Uuid::try_parse(&text) {
                Ok(record) => Ok(Self::One(SnapshotChoice::Record(record))),
                Err(_) => Err(format!(
                    "snapshot must be \"first\", \"latest\", \"changes\", or a snapshot id, not \
                     {text:?}"
                )),
            },
        }
    }
}

// Doc comments on the fields are the descriptions that the model reads in the tool schema; the
// struct deliberately has none, as it would become the schema's top-level description.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AtuinDesktopReadToolCall {
    /// The id (UUID) of a segment, as atuin_desktop_activity or atuin_desktop_search lists it, or
    /// of one of its snapshots.
    #[schemars(with = "String")]
    pub id: Uuid,
    /// Which snapshot of the segment to read. Without it, a read of a snapshot's id reads that
    /// snapshot, and a read of a segment's id reads the latest snapshot. "first" is how the
    /// window looked when the segment began. "latest" is how the window looked last. A snapshot
    /// id (UUID) from atuin_desktop_search or an earlier read reads that snapshot. "changes"
    /// lists every snapshot with the lines that the snapshot removed (-) and added (+), in order.
    /// For example, "changes" shows options toggled on a settings page that the window no longer
    /// shows.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub snapshot: Option<Snapshot>,
}

impl AtuinDesktopReadToolCall {
    pub(crate) async fn execute(
        self,
        mut client: DesktopCaptureClient,
        settings: &Settings,
    ) -> ToolOutcome {
        let tz = settings.timezone.time_zone();
        let reply = match self.snapshot {
            None => client.read_by_id(self.id, Some(MAX_BYTES)).await,
            Some(Snapshot::One(choice)) => {
                client.read_snapshot(self.id, choice, Some(MAX_BYTES)).await
            }
            Some(Snapshot::Changes) => return changes(client, self.id, tz).await,
        };
        let reply = reply.map_err(failed("Reading the segment"))?;
        Ok(rendered(ReadText {
            reply: &reply,
            tz: &tz,
        }))
    }
}

/// Lists the snapshots of the segment that `id` names, each with the lines that changed since
/// the one before. `id` names the segment or one of its snapshots.
async fn changes(
    mut client: DesktopCaptureClient,
    id: Uuid,
    tz: TimeZone,
) -> Result<String, String> {
    let listing = client.list_snapshots(id).await.map_err(failed("Listing the snapshots"))?;
    let mut log = ChangeLog::new(listing.segment.as_ref(), tz);
    let mut snapshots = listing.snapshots;
    snapshots.retain(|snapshot| !snapshot.same_as_previous);
    let wanted = &snapshots[..snapshots.len().min(MAX_CHANGE_SNAPSHOTS)];
    let budget = SnapshotsBudget {
        per_text: Some(MAX_BYTES),
        total: MAX_CHANGE_TEXT_BYTES,
    };
    let read = client
        .read_listed_snapshots(id, wanted, budget)
        .await
        .map_err(failed("Reading the snapshots"))?;
    // The daemon returns the texts in the order of the segment's snapshots, which is the order
    // of the listing. The texts that the budget leaves out come last, and the log names them as
    // left out.
    let mut taken = 0;
    for (snapshot, text) in wanted.iter().zip(read.texts) {
        if log.is_full() {
            break;
        }
        log.push(snapshot, text.text);
        taken += 1;
    }
    log.cut(&snapshots[taken..]);
    Ok(log.into_text())
}

/// The text of a read of changes: the segment's line, then each snapshot with the lines that it
/// changed, up to about [`MAX_BYTES`].
struct ChangeLog {
    text: String,
    tz: TimeZone,
    /// The text of the last snapshot that the log shows.
    before: Option<String>,
}

impl ChangeLog {
    fn new(segment: Option<&Segment>, tz: TimeZone) -> Self {
        let mut text = String::new();
        if let Some(segment) = segment {
            let _ = writeln!(text, "{}", SegmentLine { segment, tz: &tz });
        }
        text.push_str(
            "Each snapshot lists the lines that it removed (-) and added (+) since the snapshot \
             before it.\n",
        );
        Self {
            text,
            tz,
            before: None,
        }
    }

    /// Returns whether the log has reached its limit. A full log doesn't take more snapshots.
    fn is_full(&self) -> bool {
        self.text.len() >= usize::try_from(MAX_BYTES.get()).unwrap_or(usize::MAX)
    }

    /// Ends the log with the ids of `left_out`, the later snapshots that it leaves out, when
    /// there are any.
    fn cut(&mut self, left_out: &[pb::Snapshot]) {
        if left_out.is_empty() {
            return;
        }
        self.text.push_str(
            "\nThe list reached its limit and left out these later snapshots. Read one by passing \
             its id as snapshot:\n",
        );
        for snapshot in left_out.iter().take(MAX_LEFT_OUT) {
            let at = UnixMillisDisplay::clock_seconds(snapshot.at_ms, &self.tz);
            let _ = writeln!(self.text, "{} at {at}", snapshot.record_id);
        }
        if let Some(more) = left_out.len().checked_sub(MAX_LEFT_OUT).filter(|&more| more > 0) {
            let _ = writeln!(
                self.text,
                "… and {more} more. Snapshot \"latest\" reads the last of them."
            );
        }
    }

    /// Adds `snapshot`, whose text is `after`, with the lines that changed since the snapshot
    /// before. A snapshot that didn't change any line is left out.
    fn push(&mut self, snapshot: &pb::Snapshot, mut after: String) {
        // Rendered text ends without a newline. Without one, the diff would report the last line
        // as changed when the next snapshot adds lines after it.
        if !after.ends_with('\n') {
            after.push('\n');
        }
        let heading = SnapshotHeading {
            snapshot,
            tz: &self.tz,
        };
        match &self.before {
            None => {
                let lines = after.lines().count();
                let _ = writeln!(
                    self.text,
                    "{heading} the first, with {lines} lines. Read it with snapshot \"first\"."
                );
            }
            Some(before) => {
                let changes = Changes::new(before, &after);
                if changes.is_empty() {
                    return;
                }
                let _ = write!(self.text, "{heading}\n{changes}");
            }
        }
        self.before = Some(after);
    }

    fn into_text(self) -> String {
        self.text
    }
}

/// The line that starts a snapshot of a [`ChangeLog`]: its id, its time, and what triggered it.
struct SnapshotHeading<'a> {
    snapshot: &'a pb::Snapshot,
    tz: &'a TimeZone,
}

impl fmt::Display for SnapshotHeading<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pb::Snapshot {
            record_id,
            at_ms,
            trigger,
            ..
        } = self.snapshot;
        let at = UnixMillisDisplay::clock_seconds(*at_ms, self.tz);
        let trigger = if trigger.is_empty() {
            "appeared"
        } else {
            trigger
        };
        write!(f, "\nSnapshot {record_id} at {at} ({trigger}):")
    }
}

/// The lines that one text removed from another and added to it, at most
/// [`MAX_CHANGED_LINES`] of them.
struct Changes<'a> {
    input: InternedInput<&'a str>,
    diff: Diff,
}

impl<'a> Changes<'a> {
    fn new(before: &'a str, after: &'a str) -> Self {
        let input = InternedInput::new(before, after);
        let mut diff = Diff::compute(Algorithm::Histogram, &input);
        diff.postprocess_lines(&input);
        Self { input, diff }
    }

    fn is_empty(&self) -> bool {
        self.diff.hunks().next().is_none()
    }
}

impl fmt::Display for Changes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { input, diff } = self;
        let mut changed = 0;
        for hunk in diff.hunks() {
            let removed = &input.before[hunk.before.start as usize..hunk.before.end as usize];
            let added = &input.after[hunk.after.start as usize..hunk.after.end as usize];
            let shown_removed = if removed.len() > MAX_REMOVED_LINES {
                changed += 1;
                if changed <= MAX_CHANGED_LINES {
                    writeln!(f, "- ({} lines that the snapshots before show)", removed.len())?;
                }
                &[][..]
            } else {
                removed
            };
            let removed = shown_removed.iter().map(|&token| ('-', token));
            let added = added.iter().map(|&token| ('+', token));
            for (sign, token) in removed.chain(added) {
                changed += 1;
                if changed <= MAX_CHANGED_LINES {
                    let line = input.interner[token].trim_end_matches('\n');
                    match line.char_indices().nth(MAX_LINE_CHARS) {
                        Some((cut, _)) => writeln!(f, "{sign} {}…", &line[..cut])?,
                        None => writeln!(f, "{sign} {line}")?,
                    }
                }
            }
        }
        if changed > MAX_CHANGED_LINES {
            writeln!(f, "… and {} more changed lines", changed - MAX_CHANGED_LINES)?;
        }
        Ok(())
    }
}

/// The text of a read: the segment's line, the snapshot's id and time, and the snapshot's text.
struct ReadText<'a> {
    reply: &'a ReadSegmentReply,
    tz: &'a TimeZone,
}

impl fmt::Display for ReadText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { reply, tz } = *self;
        if let Some(segment) = &reply.segment {
            writeln!(f, "{}", SegmentLine { segment, tz })?;
        }
        let at = UnixMillisDisplay::ymd_hm(reply.at_ms, tz);
        write!(f, "Snapshot {} taken {at}\n\n{}", reply.record_id, reply.text)?;
        if reply.truncated {
            f.write_str("\n\nThe text was cut to the limit where […] stands.")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    #[case::toggle(
        "Power\n[ ] Show Battery Percentage\nPerformance\n",
        "Power\n[x] Show Battery Percentage\nPerformance\n",
        "- [ ] Show Battery Percentage\n+ [x] Show Battery Percentage\n"
    )]
    #[case::no_newline_at_the_end("a\nb\n", "a\nc\n", "- b\n+ c\n")]
    #[case::appended("log1\nlog2\n", "log1\nlog2\nlog3\n", "+ log3\n")]
    #[case::same("a\nb", "a\nb", "")]
    #[case::page_switch(
        "1\n2\n3\n4\n5\n6\n7\n8\n9\n",
        "other page\n",
        "- (9 lines that the snapshots before show)\n+ other page\n"
    )]
    fn changes_list_removed_and_added_lines(
        #[case] before: &str,
        #[case] after: &str,
        #[case] expected: &str,
    ) {
        let changes = Changes::new(before, after);
        assert_eq!(changes.to_string(), expected);
        assert_eq!(changes.is_empty(), expected.is_empty());
    }

    #[rstest]
    fn changes_stop_at_the_limit() {
        let after: String = (0..MAX_CHANGED_LINES + 5).map(|i| format!("{i}\n")).collect();
        let text = Changes::new("", &after).to_string();
        assert_eq!(text.lines().count(), MAX_CHANGED_LINES + 1);
        assert!(text.ends_with("… and 5 more changed lines\n"));
    }

    const START_MS: i64 = 1_700_000_000_000;

    fn snapshot(record: u8, trigger: &str) -> pb::Snapshot {
        pb::Snapshot {
            record_id: format!("018f0000-0000-7000-8000-00000000000{record}"),
            at_ms: START_MS + i64::from(record) * 1000,
            trigger: trigger.to_owned(),
            ..pb::Snapshot::default()
        }
    }

    #[rstest]
    fn a_change_log_counts_the_first_snapshot_and_leaves_out_unchanged_ones() {
        let mut log = ChangeLog::new(None, TimeZone::UTC);
        log.push(&snapshot(1, ""), "a\nb".to_owned());
        log.push(&snapshot(2, "polled"), "a\nb\n".to_owned());
        log.push(&snapshot(3, "focused"), "a\nc".to_owned());

        assert!(!log.is_full());
        assert_eq!(
            log.into_text(),
            "Each snapshot lists the lines that it removed (-) and added (+) since the snapshot \
             before it.\n\nSnapshot 018f0000-0000-7000-8000-000000000001 at 22:13:21 (appeared): \
             the first, with 2 lines. Read it with snapshot \"first\".\n\nSnapshot \
             018f0000-0000-7000-8000-000000000003 at 22:13:23 (focused):\n- b\n+ c\n"
        );
    }

    #[rstest]
    fn a_change_log_fills_up_and_says_where_it_was_cut() {
        let page = |name: &str| -> String {
            (0..MAX_CHANGED_LINES)
                .map(|line| format!("{name} {line} {}\n", "x".repeat(MAX_LINE_CHARS)))
                .collect()
        };
        let mut log = ChangeLog::new(None, TimeZone::UTC);
        log.push(&snapshot(1, ""), String::new());
        log.push(&snapshot(2, ""), page("one"));
        assert!(!log.is_full());
        log.push(&snapshot(3, ""), page("two"));
        assert!(log.is_full());

        log.cut(&[snapshot(4, ""), snapshot(5, "")]);

        let text = log.into_text();
        assert!(
            text.ends_with(
                "\nThe list reached its limit and left out these later snapshots. Read one by \
                 passing its id as snapshot:\n018f0000-0000-7000-8000-000000000004 at \
                 22:13:24\n018f0000-0000-7000-8000-000000000005 at 22:13:25\n"
            ),
            "{text}"
        );
    }

    /// A cut that leaves out more snapshots than it names counts the rest.
    #[rstest]
    fn a_cut_counts_the_snapshots_that_it_does_not_name() {
        let left_out: Vec<_> = (0..MAX_LEFT_OUT + 3).map(|_| snapshot(4, "")).collect();
        let mut log = ChangeLog::new(None, TimeZone::UTC);

        log.cut(&left_out);

        let text = log.into_text();
        assert_eq!(text.matches("018f0000-0000-7000-8000-000000000004").count(), MAX_LEFT_OUT);
        assert!(text.ends_with("… and 3 more. Snapshot \"latest\" reads the last of them.\n"));
    }

    /// A log that left nothing out says nothing about a cut.
    #[rstest]
    fn a_log_without_a_cut_says_nothing_about_one() {
        let mut log = ChangeLog::new(None, TimeZone::UTC);
        log.push(&snapshot(1, ""), "a".to_owned());

        let before = log.text.clone();
        log.cut(&[]);

        assert_eq!(log.into_text(), before);
    }

    const RECORD: &str = "018f0000-0000-7000-8000-000000000000";

    #[rstest]
    #[case::minimal(json!({"id": RECORD}), Ok(None))]
    #[case::first(
        json!({"id": RECORD, "snapshot": "first"}),
        Ok(Some(Snapshot::One(SnapshotChoice::First)))
    )]
    #[case::latest(
        json!({"id": RECORD, "snapshot": "latest"}),
        Ok(Some(Snapshot::One(SnapshotChoice::Latest)))
    )]
    #[case::a_record(
        json!({"id": RECORD, "snapshot": RECORD}),
        Ok(Some(Snapshot::One(SnapshotChoice::Record(RECORD.parse().unwrap()))))
    )]
    #[case::not_a_snapshot(json!({"id": RECORD, "snapshot": "newest"}), Err("snapshot"))]
    #[case::not_a_string(json!({"id": RECORD, "snapshot": 3}), Err("expected a string"))]
    #[case::the_removed_extent(json!({"id": RECORD, "visible_only": true}), Err("visible_only"))]
    #[case::the_old_field(json!({"segment": "s"}), Err("segment"))]
    #[case::no_id(json!({"snapshot": "first"}), Err("id"))]
    #[case::an_id_that_is_not_a_uuid(json!({"id": "segment"}), Err("invalid"))]
    fn arguments_parse(
        #[case] args: serde_json::Value,
        #[case] expected: Result<Option<Snapshot>, &str>,
    ) {
        let parsed = serde_json::from_value::<AtuinDesktopReadToolCall>(args);
        match (parsed, expected) {
            (Ok(call), Ok(snapshot)) => assert_eq!(call.snapshot, snapshot),
            (Err(err), Err(mention)) => assert!(err.to_string().contains(mention), "{err}"),
            (got, expected) => panic!("{got:?} against {expected:?}"),
        }
    }

    #[rstest]
    #[case::changes("changes", Snapshot::Changes)]
    #[case::first("first", Snapshot::One(SnapshotChoice::First))]
    fn snapshots_parse_by_name(#[case] text: &str, #[case] expected: Snapshot) {
        assert_eq!(Snapshot::try_from(text.to_owned()), Ok(expected));
    }

    #[rstest]
    #[case::whole(false, "")]
    #[case::cut(true, "\n\nThe text was cut to the limit where […] stands.")]
    fn a_read_names_its_snapshot(#[case] truncated: bool, #[case] ending: &str) {
        let reply = ReadSegmentReply {
            at_ms: 1_700_000_000_000,
            text: "the text".to_owned(),
            truncated,
            record_id: "018f0000-0000-7000-8000-000000000001".to_owned(),
            ..ReadSegmentReply::default()
        };
        let text = ReadText {
            reply: &reply,
            tz: &TimeZone::UTC,
        }
        .to_string();
        assert_eq!(
            text,
            format!(
                "Snapshot 018f0000-0000-7000-8000-000000000001 taken 2023-11-14 22:13\n\nthe \
                 text{ending}"
            )
        );
    }
}
