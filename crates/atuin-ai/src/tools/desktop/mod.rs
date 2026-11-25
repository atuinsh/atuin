//! MCP tools over the desktop capture store that the daemon keeps: `atuin_desktop_activity`,
//! `atuin_desktop_read`, and `atuin_desktop_search`.

pub mod activity;
pub mod read;
pub mod search;

use std::fmt::{self, Write as _};

use atuin_client::settings::{GivenEnd, RangeEnd, Settings, TimeRange, TimeRangeError};
use atuin_common::time::UnixMillisDisplay;
use atuin_daemon::DesktopCaptureClient;
use atuin_daemon::grpc::desktop_capture::pb::{Segment, SegmentKind};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

use self::activity::AtuinDesktopActivityToolCall;
use self::read::AtuinDesktopReadToolCall;
use self::search::AtuinDesktopSearchToolCall;
use crate::mcp::parse;
use crate::tools::ToolOutcome;

/// Returns whether the desktop capture tools are listed: they are, once the daemon has made the
/// store.
#[must_use]
pub fn is_listed(settings: &Settings) -> bool {
    atuin_daemon::store_exists(&settings.desktop_capture_dir())
}

/// Returns whether a tool name belongs to the desktop capture tools.
#[must_use]
pub fn is_desktop_tool(name: &str) -> bool {
    name.starts_with("atuin_desktop_")
}

/// The desktop capture tools, with one connection to the daemon that their calls share.
#[derive(Default)]
pub struct DesktopTools {
    /// The connection that the calls share, until a call fails.
    client: Mutex<Option<DesktopCaptureClient>>,
}

/// A call of one of the desktop capture tools, with its arguments.
#[derive(Clone)]
enum Call {
    Activity(AtuinDesktopActivityToolCall),
    Read(AtuinDesktopReadToolCall),
    Search(AtuinDesktopSearchToolCall),
}

impl Call {
    async fn execute(self, client: DesktopCaptureClient, settings: &Settings) -> ToolOutcome {
        match self {
            Self::Activity(call) => call.execute(client, settings).await,
            Self::Read(call) => call.execute(client, settings).await,
            Self::Search(call) => call.execute(client, settings).await,
        }
    }
}

impl DesktopTools {
    /// Runs the desktop capture tool `name` with `arguments`. Returns `None` when `name` isn't a
    /// desktop capture tool.
    pub async fn call(
        &self,
        name: &str,
        arguments: JsonObject,
        settings: &Settings,
    ) -> Option<ToolOutcome> {
        let call = match name {
            "atuin_desktop_activity" => parse(name, arguments).map(Call::Activity),
            "atuin_desktop_read" => parse(name, arguments).map(Call::Read),
            "atuin_desktop_search" => parse(name, arguments).map(Call::Search),
            _ => return None,
        };
        if !is_listed(settings) {
            return Some(Err("Desktop capture hasn't stored anything, because it is off or \
                             hasn't captured yet. See `atuin desktop-capture status`."
                .to_owned()));
        }
        Some(match call {
            Ok(call) => self.run(call, settings).await,
            Err(err) => Err(err),
        })
    }

    /// Runs `call` over the connection that the last call used, and again over a new connection
    /// when that fails.
    ///
    /// The daemon may have stopped since the last call, and a new connection then fails with
    /// the message that says so. The tools only read, so a second run is safe.
    async fn run(&self, call: Call, settings: &Settings) -> ToolOutcome {
        let cached = self.client.lock().await.clone();
        if let Some(client) = cached {
            let outcome = call.clone().execute(client, settings).await;
            if outcome.is_ok() {
                return outcome;
            }
            self.client.lock().await.take();
        }
        let outcome = call.execute(self.connect(settings).await?, settings).await;
        if outcome.is_err() {
            self.client.lock().await.take();
        }
        outcome
    }

