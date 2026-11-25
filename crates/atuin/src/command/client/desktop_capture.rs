//! `atuin desktop-capture`: controls desktop capture and reads what it stored, through the
//! daemon.

mod apps;
mod conflict;
mod delete;
mod hiding;
mod json;
mod render;
mod report;
mod search;
mod session;
mod setup;
mod toolkit;

use std::ffi::OsStr;
use std::io::{self, BufWriter, Write as _};
use std::iter;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use atuin_client::settings::{GivenEnd, RangeEnd, Settings, TimeRange, TimeRangeError};
use atuin_common::json::framed::Framing;
use atuin_common::time::SystemTimeExt as _;
use atuin_daemon::grpc::desktop_capture::pb::{
    ListSegmentsRequest, SegmentKind, State, StatusReply,
};
use atuin_daemon::service::{ActiveService, Launching};
use atuin_daemon::{
    DaemonStatusError, DesktopCaptureClient, Requested, Signing, SnapshotChoice, StopFile, Tallied,
};
use clap::{Args, Subcommand, ValueEnum};
use eyre::Result;
use jiff::{Timestamp, ZonedDateTime};
use uuid::Uuid;

use self::apps::AppsCmd;
pub(super) use self::conflict::Conflict;
use self::conflict::OtherDaemon;
use self::delete::Delete;
use self::render::{
    Kind, Stats, write_log, write_snapshot, write_snapshots, write_start, write_stats, write_status,
};
pub(super) use self::report::{Report, report};
use self::search::Search;
pub use self::session::Autostart;
use self::session::LoginShell;
use self::setup::run_helper;
pub(super) use self::setup::{
    CertificateOutcome, HelperAccess, Logins, Notes, ensure_certificate, helper_access,
    helper_name, request_access,
};
#[cfg(feature = "ai")]
pub(super) use self::setup::{PromptHookRemoval, ensure_prompt_hook, remove_prompt_hook};
use crate::error::Refusal;
use crate::i18n::fl;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-desktop-capture-status"))]
    Status,

    #[command(about = fl!("cmd-desktop-capture-stop"))]
    Stop,

    #[command(about = fl!("cmd-desktop-capture-start"))]
    Start,

    #[command(about = fl!("cmd-desktop-capture-log"))]
    Log {
        #[arg(long, default_value_t = 30, help = fl!("arg-desktop-capture-log-limit"))]
        limit: usize,

        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            help = fl!("arg-desktop-capture-log-kind")
        )]
        kind: Vec<Kind>,

        #[arg(
            long,
            value_enum,
            default_value_t = ListStyle::Table,
            help = fl!("arg-desktop-capture-list-style")
        )]
        style: ListStyle,
    },

    #[command(about = fl!("cmd-desktop-capture-search"))]
    Search(Search),

    #[command(about = fl!("cmd-desktop-capture-delete"))]
    Delete(Delete),

    #[command(about = fl!("cmd-desktop-capture-show"))]
    Show {
        #[arg(help = fl!("arg-desktop-capture-show-id"))]
        id: Uuid,

        #[command(flatten)]
        snapshot: SnapshotFlags,
    },

    #[command(about = fl!("cmd-desktop-capture-snapshots"))]
    Snapshots {
        #[arg(help = fl!("arg-desktop-capture-show-segment"))]
        id: Uuid,

        #[arg(
            long,
            value_enum,
            default_value_t = ListStyle::Table,
            help = fl!("arg-desktop-capture-list-style")
        )]
        style: ListStyle,
    },

    #[command(about = fl!("cmd-desktop-capture-stats"))]
    Stats,

    #[command(about = fl!("cmd-desktop-capture-apps"))]
    Apps(AppsCmd),

    /// Starts capture at the start of a session. Each login file that setup writes runs this
    /// command.
    #[command(hide = true)]
    Login {
        /// Print the session's variables in the syntax of this shell.
        #[arg(long, value_enum)]
        shell: Option<LoginShell>,

        /// Say that this runs inside a desktop session that exists now, so that a running daemon
        /// tries to capture at once. The autostart entry that setup writes passes this flag.
        #[arg(long)]
        in_desktop_session: bool,
    },

    #[command(about = fl!("cmd-desktop-capture-doctor"))]
    Doctor {
        #[arg(long, value_name = "APP", help = fl!("arg-desktop-capture-doctor-outline"))]
        outline: Option<String>,
    },
}

