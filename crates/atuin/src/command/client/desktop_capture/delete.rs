//! `atuin desktop-capture delete`: deletes the captured windows and notifications that a filter
//! selects.

use std::io::{self, BufRead, IsTerminal as _, Write};

use atuin_client::settings::Settings;
use atuin_common::time::UnixMillisDisplay;
use atuin_common::userio::confirm::{Answer, confirm};
use atuin_daemon::grpc::desktop_capture::pb::{
    DeleteFilter, DeleteReply, Segment, TallySegmentsReply,
};
use atuin_daemon::{DaemonStatusError, DesktopCaptureClient, Tallied};
use clap::{ArgGroup, Args};
use eyre::Result;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use uuid::Uuid;

use super::render::KindNames;
use super::{Daemon, flag_range};
use crate::error::Refusal;
use crate::i18n::fl;

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("selection")
        .required(true)
        .multiple(true)
        .args(["app", "after", "before", "all", "id"])
))]
pub struct Delete {
    #[arg(long, help = fl!("arg-desktop-capture-delete-app"))]
    app: Option<String>,

    #[arg(long, help = fl!("arg-desktop-capture-delete-after"))]
    after: Option<String>,

    #[arg(long, help = fl!("arg-desktop-capture-delete-before"))]
    before: Option<String>,

    #[arg(
        long,
        conflicts_with_all = ["app", "after", "before"],
        help = fl!("arg-desktop-capture-delete-all")
    )]
    all: bool,

    #[arg(
        long,
        value_name = "ID",
        conflicts_with_all = ["app", "after", "before", "all"],
        help = fl!("arg-desktop-capture-delete-id")
    )]
    id: Option<Uuid>,

    #[arg(long, help = fl!("arg-desktop-capture-delete-yes"))]
    yes: bool,
}

/// What a deletion asks the daemon to delete.
#[derive(Debug)]
enum Deletion {
    Id(Uuid),
    Matching(DeleteFilter),
    Everything,
}

impl Deletion {
    async fn send(
        self,
        client: &mut DesktopCaptureClient,
    ) -> Result<DeleteReply, DaemonStatusError> {
        match self {
            Self::Id(id) => client.delete_id(id).await,
            Self::Matching(filter) => client.delete_matching(filter).await,
            Self::Everything => client.delete_everything().await,
        }
    }
}

impl Delete {
    pub async fn run(mut self, settings: &Settings) -> Result<()> {
        let tz = settings.timezone.time_zone();
        if let Some(id) = self.id {
            let mut daemon = Daemon::connect(settings).await?;
            return self.delete_id(&mut daemon, id, &tz).await;
        }
        let now = Timestamp::now().to_zoned_datetime(tz.clone());
        let range = flag_range(self.after.as_deref(), self.before.as_deref(), &now, settings)?;
        let mut daemon = Daemon::connect(settings).await?;
        // Without `--before`, the deletion reaches up to the moment of the deletion. The deletion
        // then includes what capture stores while the question waits for an answer.
        let app = self.app.take();
        let tally = Tallied {
            from_ms: range.start_ms(),
            to_ms: range.to.map(Timestamp::as_millisecond),
            app: app.clone(),
        };
        let deletion = if self.all {
            Deletion::Everything
        } else {
            Deletion::Matching(DeleteFilter {
                app,
                from_ms: range.start_ms(),
                to_ms: range.to.map(Timestamp::as_millisecond),
            })
        };

        let reply = daemon.request(async |client| client.tally_segments(tally).await).await?;
        let Some(preview) = Preview::of(&reply) else {
            writeln!(io::stdout(), "{}", fl!("desktop-capture-delete-nothing"))?;
            return Ok(());
        };
        preview.write(&mut io::stdout().lock(), &tz)?;
        let question = fl!("desktop-capture-delete-confirm");
        self.delete_after_asking(&mut daemon, deletion, question).await
    }

    /// Deletes the segment that `id` names, after showing it and asking.
    async fn delete_id(self, daemon: &mut Daemon, id: Uuid, tz: &TimeZone) -> Result<()> {
        let segment = daemon
            .request(async |client| match client.lookup(id).await {
                Ok(reply) => Ok(reply.segment),
                Err(err) if err.is_not_found() => Ok(None),
                Err(err) => Err(err),
            })
            .await?;
        let Some(preview) = segment.map(IdPreview) else {
            writeln!(io::stdout(), "{}", fl!("desktop-capture-delete-nothing"))?;
            return Ok(());
        };
        preview.write(&mut io::stdout().lock(), tz)?;
        let question = fl!("desktop-capture-delete-confirm-one");
        self.delete_after_asking(daemon, Deletion::Id(id), question).await
    }

