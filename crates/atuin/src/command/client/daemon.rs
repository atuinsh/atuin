use std::fmt;
use std::io::{self, Write as _};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::userio;
use atuin_daemon::client::{DaemonClientErrorKind, HistoryClient, classify_error};
use atuin_daemon::grpc::versioning::{BUILD_STR, VersionMatchError};
use atuin_daemon::service::address::DaemonAddress;
use atuin_daemon::service::{
    ActiveService, FindError, Launch, PidfileWaitError, ServiceClaim, ServiceFindError, StopError,
};
use clap::Subcommand;
use eyre::{Result, WrapErr, bail};

use crate::i18n::fl;

#[derive(clap::Args, Debug)]
pub struct Cmd {
    #[arg(long, hide = true, help = fl!("arg-daemon-daemonize"))]
    daemonize: bool,

    #[arg(long, help = fl!("arg-daemon-show-logs"))]
    show_logs: bool,

    #[command(subcommand)]
    subcmd: Option<SubCmd>,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum SubCmd {
    #[command(about = fl!("cmd-daemon-start"))]
    Start {
        #[arg(long, hide = true)]
        daemonize: bool,

        #[arg(long, help = fl!("arg-daemon-show-logs"))]
        show_logs: bool,

        #[arg(long, help = fl!("arg-daemon-start-force"))]
        force: bool,
    },

    #[command(about = fl!("cmd-daemon-status"))]
    Status,

    #[command(about = fl!("cmd-daemon-stop"))]
    Stop,

    #[command(about = fl!("cmd-daemon-restart"))]
    Restart,
}

impl Cmd {
    /// Returns `true` when the process should daemonize before creating the
    /// async runtime or opening any database connections.
    pub fn should_daemonize(&self) -> bool {
        match &self.subcmd {
            Some(SubCmd::Start { daemonize, .. }) => *daemonize,
            None => self.daemonize,
            _ => false,
        }
    }

    /// How `daemon start` was asked to run: forked off the client that ran it, or in place.
    fn launch(&self) -> Launch {
        if self.should_daemonize() {
            Launch::Daemonize
        } else {
            Launch::InPlace
        }
    }

    /// Returns `true` when logs should also be written to the console.
    pub fn show_logs(&self) -> bool {
        match &self.subcmd {
            Some(SubCmd::Start { show_logs, .. }) => *show_logs,
            None => self.show_logs,
            _ => false,
        }
    }