/// How `log` and `snapshots` print their listings.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ListStyle {
    #[value(help = fl!("value-desktop-capture-list-style-table"))]
    Table,
    #[value(help = fl!("value-desktop-capture-list-style-json"))]
    Json,
    #[value(help = fl!("value-desktop-capture-list-style-ndjson"))]
    Ndjson,
}

impl ListStyle {
    /// The framing of a JSON style. `None` for the table.
    const fn framing(self) -> Option<Framing> {
        match self {
            Self::Table => None,
            Self::Json => Some(Framing::Array),
            Self::Ndjson => Some(Framing::Lines),
        }
    }
}

/// The flags of `show` that choose a snapshot of the segment.
#[derive(Args, Debug)]
pub struct SnapshotFlags {
    #[arg(long, help = fl!("arg-desktop-capture-show-first"))]
    first: bool,

    #[arg(long, conflicts_with = "first", help = fl!("arg-desktop-capture-show-snapshot"))]
    snapshot: Option<Uuid>,
}

impl SnapshotFlags {
    /// The snapshot that the flags choose. `None` without either flag, when the id of `show`
    /// decides.
    fn choice(self) -> Option<SnapshotChoice> {
        match self {
            Self {
                snapshot: Some(id), ..
            } => Some(SnapshotChoice::Record(id)),
            Self {
                first: true,
                snapshot: None,
            } => Some(SnapshotChoice::First),
            Self {
                first: false,
                snapshot: None,
            } => None,
        }
    }
}

/// How far back `stats` looks for its per-application totals.
const STATS_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

impl Cmd {
    /// Whether the command is `login`, which a session runs at its start.
    pub const fn is_login(&self) -> bool {
        matches!(self, Self::Login { .. })
    }

    pub async fn run(self, settings: &Settings) -> Result<()> {
        // A reader such as `head` that stops early closes the pipe, which ends the listing.
        self.dispatch(settings).await.or_else(|error| match error.downcast_ref::<io::Error>() {
            Some(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            _ => Err(error),
        })
    }

    async fn dispatch(self, settings: &Settings) -> Result<()> {
        let now_ms = SystemTime::now().saturating_unix_millis();
        match self {
            Self::Status => status(settings).await,
            Self::Stop => stop_capture(settings).await,
            Self::Start => start_capture(settings).await,
            Self::Log { limit, kind, style } => {
                log_segments(settings, limit, &kind, style, now_ms).await
            }
            Self::Search(search) => search.run(settings).await,
            Self::Delete(delete) => delete.run(settings).await,
            Self::Show { id, snapshot } => show(settings, id, snapshot.choice()).await,
            Self::Snapshots { id, style } => snapshots(settings, id, style, now_ms).await,
            Self::Stats => stats(settings, now_ms).await,
            Self::Apps(apps) => apps.run(settings).await,
            Self::Login {
                shell,
                in_desktop_session,
            } => {
                let site = if in_desktop_session {
                    LoginSite::DesktopSession
                } else {
                    LoginSite::BeforeDesktop
                };
                login(settings, shell, site).await;
                Ok(())
            }
            Self::Doctor { outline } => doctor(settings, outline.as_deref()).await,
        }
    }
}

/// Where `atuin desktop-capture login` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginSite {
    /// Inside a desktop session that exists now, as the call of the autostart entry does.
    DesktopSession,
    /// At the start of a login, as the calls of the login files do. A desktop session, if any,
    /// starts later, and the daemon's session watch sees its compositor.
    BeforeDesktop,
}