    /// A new connection to the daemon, which the next calls share.
    async fn connect(&self, settings: &Settings) -> Result<DesktopCaptureClient, String> {
        let mut cached = self.client.lock().await;
        let client = DesktopCaptureClient::from_settings(settings).await.map_err(|_| {
            "desktop capture needs the Atuin daemon: run `atuin daemon start`".to_owned()
        })?;
        Ok(cached.insert(client).clone())
    }

    /// The definitions of the desktop capture tools, for `tools/list`.
    pub fn definitions() -> [Tool; 3] {
        [
            Tool::new(
                "atuin_desktop_activity",
                format!(
                    "List the windows, dialogs, and notifications that desktop capture saw in a \
                     time range (the last 24 hours by default), oldest first. {DESCRIPTION_SCOPE} \
                     Each line gives the segment id, application, kind (active, dialog, \
                     notification, or background), title, URL, and local start and end times. \
                     Pass the id to atuin_desktop_read to read the text, which for a notification \
                     is its body. Windows that stayed in the background are left out unless kinds \
                     names them. Requires the Atuin daemon with desktop capture running."
                ),
                schema_for_type::<AtuinDesktopActivityToolCall>(),
            )
            .annotate(ToolAnnotations::with_title("List desktop activity").read_only(true)),
            Tool::new(
                "atuin_desktop_read",
                format!(
                    "Read the text of a captured window, dialog, or notification, by the segment \
                     id from atuin_desktop_activity or atuin_desktop_search. {DESCRIPTION_SCOPE} \
                     The snapshot argument chooses the first snapshot, the latest one (the \
                     default), or a snapshot id from atuin_desktop_search. To see what changed in \
                     a window, pass snapshot \"changes\". For example, \"changes\" shows the \
                     settings that the user toggled, including changes on pages that the window \
                     no longer shows. A text longer than 32 KiB keeps its beginning and its end. \
                     Requires the Atuin daemon."
                ),
                schema_for_type::<AtuinDesktopReadToolCall>(),
            )
            .annotate(
                ToolAnnotations::with_title("Read a captured window, dialog, or notification")
                    .read_only(true),
            ),
            Tool::new(
                "atuin_desktop_search",
                format!(
                    "Full-text search over the text of every captured window, dialog, and \
                     notification, background windows included. {DESCRIPTION_SCOPE} Use it when \
                     you know what the window said but not where it was. Narrow it to a time \
                     range, an application, or segment kinds. Each hit gives the segment id, \
                     application, kind, title, the id and time of the snapshot that matched, and \
                     a snippet. To read the text that matched, pass the segment id as id and the \
                     snapshot id as snapshot to atuin_desktop_read. Requires the Atuin daemon."
                ),
                schema_for_type::<AtuinDesktopSearchToolCall>(),
            )
            .annotate(
                ToolAnnotations::with_title("Search captured windows and notifications")
                    .read_only(true),
            ),
        ]
    }
}

/// The kind of a segment: a window, a dialog, or a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A window that the user looked at.
    Active,
    /// A dialog.
    Dialog,
    /// A window that stayed in the background.
    Background,
    /// A notification.
    Notification,
}

impl From<Kind> for SegmentKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Active => Self::Active,
            Kind::Dialog => Self::Dialog,
            Kind::Background => Self::Background,
            Kind::Notification => Self::Notification,
        }
    }
}

/// The segment kinds of `kinds` as the daemon's requests take them.
fn wire_kinds(kinds: impl IntoIterator<Item = Kind>) -> Vec<i32> {
    kinds.into_iter().map(|kind| SegmentKind::from(kind).into()).collect()
}

/// Returns the `map_err` function that turns an error of `action` into the tool's error.
///
/// For example, `failed("Reading the segment")` turns an error into "Reading the segment failed:"
/// followed by the error.
fn failed<E: fmt::Display>(action: &'static str) -> impl FnOnce(E) -> String {
    move |error| format!("{action} failed: {error}")
}

/// Writes `value` into a new string. A `Display` that fails leaves the text that it wrote
/// before the failure.
fn rendered(value: impl fmt::Display) -> String {
    let mut text = String::new();
    let _ = write!(text, "{value}");
    text
}