    pub async fn run(self, settings: Arc<Settings>) -> Result<()> {
        let launch = self.launch();
        match self.subcmd {
            None => {
                userio::log::warn!("daemon-deprecated");
                run(settings, false, launch).await
            }
            Some(SubCmd::Start { force, .. }) => run(settings, force, launch).await,
            Some(SubCmd::Status) => status_cmd(&settings).await,
            Some(SubCmd::Stop) => stop_cmd(&settings).await,
            Some(SubCmd::Restart) => restart_cmd(&settings, false).await,
        }
    }
}

fn is_legacy_daemon_error(err: &eyre::Report) -> bool {
    matches!(classify_error(err), DaemonClientErrorKind::Unimplemented)
}

pub(super) fn should_retry_after_error(err: &eyre::Report) -> bool {
    matches!(
        classify_error(err),
        DaemonClientErrorKind::Connect
            | DaemonClientErrorKind::Unavailable
            | DaemonClientErrorKind::Unimplemented
            | DaemonClientErrorKind::BuildMismatch
    )
}

/// Acquire a [`HistoryClient`] connected to a daemon of our build, starting or replacing it first
/// when `daemon.autostart` allows.
pub async fn ready_client(settings: &Settings) -> Result<HistoryClient> {
    Ok(ActiveService::ready(settings).await?.history_client())
}

/// Send a request to the daemon, first ensuring (as [`ready_client`] does) that it is running and
/// is our build, and show here what the daemon tells the user meanwhile.
async fn try_with_restart<F, R>(settings: &Settings, send_request: F) -> Result<R>
where
    F: AsyncFnOnce(&mut HistoryClient) -> Result<R> + Sync,
{
    let service = ActiveService::ready(settings).await?;
    service.with_logs(async { send_request(&mut service.history_client()).await }).await
}

/// Send a request to an already-running daemon of our build.
///
/// This function never starts or restarts the daemon, though launchd starts its agent for the
/// connection.
async fn try_without_restart<F, R>(settings: &Settings, send_request: F) -> Result<R>
where
    F: AsyncFnOnce(&mut HistoryClient) -> Result<R> + Sync,
{
    let Some(service) = ActiveService::find(settings).await? else {
        bail!("no daemon is running");
    };
    service.ensure_our_build()?;
    send_request(&mut service.history_client()).await
}

/// How long `history start` waits on the daemon. It runs synchronously before every command, and a
/// stuck daemon still accepts connections, so without a limit the prompt hangs. On timeout the
/// command is saved to the local database instead.
///
/// `history end` and `cancel` have no limit: the hooks run them in the background (on Nushell, from
/// 0.104), and giving up would lose a command that only exists in the daemon's memory.
const START_HISTORY_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn start_history(settings: &Settings, history: History) -> Result<HistoryId> {
    // Not through `try_with_restart`: the shell hook keeps quiet, so it has no use for the daemon's
    // messages.
    let start = async { ready_client(settings).await?.start_history(history).await };
    let resp = tokio::time::timeout(START_HISTORY_TIMEOUT, start)
        .await
        .wrap_err("timed out waiting for the daemon")??;
    let id = resp.id.ok_or_else(|| eyre::eyre!("daemon reply is missing the history id"))?;
    Ok(HistoryId::try_from(id)?)
}

pub async fn end_history(
    settings: &Settings,
    id: HistoryId,
    duration: Option<std::time::Duration>,
    exit: i64,
) -> Result<()> {
    try_without_restart(settings, async |client| client.end_history(id, duration, exit).await)
        .await?;
    Ok(())
}

pub async fn cancel_history(settings: &Settings, id: HistoryId) -> Result<()> {
    try_without_restart(settings, async |client| client.cancel_history(id).await).await?;
    Ok(())
}

pub async fn delete_history(settings: &Settings, ids: Vec<HistoryId>) -> Result<u64> {
    let reply = try_with_restart(settings, async |client| client.delete_history(ids).await).await?;
    Ok(reply.deleted)
}

pub async fn rebuild_history(settings: &Settings) -> Result<()> {
    try_with_restart(settings, async |client| client.rebuild_history().await).await?;
    Ok(())
}

/// Have the daemon rebuild its AI sessions from the record store (see
/// `AiClient::rebuild_sessions`), first starting or replacing it as the other AI session commands
/// do, and show here what it tells the user meanwhile.
pub async fn rebuild_ai_sessions(settings: &Settings) -> Result<()> {
    let service = ActiveService::ready(settings).await?;
    let mut client = atuin_daemon::AiClient::from_settings(settings).await?;
    service.with_logs(client.rebuild_sessions()).await
}

/// How long to look for a running daemon: a stopped daemon's socket still accepts connections but
/// never answers.
const RUNNING_DAEMON_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The running daemon, if one answers within [`RUNNING_DAEMON_PROBE_TIMEOUT`] and is our build.
///
/// Never starts or restarts the daemon, though launchd starts its agent for the connection.
async fn running_daemon_of_our_build(settings: &Settings) -> Option<ActiveService> {
    let found = tokio::time::timeout(RUNNING_DAEMON_PROBE_TIMEOUT, ActiveService::find(settings));
    let Ok(Ok(Some(service))) = found.await else {
        return None;
    };
    service.ensure_our_build().ok()?;
    Some(service)
}

/// How long a sync waits for the daemon to project the AI sessions it downloaded. A large store
/// takes longer; the daemon carries on regardless.
const PROJECT_SYNCED_WAIT: Duration = Duration::from_secs(10);

/// Have a running daemon project the AI session records this process synced into the record
/// store, returning once they are listed or, after [`PROJECT_SYNCED_WAIT`], leaving the daemon to
/// finish on its own. Never starts or replaces one, though launchd starts its agent for the
/// connection: a daemon started later, or replacing one too old to be asked, projects them as it
/// starts.
pub async fn project_synced_ai_sessions(settings: &Settings) -> Result<()> {
    let Some(service) = running_daemon_of_our_build(settings).await else {
        return Ok(());
    };
    let mut client = atuin_daemon::AiClient::from_settings(settings).await?;
    let projected = tokio::time::timeout(PROJECT_SYNCED_WAIT, client.project_synced());
    match service.with_logs(projected).await {
        Ok(Err(err)) if is_legacy_daemon_error(&err) => Ok(()),
        Ok(projected) => projected,
        Err(_) => {
            userio::log::info!("daemon-indexing-threads");
            Ok(())
        }
    }
}

/// How long to wait for the daemon to rebuild the search index after a sync.
///
/// If the timeout is reached, the daemon will still complete the rebuild; the client will just
/// return early.
const SEARCH_INDEX_REBUILD_WAIT: Duration = Duration::from_secs(10);

/// Tell the daemon to rebuild the search index from the history DB.
pub async fn rebuild_search_index(settings: &Settings) -> Result<()> {
    let Some(service) = running_daemon_of_our_build(settings).await else {
        return Ok(());
    };
    let mut client = atuin_daemon::client::SearchClient::from_settings(settings).await?;
    let rebuilt = tokio::time::timeout(SEARCH_INDEX_REBUILD_WAIT, client.rebuild_index());
    service.with_logs(rebuilt).await.unwrap_or_else(|_| {
        userio::log::info!("daemon-rebuilding-search-index");
        Ok(())
    })
}

pub async fn compact_store(settings: &Settings) -> Result<u64> {
    let reply = try_with_restart(settings, async |client| client.compact_store().await).await?;
    Ok(reply.rewritten)
}

/// What `daemon status` and `daemon restart` report about the daemon they find.
enum StatusReport {
    NotRunning,
    /// A daemon of this build.
    Running {
        pid: u32,
        version: String,
        healthy: bool,
        address: DaemonAddress,
    },
    /// A daemon of another build, which this client can't use.
    OtherBuild(VersionMatchError),
}

impl StatusReport {
    async fn of(service: &ActiveService) -> Result<Self> {
        if let Err(reason) = service.ensure_our_build() {
            return Ok(Self::OtherBuild(reason));
        }
        let status = service.history_client().status().await?;
        Ok(Self::Running {
            pid: status.pid,
            version: status.version,
            healthy: status.healthy,
            address: service.address().clone(),
        })
    }
}

impl fmt::Display for StatusReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => write!(f, "Daemon is not running"),
            Self::Running {
                pid,
                version,
                healthy,
                address,
            } => {
                writeln!(f, "Daemon running")?;
                writeln!(f, "  PID:      {pid}")?;
                writeln!(f, "  Version:  {version}")?;
                writeln!(f, "  Build:    {BUILD_STR}")?;
                writeln!(f, "  Healthy:  {healthy}")?;
                write!(f, "  Address:  {address}")
            }
            Self::OtherBuild(reason) if reason.peer_is_newer() => {
                write!(f, "Daemon running (newer than this client)\n  Reason: {reason}")
            }
            Self::OtherBuild(reason) => {
                write!(f, "Daemon running (needs restart)\n  Reason: {reason}")
            }
        }
    }
}