/// Starts capture at the start of a session, and prints the session's variables for `shell`. A
/// daemon that is already running hears of the desktop session when `site` is inside one.
///
/// Every login shell runs `login`, so `login` doesn't open a database, and doesn't print anything
/// when it fails.
async fn login(settings: &Settings, shell: Option<LoginShell>, site: LoginSite) {
    if !settings.desktop_capture.enabled {
        return;
    }
    // A daemon that the user turned off stays off, and the variables then do no good either.
    if !settings.daemon.enabled {
        return;
    }
    if let Some(logins) = Logins::for_platform() {
        let _ = logins.refresh();
    }
    let launch = if settings.daemon.autostart {
        ActiveService::launch_unless_running(&settings.daemon).await.ok()
    } else {
        None
    };
    if let Some(shell) = shell {
        let _ = session::write_variables(&mut io::stdout().lock(), shell);
    }
    // A daemon that just launched starts a helper at once.
    if site == LoginSite::DesktopSession && launch != Some(Launching::Launched) {
        let _ = tokio::time::timeout(LOGIN_REPORT_TIMEOUT, report_session(settings)).await;
    }
}

/// How long `login` waits for the daemon to take in the report of the session.
const LOGIN_REPORT_TIMEOUT: Duration = Duration::from_secs(1);

/// Tells a running daemon that a desktop session started, so that capture resumes at once when
/// capture is waiting for one.
///
/// The report goes only to a daemon of this build, because the daemon refuses the requests of
/// other builds, and the login doesn't replace a daemon.
async fn report_session(settings: &Settings) -> Result<()> {
    DesktopCaptureClient::from_settings(settings).await?.session_started().await?;
    Ok(())
}

/// Prints the state of capture.
async fn status(settings: &Settings) -> Result<()> {
    let reply = with_daemon(settings, async |client| client.status().await).await?;
    let note = permission_note(&reply).await;
    write_status(&mut io::stdout().lock(), &reply, note.as_deref())?;
    Ok(())
}

/// Prints the latest `limit` segments of the kinds `kinds`, or of every kind when `kinds` is
/// empty.
async fn log_segments(
    settings: &Settings,
    limit: usize,
    kinds: &[Kind],
    style: ListStyle,
    now_ms: i64,
) -> Result<()> {
    let request = ListSegmentsRequest {
        limit: Some(u32::try_from(limit).unwrap_or(u32::MAX)),
        kinds: kinds.iter().map(|&kind| SegmentKind::from(kind).into()).collect(),
        ..ListSegmentsRequest::default()
    };
    let reply = with_daemon(settings, async |client| client.list_segments(request).await).await?;
    let mut out = BufWriter::new(io::stdout().lock());
    match style.framing() {
        Some(framing) => json::write_segments(&mut out, &reply.segments, framing)?,
        None => write_log(&mut out, &reply.segments, now_ms)?,
    }
    out.flush()?;
    Ok(())
}

/// Prints the `choice` snapshot of the segment `id`. Without `choice`, prints the snapshot `id`,
/// or the latest snapshot of the segment `id`.
async fn show(settings: &Settings, id: Uuid, choice: Option<SnapshotChoice>) -> Result<()> {
    let reply = with_daemon(settings, async |client| match choice {
        Some(choice) => client.read_snapshot(id, choice, None).await,
        None => client.read_by_id(id, None).await,
    })
    .await?;
    write_snapshot(&mut io::stdout().lock(), &reply)?;
    Ok(())
}

/// Prints the segment that `id` names, with a line for each of its snapshots. `id` names the
/// segment or one of its snapshots.
async fn snapshots(settings: &Settings, id: Uuid, style: ListStyle, now_ms: i64) -> Result<()> {
    let reply = with_daemon(settings, async |client| client.list_snapshots(id).await).await?;
    let mut out = BufWriter::new(io::stdout().lock());
    match style.framing() {
        Some(framing) => json::write_snapshots(&mut out, &reply, framing)?,
        None => write_snapshots(&mut out, &reply, now_ms)?,
    }
    out.flush()?;
    Ok(())
}

/// Prints the store's totals and the activity of the last [`STATS_WINDOW_MS`].
async fn stats(settings: &Settings, now_ms: i64) -> Result<()> {
    let stats = fetch_stats(settings, now_ms.saturating_sub(STATS_WINDOW_MS)).await?;
    write_stats(&mut io::stdout().lock(), &stats)?;
    Ok(())
}

