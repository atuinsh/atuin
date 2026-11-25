//! `atuin_desktop_search`: full-text search over captured window text.

use std::fmt;

use atuin_client::settings::{Settings, TimeRange, TimeRangeError};
use atuin_common::range::Clamped;
use atuin_common::string::{NonBlankString, OneLine};
use atuin_common::time::UnixMillisDisplay;
use atuin_daemon::grpc::desktop_capture::pb::{SearchHit, SearchRequest};
use atuin_daemon::{DesktopCaptureClient, MAX_SNIPPET_CHARS};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Kind, failed, rendered, tool_range, wire_kinds};
use crate::tools::ToolOutcome;

// Doc comments on the fields are the descriptions that the model reads in the tool schema; the
// struct deliberately has none, as it would become the schema's top-level description.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AtuinDesktopSearchToolCall {
    /// What to look for. Every word must appear in the text, application, or title of a captured
    /// window, ignoring case.
    ///
    /// Use two or three distinctive words that the window would hold. For example, "migration
    /// deadline". Background windows are searched too.
    pub query: NonBlankString,
    /// Maximum number of windows to return, best match first. Each window appears once, with
    /// its best snapshot.
    #[serde(default)]
    pub limit: Clamped<u32, 1, 100, 20>,
    /// Only the windows that were still open at this time or later. Defaults to the oldest
    /// capture.
    ///
    /// The time is an RFC 3339 time, a duration back from now, or a day. For example, '2h',
    /// '1d 12h', 'today', 'yesterday', or '3 days ago'.
    #[serde(default)]
    pub from: Option<String>,
    /// Only the windows that opened at this time or earlier, in the same forms as from. A day,
    /// such as 'yesterday', reaches to the end of that day. Defaults to now.
    #[serde(default)]
    pub to: Option<String>,
    /// Only the segments of this application, as atuin_desktop_activity names it, ignoring case.
    #[serde(default)]
    pub app: Option<String>,
    /// Only the segments of these kinds. Defaults to every kind.
    #[serde(default)]
    pub kinds: Vec<Kind>,
}

impl AtuinDesktopSearchToolCall {
    pub(crate) async fn execute(
        self,
        mut client: DesktopCaptureClient,
        settings: &Settings,
    ) -> ToolOutcome {
        let tz = settings.timezone.time_zone();
        let now = Timestamp::now().to_zoned_datetime(tz.clone());
        let range = self.range(&now, settings).map_err(|e| format!("Invalid arguments: {e}"))?;
        let reply = client
            .search(SearchRequest {
                query: self.query.into_inner(),
                limit: self.limit.get(),
                app: self.app,
                kinds: wire_kinds(self.kinds),
                from_ms: range.start_ms(),
                to_ms: range.wire_to_ms(),
            })
            .await
            .map_err(failed("Searching desktop capture"))?;
        Ok(rendered(Hits {
            hits: &reply.hits,
            tz: &tz,
        }))
    }

    fn range(&self, now: &ZonedDateTime, settings: &Settings) -> Result<TimeRange, TimeRangeError> {
        let from = self.from.as_deref();
        tool_range(from, self.to.as_deref(), Timestamp::UNIX_EPOCH, now, settings)
    }
}

/// The text of the hits of a search: for each hit, the segment and snapshot to read, and a
/// snippet.
struct Hits<'a> {
    hits: &'a [SearchHit],
    tz: &'a TimeZone,
}

impl fmt::Display for Hits<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.hits.is_empty() {
            return f.write_str("No captured window, dialog, or notification matches.");
        }
        for hit in self.hits {
            let at = UnixMillisDisplay::ymd_hm(hit.at_ms, self.tz);
            let snippet = OneLine {
                text: &hit.snippet,
                max_chars: MAX_SNIPPET_CHARS,
            };
            writeln!(
                f,
                "{} | {} | {} | {:?} | snapshot {} at {at}\n    {snippet}",
                hit.segment,
                hit.app,
                hit.kind().name(),
                hit.title,
                hit.record_id
            )?;
        }
        f.write_str(
            "\nTo read the text that matched, call atuin_desktop_read with the segment id as id \
             and the snapshot id as snapshot.",
        )
    }
}

