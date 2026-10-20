use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_daemon::client::{DaemonClientErrorKind, HistoryClient, classify_error};
use atuin_daemon::grpc::versioning::BUILD_STR;
use atuin_daemon::server::SocketSource;
use atuin_daemon::service::pidfile::{Pidfile, PidfileInfo};
use atuin_daemon::service::{ActiveService, PidfileWaitError, StopError};
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

    pub async fn run(
        self,
        settings: Arc<Settings>,
        store: SqliteStore,
        history_db: Sqlite,
    ) -> Result<()> {
        match self.subcmd {
            None => {
                eprintln!("Warning: `atuin daemon` is deprecated, use `atuin daemon start`");
                run(settings, store, history_db, false).await
            }
            Some(SubCmd::Start { force, .. }) => run(settings, store, history_db, force).await,
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

/// An error that occurred while trying to remove a socket.
#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
#[error(
    "{}",
    fl!(
        "daemon-remove-socket-failed",
        path = .path.display().to_string(),
        source = .source.to_string()
    )
)]
struct RemoveSocketError {
    path: PathBuf,
    source: std::io::Error,
}

/// Remove the daemon's socket from every path it may be at, subject to `should_remove`, unless
/// `socket` is one systemd holds.
#[cfg(unix)]
fn remove_sockets(
    settings: &Settings,
    socket: &SocketSource,
    should_remove: impl Fn(&Path) -> bool,
) -> Result<(), RemoveSocketError> {
    match socket {
        SocketSource::Configured => {}
        SocketSource::Systemd(_) => return Ok(()),
    }

    let mut error = None;
    for socket_path in atuin_daemon::client::potential_socket_paths(settings) {
        if !socket_path.exists() || !should_remove(&socket_path) {
            continue;
        }

        if let Err(e) = fs::remove_file(&socket_path)
            && e.kind() != ErrorKind::NotFound
        {
            // Log the error because we only return the first error when multiple occur.
            tracing::error!("failed to remove daemon socket {}: {e}", socket_path.display());
            error.get_or_insert_with(|| RemoveSocketError {
                path: socket_path.into_owned(),
                source: e,
            });
        }
    }

    error.map_or(Ok(()), Err)
}

/// Remove any socket left behind by a daemon that is no longer listening.
#[cfg(unix)]
fn remove_stale_socket_if_present(
    settings: &Settings,
    socket: &SocketSource,
) -> Result<(), RemoveSocketError> {
    use std::os::unix::fs::FileTypeExt;

    remove_sockets(settings, socket, |socket_path| {
        matches!(atuin_common::os::unix::socket_in_use(socket_path), Ok(false))
            // Ensure file is a socket; `socket_in_use` can return `Ok(false)` for regular files.
            && fs::symlink_metadata(socket_path).is_ok_and(|meta| meta.file_type().is_socket())
    })
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

async fn run(
    settings: Arc<Settings>,
    store: SqliteStore,
    history_db: Sqlite,
    force: bool,
) -> Result<()> {
    let socket = SocketSource::from_env().wrap_err("taking the socket systemd passed in")?;

    if force {
        force_cleanup(&settings).await;
    }

    let _pidfile_guard = Pidfile::new(&settings.daemon.pidfile_path)
        .acquire(&PidfileInfo::current(&settings.daemon, &socket))?;

    // A daemon that crashed left its socket behind, which would keep this one from listening.
    #[cfg(unix)]
    remove_stale_socket_if_present(&settings, &socket)?;

    atuin_daemon::boot(settings, socket, store, history_db).await?;

    Ok(())
}

/// Force cleanup: kill the daemon the pidfile names and remove the pidfile.
///
/// Sockets are left to the stale-socket check every start makes: nothing listens at the killed
/// daemon's anymore, but a systemd socket unit's still does and must stay.
async fn force_cleanup(settings: &Settings) {
    let pidfile_path = Path::new(&settings.daemon.pidfile_path);

    // Read and kill the existing process if pidfile exists
    if pidfile_path.exists() {
        if let Some(pid) = Pidfile::new(&settings.daemon.pidfile_path).pid()
            && pid != 0
            && let Err(e) =
                atuin_common::os::process::force_terminate(pid, Duration::from_secs(2)).await
        {
            tracing::warn!("could not terminate existing daemon (pid {pid}): {e}");
        }

        // Remove the pidfile
        if let Err(e) = fs::remove_file(pidfile_path)
            && e.kind() != ErrorKind::NotFound
        {
            tracing::warn!("failed to remove pidfile: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::path::AbsPathBuf;
    use rstest::rstest;

    use super::*;

    #[cfg(unix)]
    #[rstest]
    fn remove_socket_error_names_the_path_and_cause() {
        let err = RemoveSocketError {
            path: PathBuf::from("/run/atuin.sock"),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        assert_eq!(
            err.to_string(),
            "failed to remove daemon socket /run/atuin.sock: permission denied"
        );
    }

    /// A socket nothing listens on is removed; one a daemon listens on, and a file that is no
    /// socket (a `socket_path` set wrong), are not.
    #[cfg(unix)]
    #[rstest]
    #[case::stale(false, true, false)]
    #[case::listening(true, true, true)]
    #[case::not_a_socket(false, false, true)]
    fn only_a_stale_socket_is_removed(
        #[case] listening: bool,
        #[case] socket: bool,
        #[case] kept: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atuin.sock");
        let listener = socket.then(|| std::os::unix::net::UnixListener::bind(&path).unwrap());
        if !socket {
            fs::write(&path, "not a socket").unwrap();
        }
        if !listening {
            drop(listener);
        }
        let mut settings = Settings::default();
        settings.daemon.socket_path = AbsPathBuf::new(path.clone()).ok();
        remove_stale_socket_if_present(&settings, &SocketSource::Configured).unwrap();
        assert_eq!(path.exists(), kept);
    }

    /// A forced start leaves a socket something still listens on, as a systemd socket unit does
    /// whether or not its daemon runs, rather than cutting clients off from it.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_forced_start_keeps_a_socket_something_listens_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atuin.sock");
        let _socket_unit = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let mut settings = Settings::default();
        settings.daemon.socket_path = AbsPathBuf::new(path.clone()).ok();
        settings.daemon.pidfile_path = AbsPathBuf::new(dir.path().join("d.pid")).unwrap();

        force_cleanup(&settings).await;
        remove_stale_socket_if_present(&settings, &SocketSource::Configured).unwrap();

        assert!(path.exists());
    }

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