/// Asks the daemon for the store's totals and for the activity since `since_ms`.
async fn fetch_stats(settings: &Settings, since_ms: i64) -> Result<Stats> {
    with_daemon(settings, async |client| {
        let status = client.status().await?;
        let recent = client
            .tally_segments(Tallied {
                from_ms: since_ms,
                ..Tallied::default()
            })
            .await?;
        eyre::Ok(Stats { status, recent })
    })
    .await
}

/// Runs the helper's `doctor`, which outlines the windows of the application `outline` when it
/// is given.
async fn doctor(settings: &Settings, outline: Option<&str>) -> Result<()> {
    let outline = outline.iter().flat_map(|app| [OsStr::new("--outline"), OsStr::new(app)]);
    run_helper(settings, iter::once(OsStr::new("doctor")).chain(outline)).await
}

/// Stops capture through the daemon, and says so.
async fn stop_capture(settings: &Settings) -> Result<()> {
    let stop_file = StopFile::in_dir(settings.desktop_capture_dir());
    let daemon = Daemon::connect(settings).await;
    stop(&stop_file, daemon, async |mut daemon| {
        daemon.request(async |client| client.stop().await).await
    })
    .await?;
    writeln!(io::stdout(), "{}", fl!("desktop-capture-stopped"))?;
    Ok(())
}

/// How long `atuin desktop-capture start` waits for the outcome of the helper that it starts.
const START_OUTCOME_WAIT: Duration = Duration::from_secs(5);

/// Starts capture through the daemon, and shows the state that follows: the outcome of the
/// helper that the start ran, or the setting that keeps capture off.
///
/// The daemon removes the stop file as it starts capture.
async fn start_capture(settings: &Settings) -> Result<()> {
    let reply = with_daemon(settings, async |client| {
        client.start(START_OUTCOME_WAIT).await?;
        client.status().await
    })
    .await?;
    let note = permission_note(&reply).await;
    write_start(&mut io::stdout().lock(), &reply, note.as_deref())?;
    Ok(())
}

/// Starts capture through the daemon, after starting the daemon when `daemon.autostart` allows.
///
/// The daemon removes the stop file as it starts capture.
pub(super) async fn start_through_daemon(settings: &Settings) -> Result<()> {
    with_daemon(settings, async |client| client.start(Duration::ZERO).await).await
}

/// Whether `atuin desktop-capture stop` keeps capture off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StopState {
    Stopped,
    NotStopped,
}

impl From<bool> for StopState {
    fn from(stopped: bool) -> Self {
        if stopped {
            Self::Stopped
        } else {
            Self::NotStopped
        }
    }
}

impl From<Requested> for StopState {
    fn from(requested: Requested) -> Self {
        match requested {
            Requested::Stopped => Self::Stopped,
            Requested::Running => Self::NotStopped,
        }
    }
}

/// Whether a stop keeps capture off, as `atuin desktop-capture status` reports it when a daemon
/// answers, and as the stop file says otherwise. Doesn't start a daemon.
pub(super) async fn stop_state(settings: &Settings) -> StopState {
    let daemon = ActiveService::find(settings).await.ok().flatten();
    read_stop_state(daemon, settings.desktop_capture_dir(), async |service| {
        let reply = service.desktop_capture_client().status().await?;
        eyre::Ok(reply.stopped)
    })
    .await
}

/// Whether a stop keeps capture off, as `daemon` answers through `stopped`, or as the stop file
/// in `dir` says when there is no `daemon` or `daemon` fails to answer.
async fn read_stop_state<D>(
    daemon: Option<D>,
    dir: PathBuf,
    stopped: impl AsyncFnOnce(D) -> Result<bool>,
) -> StopState {
    if let Some(daemon) = daemon
        && let Ok(stopped) = stopped(daemon).await
    {
        return stopped.into();
    }
    StopFile::in_dir(dir).requested().await.into()
}

/// Explains why capture waits for the permission when the helper lacks a certificate on macOS.
async fn permission_note(reply: &StatusReply) -> Option<String> {
    let signing = Signing::for_platform()?;
    let unsigned = reply.state() == State::NeedsPermission && !signing.has_certificate().await;
    unsigned.then(|| fl!("desktop-capture-status-unsigned"))
}

