use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_daemon::client::{DaemonClientErrorKind, HistoryClient, classify_error};
use atuin_daemon::grpc::versioning::BUILD_STR;
use atuin_daemon::service::{ActiveService, PidfileWaitError, ServiceClaim, StopError};
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
    #[cfg(unix)]
    pub fn should_daemonize(&self) -> bool {
        match &self.subcmd {
            Some(SubCmd::Start { daemonize, .. }) => *daemonize,
            None => self.daemonize,
            _ => false,
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
        match self.subcmd {
            None => {
                eprintln!("Warning: `atuin daemon` is deprecated, use `atuin daemon start`");
                run(settings, false).await
            }
            Some(SubCmd::Start { force, .. }) => run(settings, force).await,
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
    Ok(ActiveService::ready(settings).await?.into_client())
}

/// Send a request to the daemon, first ensuring (via [`ready_client`]) that it is running and is
/// our build.
async fn try_with_restart<F, R>(settings: &Settings, send_request: F) -> Result<R>
where
    F: AsyncFnOnce(&mut HistoryClient) -> Result<R> + Sync,
{
    let mut client = ready_client(settings).await?;
    send_request(&mut client).await
}

/// Send a request to an already-running daemon of our build.
///
/// This function never starts or restarts the daemon.
async fn try_without_restart<F, R>(settings: &Settings, send_request: F) -> Result<R>
where
    F: AsyncFnOnce(&mut HistoryClient) -> Result<R> + Sync,
{
    let Some(service) = ActiveService::find(settings).await? else {
        bail!("no daemon is running");
    };
    service.ensure_our_build()?;
    send_request(&mut service.into_client()).await
}

/// How long `history start` waits on the daemon. It runs synchronously before every command, and a
/// stuck daemon still accepts connections, so without a limit the prompt hangs. On timeout the
/// command is saved to the local database instead.
///
/// `history end` and `cancel` have no limit: the hooks run them in the background (on Nushell, from
/// 0.104), and giving up would lose a command that only exists in the daemon's memory.
const START_HISTORY_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn start_history(settings: &Settings, history: History) -> Result<HistoryId> {
    let start = try_with_restart(settings, async |client| client.start_history(history).await);
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
/// `AiClient::rebuild_sessions`), first starting or replacing it (via [`ready_client`]) as the
/// other AI session commands do.
pub async fn rebuild_ai_sessions(settings: &Settings) -> Result<()> {
    ready_client(settings).await?;
    let mut client = atuin_daemon::AiClient::from_settings(settings).await?;
    client.rebuild_sessions().await
}

/// How long to look for a running daemon to project synced AI sessions: a stopped daemon's socket
/// still accepts connections but never answers.
const PROJECT_SYNCED_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a sync waits for the daemon to project the AI sessions it downloaded. A large store
/// takes longer; the daemon carries on regardless.
const PROJECT_SYNCED_WAIT: Duration = Duration::from_secs(10);

/// Have a running daemon project the AI session records this process synced into the record
/// store, returning once they are listed or, after [`PROJECT_SYNCED_WAIT`], leaving the daemon to
/// finish on its own. Never starts or replaces one: a daemon started later, or replacing one too
/// old to be asked, projects them as it starts.
pub async fn project_synced_ai_sessions(settings: &Settings) -> Result<()> {
    let found = tokio::time::timeout(PROJECT_SYNCED_PROBE_TIMEOUT, ActiveService::find(settings));
    let Ok(Ok(Some(service))) = found.await else {
        return Ok(());
    };
    if service.ensure_our_build().is_err() {
        return Ok(());
    }
    let mut client = atuin_daemon::AiClient::from_settings(settings).await?;
    match tokio::time::timeout(PROJECT_SYNCED_WAIT, client.project_synced()).await {
        Ok(Err(err)) if is_legacy_daemon_error(&err) => Ok(()),
        Ok(projected) => projected,
        Err(_) => {
            eprintln!("Indexing threads in the background");
            Ok(())
        }
    }
}

pub async fn compact_store(settings: &Settings) -> Result<u64> {
    let reply = try_with_restart(settings, async |client| client.compact_store().await).await?;
    Ok(reply.rewritten)
}

async fn status_cmd(settings: &Settings) -> Result<()> {
    let Some(service) = ActiveService::find(settings).await? else {
        println!("Daemon is not running");
        return Ok(());
    };

    match service.ensure_our_build() {
        Ok(()) => {
            let address = service.address().clone();
            let status = service.into_client().status().await?;
            println!("Daemon running");
            println!("  PID:      {}", status.pid);
            println!("  Version:  {}", status.version);
            println!("  Build:    {BUILD_STR}");
            println!("  Healthy:  {}", status.healthy);
            println!("  Address:  {address}");
        }
        Err(reason) if reason.peer_is_newer() => {
            println!("Daemon running (newer than this client)");
            println!("  Reason: {reason}");
        }
        Err(reason) => {
            println!("Daemon running (needs restart)");
            println!("  Reason: {reason}");
        }
    }

    Ok(())
}

pub(super) async fn stop_cmd(settings: &Settings) -> Result<()> {
    let Some(service) = ActiveService::find(settings).await? else {
        println!("Daemon is not running");
        return Ok(());
    };

    match service.stop().await {
        Ok(()) => println!("Daemon stopped"),
        Err(StopError::Pidfile(PidfileWaitError::StillHeld { .. })) => {
            println!("Daemon may still be shutting down");
        }
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

pub(super) async fn restart_cmd(settings: &Settings, only_if_running: bool) -> Result<()> {
    let service = match ActiveService::find(settings).await? {
        Some(service) => {
            println!("Restarting daemon...");
            service.restart().await?
        }
        None if only_if_running => {
            println!("No daemon running");
            return Ok(());
        }
        None => {
            println!("No daemon running");
            println!("Starting daemon...");
            ActiveService::find_or_spawn(settings).await?
        }
    };

    let status = service.into_client().status().await?;

    println!("Daemon restarted");
    println!("  PID:      {}", status.pid);
    println!("  Version:  {}", status.version);

    Ok(())
}

async fn run(settings: Arc<Settings>, force: bool) -> Result<()> {
    if force {
        ActiveService::evict(&settings.daemon).await;
    }

    let (_claim, listener) = ServiceClaim::take(&settings.daemon)?;

    let history_db = Sqlite::new(&settings.db_path, settings.local_timeout).await?;
    let store = SqliteStore::new(&settings.record_store_path, settings.local_timeout).await?;

    atuin_daemon::boot(settings, listener, store, history_db).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use atuin_common::path::AbsPathBuf;
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