#[cfg(test)]
mod tests {
    use atuin_daemon::grpc::desktop_capture::pb::SegmentKind;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    fn a_hit_names_its_segment_and_snapshot() {
        let hit = SearchHit {
            segment: "018f0000-0000-7000-8000-000000000001".to_owned(),
            app: "Firefox".to_owned(),
            title: "Docs".to_owned(),
            snippet: "the  migration\ndeadline".to_owned(),
            record_id: "018f0000-0000-7000-8000-000000000002".to_owned(),
            at_ms: 1_700_000_000_000,
            kind: SegmentKind::Dialog.into(),
        };
        let text = Hits {
            hits: &[hit],
            tz: &TimeZone::UTC,
        }
        .to_string();
        assert_eq!(
            text,
            "018f0000-0000-7000-8000-000000000001 | Firefox | dialog | \"Docs\" | snapshot \
             018f0000-0000-7000-8000-000000000002 at 2023-11-14 22:13\n    the migration \
             deadline\n\nTo read the text that matched, call atuin_desktop_read with the segment \
             id as id and the snapshot id as snapshot."
        );
    }

    /// An absent `from` reaches back to the oldest capture, and an absent `to` is now.
    #[rstest]
    #[case::open(None, None, Ok(Bounds { from_ms: 0, to_ms: 0 }))]
    #[case::closed(
        Some("2026-11-14T09:30:00Z"),
        Some("2026-11-14T10:30:00Z"),
        Ok(Bounds { from_ms: 1_794_648_600_000, to_ms: 1_794_652_200_000 })
    )]
    #[case::backwards(Some("1h"), Some("2h"), Err(()))]
    #[case::bad_time(None, Some("soon"), Err(()))]
    fn ranges(
        #[case] from: Option<&str>,
        #[case] to: Option<&str>,
        #[case] expected: Result<Bounds, ()>,
    ) {
        let call = AtuinDesktopSearchToolCall {
            query: NonBlankString::new("x".to_owned()).unwrap(),
            limit: Clamped::default(),
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
            app: None,
            kinds: Vec::new(),
        };
        let now = Timestamp::now().to_zoned_datetime(TimeZone::UTC);
        let range = call.range(&now, &Settings::utc()).map(|range| Bounds {
            from_ms: range.start_ms(),
            to_ms: range.wire_to_ms(),
        });
        assert_eq!(range.map_err(|_| ()), expected);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Bounds {
        from_ms: i64,
        to_ms: i64,
    }

    #[rstest]
    #[case::words(json!({"query": "migration deadline"}), Some(20))]
    #[case::limit(json!({"query": "x", "limit": 5}), Some(5))]
    #[case::capped(json!({"query": "x", "limit": 500}), Some(100))]
    #[case::no_query(json!({"limit": 5}), None)]
    #[case::blank_query(json!({"query": "  "}), None)]
    #[case::filters(
        json!({"query": "x", "from": "2h", "to": "1h", "app": "kate", "kinds": ["active", "dialog"]}),
        Some(20)
    )]
    #[case::unknown_kind(json!({"query": "x", "kinds": ["window"]}), None)]
    fn arguments_parse(#[case] args: serde_json::Value, #[case] limit: Option<u32>) {
        let call = serde_json::from_value::<AtuinDesktopSearchToolCall>(args).ok();
        assert_eq!(call.map(|call| call.limit.get()), limit);
    }

    #[rstest]
    fn no_hits_say_so() {
        let text = Hits {
            hits: &[],
            tz: &TimeZone::UTC,
        };
        assert_eq!(text.to_string(), "No captured window, dialog, or notification matches.");
    }
}