/// Asks `daemon` to stop with `ask`. When the daemon can't be reached, writes the stop file,
/// which the daemon reads as it starts, and fails with the error that `daemon` holds.
///
/// A daemon that answers keeps the stop file itself.
async fn stop<D>(
    stop_file: &StopFile,
    daemon: Result<D>,
    ask: impl AsyncFnOnce(D) -> Result<()>,
) -> Result<()> {
    match daemon {
        Ok(daemon) => ask(daemon).await,
        Err(unreached) => {
            stop_file.create().await?;
            Err(unreached.wrap_err(fl!("desktop-capture-stop-unreached")))
        }
    }
}

/// The range of the `--after` and `--before` flags. A day as `--before` means the day's start.
///
/// The range starts at the Unix epoch without `--after`, and reaches to now without `--before`.
fn flag_range(
    after: Option<&str>,
    before: Option<&str>,
    now: &ZonedDateTime,
    settings: &Settings,
) -> Result<TimeRange> {
    TimeRange::parse(
        after.map(|text| GivenEnd {
            name: "--after",
            text,
        }),
        before.map(|text| GivenEnd {
            name: "--before",
            text,
        }),
        RangeEnd::Before,
        Timestamp::UNIX_EPOCH,
        now,
        settings.dialect,
    )
    .map_err(|err| {
        let message = match err {
            TimeRangeError::Time { name, text, source } => fl!(
                "desktop-capture-bad-time",
                flag = name,
                time = text,
                error = source.to_string()
            ),
            TimeRangeError::Backwards { .. } => fl!("desktop-capture-backwards-range"),
            TimeRangeError::BeforeDefaultStart { default_from, .. } => {
                fl!("desktop-capture-before-default-start", start = default_from.to_string())
            }
        };
        Refusal(message).into()
    })
}

/// The daemon, ready for desktop capture requests.
struct Daemon {
    service: ActiveService,
    client: DesktopCaptureClient,
}

impl Daemon {
    /// Connects to the daemon, after starting or replacing it when `daemon.autostart` allows.
    ///
    /// Before replacing a daemon of another build, `connect` says which `atuin` started that
    /// daemon, and how to keep that `atuin` from replacing the daemon again.
    async fn connect(settings: &Settings) -> Result<Self> {
        if settings.daemon.autostart {
            warn_before_replacing(settings).await;
        }
        let service = ActiveService::ready(settings).await.map_err(|err| {
            // The refusal's message already says what the user can do.
            err.other_program()
                .map(|refusal| Refusal(refusal.to_string()))
                .map_or_else(|| eyre::Report::new(err), Into::into)
        })?;
        let client = service.desktop_capture_client();
        Ok(Self { service, client })
    }

    /// Sends a request to the daemon.
    ///
    /// A request that the daemon refuses, such as one that names an unknown id, fails with the
    /// daemon's reason alone. Any other failure also says that the daemon failed.
    async fn request<R, E>(
        &mut self,
        request: impl AsyncFnOnce(&mut DesktopCaptureClient) -> Result<R, E>,
    ) -> Result<R>
    where
        E: Into<eyre::Report>,
    {
        let reply = self.service.with_logs(request(&mut self.client)).await;
        reply.map_err(|err| failed_request(err.into()))
    }
}

/// Says which `atuin` started the daemon, and how to keep that `atuin` from replacing the daemon
/// again, when the daemon is another build that this client replaces.
async fn warn_before_replacing(settings: &Settings) {
    let Some(daemon) = OtherDaemon::find(settings).await.filter(OtherDaemon::is_replaced) else {
        return;
    };
    // A client doesn't replace a daemon that a launchd job runs, because launchd would start the
    // job's daemon again.
    let kept_by_brew_services = daemon.is_run_by_brew_services();
    if let Some(explanation) = Conflict::besides(Some(daemon)).await.explain() {
        atuin_common::userio::log::warn!("desktop-capture-conflict", explanation = explanation);
    }
    if kept_by_brew_services {
        return;
    }
    atuin_common::userio::log::info!(
        "desktop-capture-conflict",
        explanation = self::conflict::replacing()
    );
}

