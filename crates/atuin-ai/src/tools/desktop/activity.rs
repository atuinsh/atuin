//! `atuin_desktop_activity`: the windows, dialogs, and notifications of a time range.

use std::fmt;

use atuin_client::settings::{Settings, TimeRange, TimeRangeError};
use atuin_daemon::DesktopCaptureClient;
use atuin_daemon::grpc::desktop_capture::pb::{ListSegmentsRequest, Segment};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ZonedDateTime};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Kind, SegmentLine, failed, rendered, tool_range, wire_kinds};
use crate::tools::ToolOutcome;

/// The most segments that one reply lists. The newest are kept.
const MAX_SEGMENTS: usize = 200;
/// How far back the listing reaches when `from` is absent.
const DEFAULT_LOOK_BACK: SignedDuration = SignedDuration::from_hours(24);

// Doc comments on the fields are the descriptions that the model reads in the tool schema; the
// struct deliberately has none, as it would become the schema's top-level description.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AtuinDesktopActivityToolCall {
    /// The start of the range: an RFC 3339 time, a duration back from now, or a day. For
    /// example, '2h', '1d 12h', 'today', 'yesterday', or '3 days ago'. Defaults to 24 hours ago.
    #[serde(default)]
    pub from: Option<String>,
    /// The end of the range, in the same forms as from. A day, such as 'yesterday', reaches to
    /// the end of that day. Defaults to now.
    #[serde(default)]
    pub to: Option<String>,
    /// Only the segments of this application, as the listing names it, ignoring case.
    #[serde(default)]
    pub app: Option<String>,
    /// Only the segments of these kinds. Defaults to every kind apart from background, because
    /// background windows are numerous and the user never looked at them.
    #[serde(default)]
    pub kinds: Vec<Kind>,
}

/// The kinds that a listing shows when the call doesn't name any.
const DEFAULT_KINDS: [Kind; 3] = [Kind::Active, Kind::Dialog, Kind::Notification];

impl AtuinDesktopActivityToolCall {
    pub(crate) async fn execute(
        self,
        mut client: DesktopCaptureClient,
        settings: &Settings,
    ) -> ToolOutcome {
        let tz = settings.timezone.time_zone();
        let now = Timestamp::now().to_zoned_datetime(tz.clone());
        let range = self.range(&now, settings).map_err(|e| format!("Invalid arguments: {e}"))?;
        let kinds = if self.kinds.is_empty() {
            wire_kinds(DEFAULT_KINDS)
        } else {
            wire_kinds(self.kinds)
        };
        // The daemon lists oldest first, as the proto says and its tests pin. One more than the
        // cap tells whether older entries were left out.
        let segments = client
            .list_segments(ListSegmentsRequest {
                from_ms: range.start_ms(),
                to_ms: range.wire_to_ms(),
                app: self.app,
                kinds,
                limit: Some(cap_with_one_more(MAX_SEGMENTS)),
            })
            .await
            .map_err(failed("Listing desktop activity"))?
            .segments;
        Ok(rendered(Activity {
            segments: &segments,
            tz: &tz,
            max_segments: MAX_SEGMENTS,
        }))
    }

    fn range(&self, now: &ZonedDateTime, settings: &Settings) -> Result<TimeRange, TimeRangeError> {
        let default_from = now.timestamp() - DEFAULT_LOOK_BACK;
        tool_range(self.from.as_deref(), self.to.as_deref(), default_from, now, settings)
    }
}

/// The request limit for a listing of `cap` entries. The extra entry shows whether the listing
/// left out older entries.
fn cap_with_one_more(cap: usize) -> u32 {
    u32::try_from(cap.saturating_add(1)).unwrap_or(u32::MAX)
}

/// The text of the activity listing. The newest segments are kept when the list is longer than
/// `max_segments`.
struct Activity<'a> {
    /// Oldest first.
    segments: &'a [Segment],
    tz: &'a TimeZone,
    max_segments: usize,
}

