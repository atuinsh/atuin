//! The tables and reports that `atuin desktop-capture` prints.

use std::io;

use atuin_daemon::grpc::desktop_capture::pb::{
    ListSnapshotsReply, ReadSegmentReply, Segment, SegmentKind, Snapshot, State, StatusReply,
    TallySegmentsReply,
};
use clap::ValueEnum;

use crate::i18n::{fl, writet};

/// Writes the state of capture and the `note` on it, then the store's counts when the daemon
/// knows them.
pub fn write_status(
    out: &mut impl io::Write,
    reply: &StatusReply,
    note: Option<&str>,
) -> io::Result<()> {
    write_state(out, reply)?;
    write_details(out, reply, note)
}

/// Writes what `atuin desktop-capture start` led to, as [`write_status`] does. A setting that
/// keeps capture off gets a line that names the setting.
pub fn write_start(
    out: &mut impl io::Write,
    reply: &StatusReply,
    note: Option<&str>,
) -> io::Result<()> {
    match reply.state() {
        State::Disabled => writeln!(out, "{}", fl!("desktop-capture-start-disabled"))?,
        State::NoApps => writeln!(out, "{}", fl!("desktop-capture-start-no-apps"))?,
        _ => write_state(out, reply)?,
    }
    write_details(out, reply, note)
}

/// Writes the line of the state of capture.
fn write_state(out: &mut impl io::Write, reply: &StatusReply) -> io::Result<()> {
    match reply.state() {
        State::Unspecified => writet!(out, "desktop-capture-state-unknown")?,
        State::Disabled => writet!(out, "desktop-capture-state-disabled")?,
        State::NoApps => writet!(out, "desktop-capture-state-no-apps")?,
        State::Stopped => writet!(out, "desktop-capture-state-stopped")?,
        State::Starting => writet!(out, "desktop-capture-state-starting")?,
        State::Running => writet!(out, "desktop-capture-state-running")?,
        State::NeedsPermission => writet!(out, "desktop-capture-state-needs-permission")?,
        State::NoDesktop => writet!(out, "desktop-capture-state-no-desktop")?,
        State::ScreenLocked => writet!(out, "desktop-capture-state-screen-locked")?,
        State::Failed => {
            writet!(out, "desktop-capture-state-failed", error = reply.error.as_str())?;
        }
    }
    writeln!(out)
}

/// Writes the lines under the state: the stop, the `note`, and the store's counts.
fn write_details(
    out: &mut impl io::Write,
    reply: &StatusReply,
    note: Option<&str>,
) -> io::Result<()> {
    if reply.stopped && reply.state() != State::Stopped {
        writeln!(out, "  {}", fl!("desktop-capture-status-stopped"))?;
    }
    if let Some(note) = note {
        writeln!(out, "  {note}")?;
    }
    if let Some(count) = reply.segments {
        writeln!(out, "  {}", fl!("desktop-capture-status-segments", count = count))?;
    }
    if let Some(count) = reply.records {
        writeln!(out, "  {}", fl!("desktop-capture-status-records", count = count))?;
    }
    Ok(())
}

/// Writes the id of the snapshot that `reply` holds, then its text.
pub fn write_snapshot(out: &mut impl io::Write, reply: &ReadSegmentReply) -> io::Result<()> {
    let record = reply.record_id.as_str();
    writeln!(out, "{}\n\n{}", fl!("desktop-capture-show-snapshot", record = record), reply.text)
}

/// Writes the segment of `reply` as `log` lists it, then one line for each of its snapshots.
pub fn write_snapshots(
    out: &mut impl io::Write,
    reply: &ListSnapshotsReply,
    now_ms: i64,
) -> io::Result<()> {
    write_log(out, &reply.segment, now_ms)?;
    let started_ms = reply.segment.as_ref().map_or(0, |segment| segment.started_ms);
    writeln!(
        out,
        "\n{:<36} {:>8} {:<13} {:>6}  {}",
        fl!("desktop-capture-column-id"),
        fl!("desktop-capture-column-secs-in"),
        fl!("desktop-capture-column-trigger"),
        fl!("desktop-capture-column-nodes"),
        fl!("desktop-capture-column-content"),
    )?;
    let same = fl!("desktop-capture-content-same");
    let changed = fl!("desktop-capture-content-changed");
    for snapshot in &reply.snapshots {
        let Snapshot {
            record_id,
            at_ms,
            trigger,
            nodes,
            same_as_previous,
        } = snapshot;
        let trigger = if trigger.is_empty() {
            "-"
        } else {
            trigger
        };
        let content = if *same_as_previous {
            &same
        } else {
            &changed
        };
        writeln!(
            out,
            "{record_id:<36} {:>8.1} {trigger:<13} {nodes:>6}  {content}",
            secs_between(started_ms, *at_ms),
        )?;
    }
    Ok(())
}