async fn status_cmd(settings: &Settings) -> Result<()> {
    let report = match ActiveService::find(settings).await? {
        Some(service) => StatusReport::of(&service).await?,
        None => StatusReport::NotRunning,
    };
    writeln!(io::stdout(), "{report}")?;
    Ok(())
}

pub(super) async fn stop_cmd(settings: &Settings) -> Result<()> {
    let service = match ActiveService::find(settings).await {
        Ok(Some(service)) => service,
        Ok(None) => {
            userio::log::info!("daemon-not-running");
            return Ok(());
        }
        // One that can't answer can't be asked to stop either.
        Err(ServiceFindError::Find(FindError::Unresponsive(_))) => {
            ActiveService::evict(&settings.daemon).await?;
            userio::log::info!("daemon-stopped");
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    match service.stop().await {
        Ok(()) => userio::log::info!("daemon-stopped"),
        Err(StopError::Pidfile(PidfileWaitError::StillHeld { .. })) => {
            userio::log::info!("daemon-may-be-stopping");
        }
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

pub(super) async fn restart_cmd(settings: &Settings, only_if_running: bool) -> Result<()> {
    let service = match ActiveService::find(settings).await {
        Ok(Some(service)) => {
            userio::log::info!("daemon-restarting");
            service.restart().await?
        }
        Ok(None) if only_if_running => {
            userio::log::info!("daemon-none-running");
            return Ok(());
        }
        Ok(None) => {
            userio::log::info!("daemon-none-running");
            userio::log::info!("daemon-starting");
            ActiveService::find_or_spawn(settings).await?
        }
        Err(ServiceFindError::Find(FindError::Unresponsive(_))) => {
            userio::log::info!("daemon-replacing-unresponsive");
            ActiveService::evict(&settings.daemon).await?;
            ActiveService::find_or_spawn(settings).await?
        }
        Err(err) => return Err(err.into()),
    };

    let report = StatusReport::of(&service).await?;
    userio::log::info!("daemon-restarted");
    writeln!(io::stdout(), "{report}")?;
    Ok(())
}

async fn run(settings: Arc<Settings>, force: bool, launch: Launch) -> Result<()> {
    if force && let Err(e) = ActiveService::evict(&settings.daemon).await {
        tracing::warn!("{e}");
    }

    let (_claim, listener) = ServiceClaim::take(&settings.daemon, launch).await?;

    let history_db = Sqlite::new(&settings.db_path, settings.local_timeout).await?;
    let store = SqliteStore::new(&settings.record_store_path, settings.local_timeout).await?;

    atuin_daemon::boot(settings, listener, store, history_db).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use atuin_common::path::AbsPathBuf;
    use atuin_daemon::service::RunMode;
    use atuin_daemon::service::pidfile::{Pidfile, PidfileInfo};
    use rstest::rstest;

    use super::*;

    /// A stopped daemon's socket still accepts connections but never answers.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn start_history_times_out_on_a_daemon_that_never_answers() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("stopped.sock");
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let mut settings = Settings::default();
        settings.daemon.socket_path = AbsPathBuf::new(socket).ok();
        settings.daemon.pidfile_path = AbsPathBuf::new(dir.path().join("d.pid")).unwrap();
        settings.daemon.autostart = false;
        // Stopped, it still holds its pidfile, so clients take it for a slow daemon to wait on,
        // not a socket with nothing behind it to give up on.
        let info = PidfileInfo::current(&settings.daemon, RunMode::DaemonizeForked);
        let _stopped = Pidfile::new(&settings.daemon.pidfile_path).acquire(&info).unwrap();
        let history = History::import()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("true")
            .cwd("/")
            .build();

        let started = std::time::Instant::now();
        let err = start_history(&settings, history.into()).await.unwrap_err();

        assert!(started.elapsed() < START_HISTORY_TIMEOUT * 2, "{:?}", started.elapsed());
        assert!(format!("{err:#}").contains("timed out"), "{err:#}");
    }
}