/// Reads the `from` and `to` arguments of a tool. A day as `to` reaches to the end of the day.
/// An absent `from` is `default_from`, and an absent `to` is now.
fn tool_range(
    from: Option<&str>,
    to: Option<&str>,
    default_from: Timestamp,
    now: &ZonedDateTime,
    settings: &Settings,
) -> Result<TimeRange, TimeRangeError> {
    TimeRange::parse(
        from.map(|text| GivenEnd { name: "from", text }),
        to.map(|text| GivenEnd { name: "to", text }),
        RangeEnd::Through,
        default_from,
        now,
        settings.dialect,
    )
}

/// Writes one line that names the segment: its id, application, kind, title, URL, and times.
struct SegmentLine<'a> {
    segment: &'a Segment,
    tz: &'a TimeZone,
}

impl fmt::Display for SegmentLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Segment {
            id,
            app,
            title,
            url,
            started_ms,
            ended_ms,
            ..
        } = self.segment;
        let kind = self.segment.kind().name();
        write!(f, "{id} | {app} | {kind} | {title:?} | ")?;
        if let Some(url) = url {
            write!(f, "{url} | ")?;
        }
        write!(f, "{}", UnixMillisDisplay::ymd_hm(*started_ms, self.tz))?;
        match ended_ms {
            Some(ended_ms) => {
                write!(f, " to {}", UnixMillisDisplay::ymd_hm(*ended_ms, self.tz))
            }
            None => f.write_str(" to now"),
        }
    }
}

/// What the tool descriptions say about the store and when to use the tools.
pub const DESCRIPTION_SCOPE: &str = "Desktop capture holds what the user's desktop showed: active \
                                     windows with their titles, URLs, and text, plus dialogs, \
                                     notifications, and windows that stayed in the background. \
                                     Use these tools when the user refers to something on screen, \
                                     or to something that they read, saw, or changed recently.";

#[cfg(test)]
mod tests {
    use atuin_daemon::grpc::desktop_capture::pb::SegmentKind;
    use rstest::rstest;

    use super::*;

    /// A call over the connection of a daemon that stopped since the last call says that the
    /// daemon has to run.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_stopped_daemon_gets_the_message_that_it_has_to_run() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let mut settings = Settings::utc();
        // The pidfile names the socket of a running daemon, so the test keeps one of its own.
        settings.daemon.pidfile_path = dir.path().join("atuin-daemon.pid").try_into().unwrap();
        settings.daemon.socket_path = Some(socket.clone().try_into().unwrap());
        settings.daemon.systemd_socket = false;
        let connect = DesktopCaptureClient::from_settings(&settings);
        let client = tokio::time::timeout(std::time::Duration::from_secs(5), connect);
        let client = client.await.unwrap().unwrap();
        drop(listener);
        fs_err::remove_file(&socket).unwrap();
        let tools = DesktopTools {
            client: Mutex::new(Some(client)),
        };
        let call = Call::Activity(serde_json::from_value(serde_json::json!({})).unwrap());

        let outcome = tools.run(call, &settings).await;