/// A segment kind that a command can select.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Kind {
    Active,
    Dialog,
    Background,
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

/// The localized names of the segment kinds, looked up once for a listing.
pub struct KindNames {
    unknown: String,
    active: String,
    dialog: String,
    background: String,
    notification: String,
}

impl KindNames {
    pub fn look_up() -> Self {
        Self {
            unknown: fl!("desktop-capture-kind-unknown"),
            active: fl!("desktop-capture-kind-active"),
            dialog: fl!("desktop-capture-kind-dialog"),
            background: fl!("desktop-capture-kind-background"),
            notification: fl!("desktop-capture-kind-notification"),
        }
    }

    pub fn get(&self, kind: SegmentKind) -> &str {
        match kind {
            SegmentKind::Unspecified => &self.unknown,
            SegmentKind::Active => &self.active,
            SegmentKind::Dialog => &self.dialog,
            SegmentKind::Background => &self.background,
            SegmentKind::Notification => &self.notification,
        }
    }

    /// The characters of the longest name.
    fn longest(&self) -> usize {
        [&self.unknown, &self.active, &self.dialog, &self.background, &self.notification]
            .into_iter()
            .map(|name| name.chars().count())
            .max()
            .unwrap_or_default()
    }
}

/// The seconds that `segment` lasted, or has lasted so far while it is open.
#[expect(clippy::cast_precision_loss, reason = "durations are far below 2^52 ms")]
fn duration_secs(segment: &Segment, now_ms: i64) -> f64 {
    let ended_ms = segment.ended_ms.unwrap_or(now_ms);
    ended_ms.saturating_sub(segment.started_ms) as f64 / 1000.0
}

/// The seconds from `from_ms` to `to_ms`.
#[expect(clippy::cast_precision_loss, reason = "the spans are far below 2^52 ms")]
fn secs_between(from_ms: i64, to_ms: i64) -> f64 {
    to_ms.saturating_sub(from_ms) as f64 / 1000.0
}

/// Writes one line for each of `segments`, under a header.
pub fn write_log<'a>(
    out: &mut impl io::Write,
    segments: impl IntoIterator<Item = &'a Segment>,
    now_ms: i64,
) -> io::Result<()> {
    let kinds = KindNames::look_up();
    let kind_header = fl!("desktop-capture-column-kind");
    let kind_width = kinds.longest().max(kind_header.chars().count());
    writeln!(
        out,
        "{:<36} {:>8} {kind_header:<kind_width$} {:<14} {:>5}  {}",
        fl!("desktop-capture-column-id"),
        fl!("desktop-capture-column-secs"),
        fl!("desktop-capture-column-app"),
        fl!("desktop-capture-column-snapshots"),
        fl!("desktop-capture-column-title"),
    )?;
    for segment in segments {
        write!(
            out,
            "{:<36} {:>8.1} {:<kind_width$} {:<14.14} {:>5}  {}",
            segment.id,
            duration_secs(segment, now_ms),
            kinds.get(segment.kind()),
            segment.app,
            segment.snapshots,
            segment.title,
        )?;
        if let Some(url) = segment.url.as_deref().filter(|url| !url.is_empty()) {
            write!(out, "  <{url}>")?;
        }
        writeln!(out)?;
    }
    Ok(())
}

/// What `stats` reports: the store's totals, and the activity of the last 24 hours.
pub struct Stats {
    pub status: StatusReply,
    /// The tally of the last 24 hours.
    pub recent: TallySegmentsReply,
}