impl fmt::Display for Activity<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { segments, tz, .. } = *self;
        let omitted = segments.len().saturating_sub(self.max_segments);
        if omitted > 0 {
            f.write_str("Older segments are not shown. Narrow from, to, or app to see them.\n")?;
        }
        for segment in &segments[omitted..] {
            writeln!(f, "{}", SegmentLine { segment, tz })?;
        }
        if segments.is_empty() {
            f.write_str("No segments in this range.\n")?;
        } else {
            f.write_str("\nRead one with atuin_desktop_read and its id.\n")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::Dialect;
    use atuin_daemon::grpc::desktop_capture::pb::SegmentKind;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    const START_MS: i64 = 1_700_000_000_000;
    const MINUTE_MS: i64 = 60_000;

    fn segment(index: i64) -> Segment {
        Segment {
            id: format!("id-{index}"),
            kind: SegmentKind::Active.into(),
            app: "app".to_owned(),
            title: format!("title {index}"),
            url: None,
            started_ms: START_MS + index * MINUTE_MS,
            ended_ms: None,
            snapshots: 1,
        }
    }

    fn notification(index: i64) -> Segment {
        Segment {
            kind: SegmentKind::Notification.into(),
            ended_ms: Some(START_MS + index * MINUTE_MS),
            ..segment(index)
        }
    }

    fn text(segments: &[Segment], max_segments: usize) -> String {
        Activity {
            segments,
            tz: &TimeZone::UTC,
            max_segments,
        }
        .to_string()
    }

    #[rstest]
    fn nothing_says_so() {
        assert_eq!(text(&[], 10), "No segments in this range.\n");
    }

    #[rstest]
    fn the_newest_segments_are_kept_under_the_cap() {
        let segments: Vec<_> = (0..4).map(segment).collect();
        let out = text(&segments, 2);
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(lines[0], "Older segments are not shown. Narrow from, to, or app to see them.");
        assert!(lines[1].starts_with("id-2 | app | active | \"title 2\""), "{out}");
        assert!(lines[2].starts_with("id-3 "), "{out}");
        assert_eq!(lines.len(), 5, "{out}");
        assert!(out.ends_with("Read one with atuin_desktop_read and its id.\n"));
    }

    #[rstest]
    fn a_notification_is_a_row_of_the_segment_table() {
        let out = text(&[segment(1), notification(2)], 10);

        let lines: Vec<_> = out.lines().collect();
        assert!(lines[0].starts_with("id-1 | app | active | "), "{out}");
        assert!(lines[1].starts_with("id-2 | app | notification | \"title 2\""), "{out}");
        assert!(!out.contains("Notifications:"), "{out}");
    }

    #[rstest]
    #[case::backwards(Some("1h"), Some("2h"), false)]
    #[case::forwards(Some("2h"), Some("1h"), true)]
    #[case::open_end(Some("2h"), None, true)]
    #[case::words(Some("yesterday"), Some("3 hours ago"), true)]
    #[case::bad_time(Some("soon"), None, false)]
    #[case::an_end_before_the_default_start(None, Some("3 days ago"), false)]
    #[case::an_end_after_the_default_start(None, Some("3 hours ago"), true)]
    fn ranges(#[case] from: Option<&str>, #[case] to: Option<&str>, #[case] ok: bool) {
        let call = AtuinDesktopActivityToolCall {
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
            app: None,
            kinds: Vec::new(),
        };
        let mut settings = Settings::utc();
        settings.dialect = Dialect::Uk;
        let now = Timestamp::now().to_zoned_datetime(TimeZone::UTC);
        assert_eq!(call.range(&now, &settings).is_ok(), ok);
    }

    #[rstest]
    #[case::empty(json!({}), true)]
    #[case::everything(
        json!({"from": "2h", "to": "2026-11-14T09:30:00Z", "app": "Settings", "kinds": ["background"]}),
        true
    )]
    #[case::unknown_argument(json!({"since": "2h"}), false)]
    #[case::the_old_flag(json!({"include_background": true}), false)]
    #[case::unknown_kind(json!({"kinds": ["window"]}), false)]
    fn arguments_parse(#[case] args: serde_json::Value, #[case] ok: bool) {
        assert_eq!(serde_json::from_value::<AtuinDesktopActivityToolCall>(args).is_ok(), ok);
    }
}