    /// Sends `deletion` unless the user declines `question`, which `--yes` skips.
    async fn delete_after_asking(
        &self,
        daemon: &mut Daemon,
        deletion: Deletion,
        question: String,
    ) -> Result<()> {
        if !self.yes {
            let stdin = io::stdin();
            let input = stdin.is_terminal().then(|| stdin.lock());
            if confirm_deletion(input, &mut io::stdout().lock(), &question)? == Answer::No {
                writeln!(io::stdout(), "{}", fl!("desktop-capture-delete-declined"))?;
                return Ok(());
            }
        }

        let reply = daemon.request(async |client| deletion.send(client).await).await?;
        write_done(&mut io::stdout().lock(), reply)?;
        Ok(())
    }
}

/// The one segment that a deletion by id is about to delete.
#[derive(Debug, Clone, PartialEq)]
struct IdPreview(Segment);

impl IdPreview {
    fn write(&self, out: &mut impl Write, tz: &TimeZone) -> io::Result<()> {
        let Self(segment) = self;
        let from = UnixMillisDisplay::ymd_hm(segment.started_ms, tz).to_string();
        let kinds = KindNames::look_up();
        let kind = kinds.get(segment.kind());
        let text = match segment.ended_ms {
            Some(ended_ms) => fl!(
                "desktop-capture-delete-preview-segment",
                id = segment.id.as_str(),
                kind = kind,
                app = segment.app.as_str(),
                title = segment.title.as_str(),
                from = from,
                to = UnixMillisDisplay::ymd_hm(ended_ms, tz).to_string()
            ),
            None => fl!(
                "desktop-capture-delete-preview-open-segment",
                id = segment.id.as_str(),
                kind = kind,
                app = segment.app.as_str(),
                title = segment.title.as_str(),
                from = from
            ),
        };
        writeln!(out, "{text}")
    }
}

/// What a deletion is about to delete: how many segments and notifications, the times that
/// they span, and how many damaged segments and records the deletion also removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Preview {
    segments: u64,
    notifications: u64,
    /// `None` when the deletion removes only damaged segments and records.
    span: Option<SpanMs>,
    damaged: u64,
}

/// The times that the segments of a [`Preview`] span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpanMs {
    from_ms: i64,
    to_ms: i64,
}

impl Preview {
    /// The preview of the tally `reply`, which counts the notification segments apart from the
    /// others. `None` when the tally doesn't hold any segment or damaged record.
    fn of(reply: &TallySegmentsReply) -> Option<Self> {
        let span = reply.from_ms.zip(reply.to_ms).map(|(from_ms, to_ms)| SpanMs { from_ms, to_ms });
        if span.is_none() && reply.damaged == 0 {
            return None;
        }
        let mut preview = Self {
            segments: 0,
            notifications: 0,
            span,
            damaged: reply.damaged,
        };
        for app in &reply.apps {
            preview.segments += app.windows;
            preview.notifications += app.notifications;
        }
        Some(preview)
    }

    fn write(&self, out: &mut impl Write, tz: &TimeZone) -> io::Result<()> {
        if let Some(SpanMs { from_ms, to_ms }) = self.span {
            let text = fl!(
                "desktop-capture-delete-preview",
                segments = self.segments,
                notifications = self.notifications,
                from = UnixMillisDisplay::ymd_hm(from_ms, tz).to_string(),
                to = UnixMillisDisplay::ymd_hm(to_ms, tz).to_string()
            );
            writeln!(out, "{text}")?;
        }
        if self.damaged > 0 {
            let text = fl!("desktop-capture-delete-preview-damaged", damaged = self.damaged);
            writeln!(out, "{text}")?;
        }
        Ok(())
    }
}

/// Warns about the windows that are still open, and asks `question` on `input`. `None`
/// for `input` means that there is no terminal to ask, which fails. An empty answer or an
/// ended input declines.
fn confirm_deletion(
    input: Option<impl BufRead>,
    out: &mut impl Write,
    question: &str,
) -> Result<Answer> {
    let Some(input) = input else {
        return Err(Refusal(fl!("desktop-capture-delete-needs-terminal")).into());
    };
    writeln!(out, "{}", fl!("desktop-capture-delete-open-windows"))?;
    Ok(confirm(input, out, question, Answer::No)?)
}