/// Writes the store's totals, then the activity of `stats` with the totals of each application.
pub fn write_stats(out: &mut impl io::Write, stats: &Stats) -> io::Result<()> {
    let segments = stats.status.segments.map_or_else(
        || fl!("desktop-capture-stats-segments-unknown"),
        |count| fl!("desktop-capture-stats-segments", count = count),
    );
    let records = stats.status.records.map_or_else(
        || fl!("desktop-capture-stats-records-unknown"),
        |count| fl!("desktop-capture-stats-records", count = count),
    );
    writeln!(out, "{segments}\n{records}")?;

    writeln!(out, "\n{}", fl!("desktop-capture-stats-recent"))?;
    let apps = &stats.recent.apps;
    let count: u64 = apps.iter().map(|app| app.windows).sum();
    let notifications: u64 = apps.iter().map(|app| app.notifications).sum();
    writeln!(out, "  {}", fl!("desktop-capture-stats-recent-segments", count = count))?;
    writeln!(
        out,
        "  {}",
        fl!("desktop-capture-stats-recent-notifications", count = notifications)
    )?;

    // The busiest application comes first, and a tie keeps the order of the names.
    let mut windows: Vec<_> = apps.iter().filter(|app| app.windows > 0).collect();
    windows.sort_by_key(|app| std::cmp::Reverse(app.windows));

    writeln!(out, "\n{}", fl!("desktop-capture-stats-by-app"))?;
    writeln!(
        out,
        "  {:<14} {:>8} {:>9} {:>9}",
        fl!("desktop-capture-column-app"),
        fl!("desktop-capture-column-segments"),
        fl!("desktop-capture-column-snapshots-long"),
        fl!("desktop-capture-column-secs"),
    )?;
    for app in windows {
        writeln!(
            out,
            "  {:<14.14} {:>8} {:>9} {:>9.0}",
            app.app,
            app.windows,
            app.snapshots,
            secs_between(0, app.duration_ms)
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use atuin_daemon::grpc::desktop_capture::pb::AppTally;
    use rstest::rstest;

    use super::*;

    /// The kinds that the flags take have the names that the JSON output gives them.
    #[rstest]
    #[case::active(Kind::Active)]
    #[case::dialog(Kind::Dialog)]
    #[case::background(Kind::Background)]
    #[case::notification(Kind::Notification)]
    fn kinds_have_the_names_of_the_json_output(#[case] kind: Kind) {
        let value = kind.to_possible_value().unwrap();
        assert_eq!(value.get_name(), SegmentKind::from(kind).name());
    }

    fn segment(app: &str, started_ms: i64, ended_ms: Option<i64>) -> Segment {
        Segment {
            id: "01890000-0000-7000-8000-000000000001".to_owned(),
            kind: SegmentKind::Active.into(),
            app: app.to_owned(),
            title: "notes.txt".to_owned(),
            url: None,
            started_ms,
            ended_ms,
            snapshots: 3,
        }
    }

    fn written(write: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[rstest]
    #[case::closed(Some(4_500), "     3.5")]
    #[case::open_until_now(None, "     9.0")]
    fn log_lists_a_segment_with_its_duration(#[case] ended_ms: Option<i64>, #[case] secs: &str) {
        let mut segment = segment("gedit", 1_000, ended_ms);
        segment.url = Some("https://example.com/".to_owned());

        let log = written(|out| write_log(out, [&segment], 10_000));

        assert_eq!(
            log.lines().next(),
            Some(
                "id                                       secs kind         app            snaps  \
                 title"
            )
        );
        let line = log.lines().nth(1).unwrap();
        assert_eq!(
            line,
            format!(
                "01890000-0000-7000-8000-000000000001 {secs} active       gedit              3  \
                 notes.txt  <https://example.com/>"
            )
        );
    }

    #[rstest]
    fn log_aligns_the_columns_after_every_kind(
        #[values(
            SegmentKind::Active,
            SegmentKind::Dialog,
            SegmentKind::Background,
            SegmentKind::Notification
        )]
        kind: SegmentKind,
    ) {
        let mut segment = segment("gedit", 1_000, Some(2_000));
        segment.kind = kind.into();

        let log = written(|out| write_log(out, [&segment], 10_000));

        let mut lines = log.lines();
        let header = lines.next().unwrap();
        let line = lines.next().unwrap();
        assert_eq!(line.find(" gedit ").map(|at| at + 1), header.find(" app ").map(|at| at + 1));
    }

    #[rstest]
    fn show_names_the_snapshot_before_its_text() {
        let reply = ReadSegmentReply {
            text: "line one\nline two".to_owned(),
            record_id: "01890000-0000-7000-8000-000000000002".to_owned(),
            ..ReadSegmentReply::default()
        };

        let shown = written(|out| write_snapshot(out, &reply));

        assert_eq!(shown, "Snapshot 01890000-0000-7000-8000-000000000002\n\nline one\nline two\n");
    }

    #[rstest]
    fn snapshots_follow_their_segment() {
        let snapshot = |record_id: &str, at_ms, trigger: &str, same_as_previous| Snapshot {
            record_id: record_id.to_owned(),
            at_ms,
            trigger: trigger.to_owned(),
            nodes: 42,
            same_as_previous,
        };
        let reply = ListSnapshotsReply {
            segment: Some(segment("gedit", 1_000, Some(4_500))),
            snapshots: vec![
                snapshot("01890000-0000-7000-8000-000000000002", 1_000, "activated", false),
                snapshot("01890000-0000-7000-8000-000000000003", 3_250, "polled", true),
                snapshot("01890000-0000-7000-8000-000000000004", 4_500, "", false),
            ],
        };

        let listed = written(|out| write_snapshots(out, &reply, 10_000));

        let snapshots = listed.split("\n\n").nth(1).unwrap();
        assert_eq!(
            snapshots,
            "id                                    secs in trigger        nodes  \
             content\n01890000-0000-7000-8000-000000000002      0.0 activated         42  \
             changed\n01890000-0000-7000-8000-000000000003      2.2 polled            42  \
             same\n01890000-0000-7000-8000-000000000004      3.5 -                 42  changed\n"
        );
        assert!(listed.starts_with("id "), "{listed}");
        assert!(listed.lines().nth(1).unwrap().contains("gedit"), "{listed}");
    }

    #[rstest]
    fn status_shows_the_counts() {
        let reply = StatusReply {
            state: State::Running.into(),
            segments: Some(4),
            records: Some(11),
            ..StatusReply::default()
        };

        let status = written(|out| write_status(out, &reply, None));

        assert_eq!(
            status,
            "Desktop capture: running\n  Segments:     4\n  Records:      about 11\n"
        );
    }

    #[rstest]
    #[case::no_apps_stopped(State::NoApps, true, true)]
    #[case::no_apps_running(State::NoApps, false, false)]
    #[case::disabled_stopped(State::Disabled, true, true)]
    #[case::stopped_state(State::Stopped, true, false)]
    fn status_shows_a_stop_unless_the_state_says_it(
        #[case] state: State,
        #[case] stopped: bool,
        #[case] shown: bool,
    ) {
        let reply = StatusReply {
            state: state.into(),
            stopped,
            ..StatusReply::default()
        };

        let status = written(|out| write_status(out, &reply, None));

        let line = "  Stopped: capture stays off until you run `atuin desktop-capture start`.";
        assert_eq!(status.lines().nth(1) == Some(line), shown, "{status}");
    }

    /// A start that a setting keeps from capturing names the setting, and any other start shows
    /// the state as `status` does.
    #[rstest]
    #[case::disabled(State::Disabled, "`enabled = true`")]
    #[case::no_apps(State::NoApps, "`apps`")]
    #[case::running(State::Running, "Desktop capture: running")]
    #[case::no_desktop(State::NoDesktop, "Desktop capture: waiting for a desktop session")]
    #[case::screen_locked(
        State::ScreenLocked,
        "Desktop capture: paused while the screen is locked"
    )]
    fn start_names_the_setting_that_keeps_capture_off(#[case] state: State, #[case] shown: &str) {
        let reply = StatusReply {
            state: state.into(),
            segments: Some(4),
            ..StatusReply::default()
        };

        let started = written(|out| write_start(out, &reply, None));

        let lines: Vec<_> = started.lines().collect();
        assert!(lines[0].contains(shown), "{started}");
        assert_eq!(lines[1..], ["  Segments:     4"]);
        let configured = matches!(state, State::Disabled | State::NoApps);
        assert_eq!(lines[0].contains("atuin setup"), configured, "{started}");
    }

    #[rstest]
    fn status_puts_the_note_under_the_state() {
        let reply = StatusReply {
            state: State::NeedsPermission.into(),
            segments: Some(4),
            ..StatusReply::default()
        };

        let status = written(|out| write_status(out, &reply, Some("A note.")));

        let lines: Vec<_> = status.lines().collect();
        assert_eq!(lines[1..], ["  A note.", "  Segments:     4"]);
    }

    fn tally(app: &str, windows: u64, notifications: u64, snapshots: u64, secs: i64) -> AppTally {
        AppTally {
            app: app.to_owned(),
            windows,
            notifications,
            snapshots,
            duration_ms: secs * 1000,
        }
    }

    #[rstest]
    fn stats_list_the_busiest_app_first() {
        let stats = Stats {
            status: StatusReply {
                segments: Some(7),
                records: None,
                ..StatusReply::default()
            },
            recent: TallySegmentsReply {
                apps: vec![tally("gedit", 2, 0, 6, 4), tally("kate", 1, 0, 3, 2)],
                ..TallySegmentsReply::default()
            },
        };

        let report = written(|out| write_stats(out, &stats));

        assert!(report.contains("segments:      7\nrecords:       unknown\n"), "{report}");
        assert!(report.contains("  segments:      3\n  notifications: 0\n"), "{report}");
        let by_app = report.split("by application:\n").nth(1).unwrap();
        assert_eq!(
            by_app,
            "  app            segments snapshots      secs\n  gedit                 2         6         4\n  kate                  1         3         2\n"
        );
    }

    #[rstest]
    fn stats_count_notifications_apart() {
        let stats = Stats {
            status: StatusReply::default(),
            recent: TallySegmentsReply {
                apps: vec![tally("Backup", 0, 1, 0, 0), tally("gedit", 2, 0, 6, 4)],
                ..TallySegmentsReply::default()
            },
        };

        let report = written(|out| write_stats(out, &stats));

        assert!(report.contains("  segments:      2\n  notifications: 1\n"), "{report}");
        let by_app = report.split("by application:\n").nth(1).unwrap();
        assert!(by_app.contains("gedit"), "{report}");
        assert!(!by_app.contains("Backup"), "{report}");
    }
}