/// The error of a request that failed with `report`. A refusal of the daemon shows the daemon's
/// reason alone.
fn failed_request(report: eyre::Report) -> eyre::Report {
    match report.downcast_ref::<DaemonStatusError>() {
        Some(refused) if refused.is_refusal() => Refusal(refused.to_string()).into(),
        _ => report.wrap_err(fl!("desktop-capture-daemon-failed")),
    }
}

/// Sends a request to the daemon, after starting or replacing it when `daemon.autostart` allows.
async fn with_daemon<R, E>(
    settings: &Settings,
    request: impl AsyncFnOnce(&mut DesktopCaptureClient) -> Result<R, E>,
) -> Result<R>
where
    E: Into<eyre::Report>,
{
    Daemon::connect(settings).await?.request(request).await
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use eyre::eyre;
    use rstest::rstest;

    use super::*;

    #[derive(Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[rstest]
    #[case::apps(&["apps"], true)]
    #[case::apps_prefix(&["ap"], true)]
    #[case::setup_is_gone(&["setup"], false)]
    #[case::setup_prefix(&["set"], false)]
    #[case::write(&["apps", "--write"], true)]
    #[case::no_write(&["apps", "--no-write"], true)]
    #[case::write_and_no_write(&["apps", "--write", "--no-write"], false)]
    fn apps_replaces_setup(#[case] args: &[&str], #[case] parses_as_apps: bool) {
        let cli =
            Cli::try_parse_from(std::iter::once("desktop-capture").chain(args.iter().copied()));

        assert_eq!(cli.is_ok_and(|cli| matches!(cli.cmd, Cmd::Apps(_))), parses_as_apps);
    }

    /// A daemon that refuses a request names the reason alone, and a daemon that fails says
    /// that it failed too.
    #[rstest]
    #[case::not_found(tonic::Status::not_found("there is no segment or snapshot x"), true)]
    #[case::invalid(tonic::Status::invalid_argument("x is not an id"), true)]
    #[case::failed(tonic::Status::internal("the store is broken"), false)]
    fn refusals_are_not_daemon_failures(#[case] status: tonic::Status, #[case] refusal: bool) {
        let message = status.message().to_owned();
        let report = failed_request(DaemonStatusError::from(status).into());

        assert_eq!(report.downcast_ref::<Refusal>().is_some(), refusal);
        let shown = format!("{report:#}");
        assert!(shown.contains(&message), "{shown}");
        assert_eq!(shown.contains(&fl!("desktop-capture-daemon-failed")), !refusal, "{shown}");
    }

    #[rstest]
    #[case::neither(&["show", RECORD], None)]
    #[case::first(&["show", RECORD, "--first"], Some(SnapshotChoice::First))]
    #[case::snapshot(
        &["show", RECORD, "--snapshot", RECORD],
        Some(SnapshotChoice::Record(RECORD.parse().unwrap()))
    )]
    fn show_flags_choose_a_snapshot(
        #[case] args: &[&str],
        #[case] expected: Option<SnapshotChoice>,
    ) {
        let cli = Cli::try_parse_from(iter::once("desktop-capture").chain(args.iter().copied()));
        let Ok(Cli {
            cmd: Cmd::Show { snapshot, .. },
        }) = cli
        else {
            panic!("{cli:?}");
        };
        assert_eq!(snapshot.choice(), expected);
    }

    const RECORD: &str = "018f0000-0000-7000-8000-000000000000";

    /// The flag that the autostart entry passes says that `login` runs in a desktop session.
    #[rstest]
    #[case::autostart_entry(&["login", session::IN_DESKTOP_SESSION_FLAG], true)]
    #[case::login_file(&["login", "--shell", "sh"], false)]
    fn login_knows_whether_it_runs_in_a_desktop_session(
        #[case] args: &[&str],
        #[case] expected: bool,
    ) {
        let cli = Cli::try_parse_from(iter::once("desktop-capture").chain(args.iter().copied()));
        let Ok(Cli {
            cmd: Cmd::Login {
                in_desktop_session, ..
            },
        }) = cli
        else {
            panic!("{cli:?}");
        };
        assert_eq!(in_desktop_session, expected);
    }

    #[rstest]
    fn log_takes_kinds_as_a_list() {
        let cli = Cli::try_parse_from(["desktop-capture", "log", "--kind", "active,dialog"]);
        let Ok(Cli {
            cmd: Cmd::Log { kind, .. },
        }) = cli
        else {
            panic!("{cli:?}");
        };
        assert_eq!(kind, [Kind::Active, Kind::Dialog]);
    }

    /// The flags of a range fail with a refusal that names the flag, or the order of the flags.
    #[rstest]
    #[case::bad_time(Some("soon"), None, "--after")]
    #[case::backwards(Some("1h"), Some("2h"), "is later than")]
    fn flag_ranges_refuse_bad_flags(
        #[case] after: Option<&str>,
        #[case] before: Option<&str>,
        #[case] named: &str,
    ) {
        let now = Timestamp::now().to_zoned_datetime(jiff::tz::TimeZone::UTC);
        let err = flag_range(after, before, &now, &Settings::utc()).unwrap_err();

        let refusal = err.downcast_ref::<Refusal>().unwrap();
        assert!(refusal.0.contains(named), "{}", refusal.0);
    }

    /// The number of files in `dir`. The stop file is the only file there when capture is
    /// stopped.
    fn files(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, Iterator::count)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Connection {
        Answers,
        Unreached,
    }

    /// The daemon's answer decides when the daemon answers, and the stop file decides when no
    /// daemon answers.
    #[rstest]
    #[case::daemon_stopped(Some(Ok(true)), StopState::NotStopped, StopState::Stopped)]
    #[case::daemon_running(Some(Ok(false)), StopState::Stopped, StopState::NotStopped)]
    #[case::daemon_fails_file_stopped(Some(Err(())), StopState::Stopped, StopState::Stopped)]
    #[case::daemon_fails_no_file(Some(Err(())), StopState::NotStopped, StopState::NotStopped)]
    #[case::no_daemon_file_stopped(None, StopState::Stopped, StopState::Stopped)]
    #[case::no_daemon_no_file(None, StopState::NotStopped, StopState::NotStopped)]
    #[tokio::test]
    async fn stop_state_asks_the_daemon_before_the_stop_file(
        #[case] daemon: Option<Result<bool, ()>>,
        #[case] file: StopState,
        #[case] expected: StopState,
    ) {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("desktop-capture");
        if file == StopState::Stopped {
            StopFile::in_dir(dir.clone()).create().await.unwrap();
        }

        let state =
            read_stop_state(daemon, dir, async |answer| answer.map_err(|()| eyre!("no answer")))
                .await;

        assert_eq!(state, expected);
    }

    /// A directory that holds other files and no stop file says that capture isn't stopped.
    #[rstest]
    #[tokio::test]
    async fn other_files_are_not_a_stop() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("store.db"), b"").unwrap();

        let state = read_stop_state(None::<()>, home.path().to_owned(), async |()| Ok(true)).await;

        assert_eq!(state, StopState::NotStopped);
    }

    /// The stop file is the daemon's when the daemon answers, and the CLI writes it only when
    /// the daemon can't be reached.
    #[rstest]
    #[case::answered(Connection::Answers, 0)]
    #[case::unreached(Connection::Unreached, 1)]
    #[tokio::test]
    async fn stop_writes_the_stop_file_only_without_a_daemon(
        #[case] daemon: Connection,
        #[case] left: usize,
    ) {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("desktop-capture");
        let stop_file = StopFile::in_dir(dir.clone());
        let connected = match daemon {
            Connection::Answers => Ok(()),
            Connection::Unreached => Err(eyre!("no daemon")),
        };

        let stopped = stop(&stop_file, connected, async |()| {
            assert_eq!(files(&dir), 0, "the daemon keeps the stop file itself");
            Ok(())
        })
        .await;

        assert_eq!(stopped.is_ok(), daemon == Connection::Answers);
        assert_eq!(files(&dir), left);
    }
}