/// Writes the counts that the deletion reports, a warning about the damaged records that it
/// kept, and what the deletion leaves behind.
fn write_done(out: &mut impl Write, reply: DeleteReply) -> io::Result<()> {
    writeln!(out, "{}", fl!("desktop-capture-delete-done", segments = reply.segments))?;
    if reply.orphans > 0 {
        writeln!(out, "{}", fl!("desktop-capture-delete-done-orphans", orphans = reply.orphans))?;
    }
    writeln!(out)?;
    if reply.undecided > 0 {
        let warning = fl!("desktop-capture-delete-undecided", undecided = reply.undecided);
        writeln!(out, "{warning}\n")?;
    }
    writeln!(out, "{}", fl!("desktop-capture-delete-notes"))
}

#[cfg(test)]
mod tests {
    use atuin_daemon::grpc::desktop_capture::pb::{AppTally, SegmentKind};
    use clap::Parser;
    use rstest::rstest;

    use super::super::Cmd;
    use super::*;

    #[derive(Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("desktop-capture").chain(args.iter().copied()))
    }

    #[rstest]
    #[case::app(&["delete", "--app", "firefox"])]
    #[case::range(&["delete", "--after", "1 hour ago"])]
    #[case::before(&["delete", "--before", "yesterday"])]
    #[case::app_and_range(&["delete", "--app", "firefox", "--after", "1 hour ago", "--before", "now"])]
    #[case::all(&["delete", "--all"])]
    #[case::all_yes(&["delete", "--all", "--yes"])]
    #[case::id(&["delete", "--id", "018f0000-0000-7000-8000-000000000001"])]
    #[case::id_yes(&["delete", "--id", "018f0000-0000-7000-8000-000000000001", "--yes"])]
    fn accepted_selections(#[case] args: &[&str]) {
        let cli = parse(args).unwrap();
        assert!(matches!(cli.cmd, Cmd::Delete(_)));
    }

    const ID: &str = "018f0000-0000-7000-8000-000000000001";

    #[rstest]
    #[case::nothing(&["delete"])]
    #[case::only_yes(&["delete", "--yes"])]
    #[case::all_with_app(&["delete", "--all", "--app", "firefox"])]
    #[case::all_with_after(&["delete", "--all", "--after", "1 hour ago"])]
    #[case::all_with_before(&["delete", "--all", "--before", "now"])]
    #[case::id_with_app(&["delete", "--id", ID, "--app", "firefox"])]
    #[case::id_with_after(&["delete", "--id", ID, "--after", "1 hour ago"])]
    #[case::id_with_before(&["delete", "--id", ID, "--before", "now"])]
    #[case::id_with_all(&["delete", "--id", ID, "--all"])]
    #[case::id_not_a_uuid(&["delete", "--id", "firefox"])]
    fn rejected_selections(#[case] args: &[&str]) {
        parse(args).unwrap_err();
    }

    fn segment(started_ms: i64, ended_ms: Option<i64>) -> Segment {
        Segment {
            id: "018f0000-0000-7000-8000-000000000001".to_owned(),
            app: "firefox".to_owned(),
            title: "Release notes".to_owned(),
            started_ms,
            ended_ms,
            ..Segment::default()
        }
    }

    const HOUR_MS: i64 = 60 * 60 * 1000;
    /// 2023-11-14 22:13 UTC.
    const AT_MS: i64 = 1_700_000_000_000;

    fn written(write: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn tally(app: &str, windows: u64, notifications: u64) -> AppTally {
        AppTally {
            app: app.to_owned(),
            windows,
            notifications,
            ..AppTally::default()
        }
    }

    #[rstest]
    fn the_preview_adds_up_the_applications_and_counts_notifications_apart() {
        let reply = TallySegmentsReply {
            apps: vec![tally("firefox", 2, 1), tally("gedit", 1, 0)],
            from_ms: Some(AT_MS),
            to_ms: Some(AT_MS + 4 * HOUR_MS),
            damaged: 0,
        };
        let preview = Preview::of(&reply).unwrap();

        let text = written(|out| preview.write(out, &TimeZone::UTC));

        assert_eq!(
            text,
            "Found 3 window segments and 1 notification, from 2023-11-14 22:13 to 2023-11-15 \
             02:13.\n"
        );
    }

    #[rstest]
    #[case::dialog(SegmentKind::Dialog, "dialog")]
    #[case::background(SegmentKind::Background, "background")]
    #[case::active(SegmentKind::Active, "active")]
    #[case::notification(SegmentKind::Notification, "notification")]
    fn the_preview_of_a_segment_names_it_and_its_kind(
        #[case] kind: SegmentKind,
        #[case] kind_name: &str,
        #[values(Some(AT_MS + HOUR_MS), None)] ended_ms: Option<i64>,
    ) {
        let mut segment = segment(AT_MS, ended_ms);
        segment.set_kind(kind);
        let preview = IdPreview(segment);

        let text = written(|out| preview.write(out, &TimeZone::UTC));

        let kind_phrase = format!("the {kind_name} segment");
        for expected in [kind_phrase.as_str(), "firefox", "Release notes", "2023-11-14 22:13"] {
            assert!(text.contains(expected), "{text}");
        }
    }

    #[rstest]
    fn nothing_makes_no_preview() {
        assert!(Preview::of(&TallySegmentsReply::default()).is_none());
    }

    /// Damaged records alone make a preview, so a deletion of everything removes the damage
    /// that a deletion by application leaves.
    #[rstest]
    #[case::only_damage(None, "Found 2 damaged segments or records")]
    #[case::segments_and_damage(Some(AT_MS), "Found 1 window segment and 0 notifications")]
    fn damage_makes_a_preview(#[case] from_ms: Option<i64>, #[case] first_line: &str) {
        let reply = TallySegmentsReply {
            apps: from_ms.map(|_| tally("gedit", 1, 0)).into_iter().collect(),
            from_ms,
            to_ms: from_ms,
            damaged: 2,
        };
        let preview = Preview::of(&reply).unwrap();

        let text = written(|out| preview.write(out, &TimeZone::UTC));

        assert!(text.starts_with(first_line), "{text}");
        assert!(text.contains("Found 2 damaged segments or records"), "{text}");
    }

    #[rstest]
    #[case::yes("y\n", Answer::Yes)]
    #[case::yes_word("Yes\n", Answer::Yes)]
    #[case::empty("\n", Answer::No)]
    #[case::ended("", Answer::No)]
    #[case::other("nope\n", Answer::No)]
    fn the_question_defaults_to_no(#[case] input: &str, #[case] expected: Answer) {
        let mut out = Vec::new();
        let answer = confirm_deletion(Some(input.as_bytes()), &mut out, "Delete them?").unwrap();
        assert_eq!(answer, expected);
        assert!(String::from_utf8(out).unwrap().contains("Delete them? [y/N]"));
    }

    #[rstest]
    fn without_a_terminal_the_question_fails() {
        let mut out = Vec::new();
        let error = confirm_deletion(None::<&[u8]>, &mut out, "Delete them?").unwrap_err();
        assert!(error.to_string().contains("--yes"), "{error}");
        assert!(error.is::<Refusal>(), "{error:?}");
        assert!(out.is_empty());
    }

    #[rstest]
    #[case::one(1, "Deleted 1 segment.")]
    #[case::several(3, "Deleted 3 segments.")]
    fn the_result_names_the_count_and_what_it_leaves_behind(
        #[case] segments: u64,
        #[case] first_line: &str,
    ) {
        let reply = DeleteReply {
            segments,
            undecided: 0,
            orphans: 0,
        };

        let text = written(|out| write_done(out, reply));

        assert!(text.starts_with(&format!("{first_line}\n\n")), "{text}");
        assert!(text.contains("captured again"), "{text}");
        assert!(text.contains("agent threads"), "{text}");
        assert!(!text.contains("Warning"), "{text}");
    }

    /// The damaged records that a deletion by application keeps get a warning.
    #[rstest]
    #[case::one(1, "1 damaged segment or record stayed")]
    #[case::several(2, "2 damaged segments or records stayed")]
    fn the_result_warns_about_what_damage_kept(#[case] undecided: u64, #[case] warning: &str) {
        let reply = DeleteReply {
            segments: 3,
            undecided,
            orphans: 0,
        };

        let text = written(|out| write_done(out, reply));

        assert!(text.contains(&format!("Warning: {warning}")), "{text}");
        assert!(text.contains("delete --all"), "{text}");
        assert!(text.contains("captured again"), "{text}");
    }

    #[rstest]
    fn the_result_counts_the_damaged_records_that_it_removed() {
        let reply = DeleteReply {
            segments: 0,
            undecided: 0,
            orphans: 2,
        };

        let text = written(|out| write_done(out, reply));

        assert!(text.starts_with("Deleted 0 segments.\nDeleted 2 damaged records"), "{text}");
    }
}