        assert_eq!(
            outcome,
            Err("desktop capture needs the Atuin daemon: run `atuin daemon start`".to_owned())
        );
    }

    /// A stop alone creates the store's directory, to hold the stop file, and doesn't make a
    /// store.
    #[rstest]
    #[case::with_the_store(&["db-1"], true)]
    #[case::with_only_a_stop(&["stopped"], false)]
    #[case::without_the_store(&[], false)]
    fn listed_only_when_the_store_exists(#[case] files: &[&str], #[case] listed: bool) {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::utc();
        settings.data_dir = dir.path().to_path_buf().try_into().unwrap();
        let store = settings.desktop_capture_dir();
        if !files.is_empty() {
            fs_err::create_dir_all(&store).unwrap();
        }
        for file in files {
            fs_err::write(store.join(file), "").unwrap();
        }

        assert_eq!(is_listed(&settings), listed);
    }

    const RECORD: &str = "018f0000-0000-7000-8000-000000000000";

    fn noon() -> ZonedDateTime {
        "2026-11-14T12:00:00Z".parse::<Timestamp>().unwrap().to_zoned_datetime(TimeZone::UTC)
    }

    /// A day as `to` reaches to the day's end, and errors name the tool's arguments.
    #[rstest]
    #[case::a_day_as_the_end(
        Some("yesterday"),
        Some("yesterday"),
        Ok(("2026-11-13T00:00:00Z", "2026-11-14T00:00:00Z"))
    )]
    #[case::a_date_as_the_end(
        None,
        Some("2026-11-01"),
        Ok(("1970-01-01T00:00:00Z", "2026-11-02T00:00:00Z"))
    )]
    #[case::an_unknown_time(None, Some("soon"), Err("to \"soon\": give an RFC 3339 time"))]
    #[case::backwards(Some("1h"), Some("2h"), Err("from is later than to"))]
    fn ranges_parse(
        #[case] from: Option<&str>,
        #[case] to: Option<&str>,
        #[case] expected: Result<(&str, &str), &str>,
    ) {
        let mut settings = Settings::utc();
        settings.dialect = atuin_client::settings::Dialect::Uk;
        let range = tool_range(from, to, Timestamp::UNIX_EPOCH, &noon(), &settings);
        match (range, expected) {
            (Ok(range), Ok((from, to))) => {
                assert_eq!(range.from, from.parse::<Timestamp>().unwrap());
                assert_eq!(range.to, Some(to.parse::<Timestamp>().unwrap()));
            }
            (Err(err), Err(start)) => assert!(err.to_string().starts_with(start), "{err}"),
            (Ok(range), Err(_)) => panic!("{range:?}"),
            (Err(err), Ok(_)) => panic!("{err}"),
        }
    }

    /// The kinds that the tools take have the names that the tools' own output gives them.
    #[rstest]
    #[case::active(Kind::Active)]
    #[case::dialog(Kind::Dialog)]
    #[case::background(Kind::Background)]
    #[case::notification(Kind::Notification)]
    fn kinds_have_the_names_of_the_output(#[case] kind: Kind) {
        let name = SegmentKind::from(kind).name();
        assert_eq!(serde_json::from_value::<Kind>(serde_json::json!(name)).unwrap(), kind);
    }

    /// A time outside the range of `Timestamp` still gives a whole line, since a failing `Display`
    /// would panic in `to_string`.
    #[rstest]
    fn a_segment_line_survives_a_time_out_of_range() {
        let segment = Segment {
            id: RECORD.to_owned(),
            app: "Settings".to_owned(),
            started_ms: 253_402_300_799_000,
            ended_ms: Some(i64::MAX),
            ..Segment::default()
        };
        let line = SegmentLine {
            segment: &segment,
            tz: &TimeZone::fixed(jiff::tz::Offset::constant(3)),
        }
        .to_string();
        assert!(line.ends_with("| --:-- to --:--"), "{line}");
    }

    #[rstest]
    #[case::open(None, "to now")]
    #[case::closed(Some(1_700_000_100_000), "to 2023-11-14 22:15")]
    fn a_segment_line_names_everything_needed_to_read_it(
        #[case] ended_ms: Option<i64>,
        #[case] ending: &str,
    ) {
        let segment = Segment {
            id: RECORD.to_owned(),
            kind: SegmentKind::Dialog.into(),
            app: "Settings".to_owned(),
            title: "Display".to_owned(),
            url: Some("https://example.com/".to_owned()),
            started_ms: 1_700_000_000_000,
            ended_ms,
            snapshots: 2,
        };
        let line = SegmentLine {
            segment: &segment,
            tz: &TimeZone::UTC,
        }
        .to_string();
        assert_eq!(
            line,
            format!(
                "{RECORD} | Settings | dialog | \"Display\" | https://example.com/ | 2023-11-14 \
                 22:13 {ending}"
            )
        );
    }
}
