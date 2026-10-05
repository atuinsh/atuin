use std::fs;
use std::io::ErrorKind;
use std::ops::ControlFlow;
#[cfg(unix)]
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::fs::lock::{LockError, LockMode, LockOptions};
use atuin_common::futures::Backoff;
use atuin_daemon::client::{DaemonClientErrorKind, HistoryClient, classify_error};
use atuin_daemon::pidfile::{self, PidfileGuard};
use atuin_daemon::{PROTOCOL_VERSION, VERSION};
use clap::Subcommand;
#[cfg(unix)]
use daemonix::Daemonize;
use eyre::{Result, WrapErr, bail, eyre};

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
        settings: Settings,
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
            Some(SubCmd::Restart) => restart_cmd(&settings).await,
        }
    }
}

const STARTUP_POLL: Duration = Duration::from_millis(40);
const LEGACY_DAEMON_RESTART_MESSAGE: &str = "legacy daemon detected; restart daemon manually";

/// How long a single connect to the daemon's socket may take.
///
/// The check only has to notice a listener, so it must not inherit the stall it exists to
/// diagnose: a socket that hasn't answered within this isn't serving.
const SERVING_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// How long a daemon that is alive but not yet reachable is given to start serving, before it is
/// treated as wedged.
///
/// A daemon takes the pidfile lock before it binds its socket, so without this grace a client
/// racing a daemon that is still booting would replace it.
const SERVING_GRACE: Duration = Duration::from_secs(1);

/// How long to wait for a wedged daemon to exit once it has been asked to.
const WEDGE_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

enum Probe {
    Ready(HistoryClient),
    NeedsRestart(String),
    Unreachable(eyre::Report),
}

fn daemon_matches_expected(version: &str, protocol: u32) -> bool {
    version == VERSION && protocol == PROTOCOL_VERSION
}

fn daemon_mismatch_message(version: &str, protocol: u32) -> String {
    if protocol == PROTOCOL_VERSION {
        format!("daemon is out of date: expected {VERSION}, got {version}")
    } else {
        format!("daemon protocol mismatch: expected {PROTOCOL_VERSION}, got {protocol}")
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
    )
}

async fn wait_for_pidfile_available(path: &Path, timeout: Duration) -> Result<()> {
    let file = LockOptions {
        create: true,
        mode: LockMode::Exclusive,
    }
    .wait(path, timeout)
    .await
    .wrap_err_with(|| format!("failed to lock daemon pidfile at {}", path.display()))?;

    file.unlock()
        .wrap_err_with(|| format!("failed to unlock daemon pidfile at {}", path.display()))?;
    Ok(())
}

async fn connect_client(settings: &Settings) -> Result<HistoryClient> {
    HistoryClient::from_settings(settings).await
}

async fn probe(settings: &Settings) -> Probe {
    let mut client = match connect_client(settings).await {
        Ok(client) => client,
        Err(err) => return Probe::Unreachable(err),
    };

    match client.status().await {
        Ok(status) => {
            if daemon_matches_expected(&status.version, status.protocol) {
                Probe::Ready(client)
            } else {
                Probe::NeedsRestart(daemon_mismatch_message(&status.version, status.protocol))
            }
        }
        Err(err) => Probe::Unreachable(err),
    }
}

async fn request_shutdown(settings: &Settings) {
    if let Ok(mut client) = connect_client(settings).await {
        let _ = client.shutdown().await;
    }
}

fn spawn_daemon_process() -> Result<()> {
    let exe = std::env::current_exe().wrap_err("could not locate atuin executable")?;

    let mut cmd = Command::new(exe);
    cmd.arg("daemon").arg("start").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    #[cfg(unix)]
    cmd.arg("--daemonize");

    cmd.spawn().wrap_err("failed to spawn daemon process")?;

    Ok(())
}

fn startup_timeout(settings: &Settings) -> Duration {
    settings.local_timeout.max(Duration::from_millis(500)) + Duration::from_secs(2)
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

/// Remove the daemon's socket from every path it may be at, subject to `should_remove`.
#[cfg(unix)]
fn remove_sockets(
    settings: &Settings,
    should_remove: impl Fn(&Path) -> bool,
) -> Result<(), RemoveSocketError> {
    if settings.daemon.systemd_socket {
        return Ok(());
    }

    let mut error = None;
    for socket_path in settings.daemon.potential_socket_paths() {
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
fn remove_stale_socket_if_present(settings: &Settings) -> Result<(), RemoveSocketError> {
    remove_sockets(settings, |socket_path| {
        // A refused connection means the socket is left over from a daemon that is gone.
        matches!(
            StdUnixStream::connect(socket_path),
            Err(e) if e.kind() == ErrorKind::ConnectionRefused
        )
    })
}

async fn wait_until_ready(settings: &Settings, timeout: Duration) -> Result<HistoryClient> {
    Backoff::Constant(STARTUP_POLL)
        .retry(
            || async move {
                match probe(settings).await {
                    Probe::Ready(client) => ControlFlow::Break(Ok(client)),
                    Probe::NeedsRestart(reason) => ControlFlow::Continue(eyre!(reason)),
                    Probe::Unreachable(err) => {
                        if is_legacy_daemon_error(&err) {
                            ControlFlow::Break(Err(err.wrap_err(LEGACY_DAEMON_RESTART_MESSAGE)))
                        } else {
                            ControlFlow::Continue(err)
                        }
                    }
                }
            },
            timeout,
        )
        .await
        .unwrap_or_else(|last| {
            Err(last.wrap_err(format!(
                "timed out waiting for daemon startup after {}ms",
                timeout.as_millis()
            )))
        })
}

#[allow(clippy::unnecessary_wraps)]
fn ensure_autostart_supported(settings: &Settings) -> Result<()> {
    #[cfg(unix)]
    if settings.daemon.systemd_socket {
        bail!(
            "daemon autostart is incompatible with `daemon.systemd_socket = true`; use systemd to \
             manage the daemon"
        );
    }
    #[cfg(not(unix))]
    let _ = settings;

    Ok(())
}

/// The path to the lock used to prevent two clients from trying to start the daemon at the same
/// time.
#[must_use]
fn startup_lock_path(pidfile_path: &Path) -> PathBuf {
    let mut os = pidfile_path.as_os_str().to_os_string();
    os.push(".startup.lock");
    PathBuf::from(os)
}

/// What to do with the daemon recorded in the pidfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Autostart {
    /// A daemon is serving its socket; leave it alone.
    Keep,
    /// Nothing is holding the pidfile; spawn a daemon.
    Spawn,
    /// A daemon is alive but not serving; replace it.
    Replace,
}

/// Decide what to do with the daemon recorded in the pidfile.
///
/// Liveness is not health: a daemon can outlive its socket, and it holds the pidfile lock for its
/// whole lifetime, so that wedge blocks every later attempt to take the lock. A daemon that is
/// alive without serving therefore has to be replaced, not assumed healthy.
#[must_use]
fn autostart_action(daemon_alive: bool, socket_serving: bool) -> Autostart {
    match (daemon_alive, socket_serving) {
        (true, true) => Autostart::Keep,
        (true, false) => Autostart::Replace,
        (false, _) => Autostart::Spawn,
    }
}

/// Whether a daemon is still holding the pidfile's exclusive lock, which it does from the moment it
/// starts until it exits.
///
/// A held lock is the liveness signal here, because it belongs to the daemon itself: a PID read
/// from the pidfile can name a process that has long since exited and had its PID reused.
#[must_use]
fn daemon_holds_pidfile(pidfile_path: &Path) -> bool {
    let options = LockOptions {
        create: false,
        mode: LockMode::Exclusive,
    };

    match options.try_open(pidfile_path) {
        Err(LockError::WouldBlock) => true,
        Ok(_) => false,
        Err(e) => {
            tracing::debug!(path = %pidfile_path.display(), "could not lock daemon pidfile: {e}");
            false
        }
    }
}

/// Whether a connect to the daemon's socket succeeds within [`SERVING_PROBE_TIMEOUT`].
///
/// A missing, a refused and a non-answering socket all mean the same thing here: not serving.
#[cfg(unix)]
#[must_use]
async fn socket_is_serving(socket_path: &Path) -> bool {
    let connect = tokio::net::UnixStream::connect(socket_path);
    let socket = socket_path.display();

    match tokio::time::timeout(SERVING_PROBE_TIMEOUT, connect).await {
        Ok(Ok(_stream)) => true,
        outcome => {
            tracing::debug!("daemon socket {socket} is not serving: {outcome:?}");
            false
        }
    }
}

/// Whether a connect to the daemon's TCP port succeeds within [`SERVING_PROBE_TIMEOUT`].
#[cfg(not(unix))]
#[must_use]
async fn socket_is_serving(port: u64) -> bool {
    let address = format!("127.0.0.1:{port}");
    let connect = tokio::net::TcpStream::connect(address.clone());

    match tokio::time::timeout(SERVING_PROBE_TIMEOUT, connect).await {
        Ok(Ok(_stream)) => true,
        outcome => {
            tracing::debug!("daemon port {address} is not serving: {outcome:?}");
            false
        }
    }
}

/// Whether the socket the client connects to is serving, polling for up to `timeout`.
///
/// The grace period matters because a daemon locks the pidfile before it binds its socket, so a
/// daemon that is still booting looks exactly like a wedged one for as long as it takes to start.
/// The socket path is resolved on every attempt rather than once, because a daemon may come up on a
/// fallback path that only exists once it is listening.
async fn daemon_is_serving(settings: &Settings, timeout: Duration) -> bool {
    Backoff::Constant(STARTUP_POLL)
        .retry(
            || async move {
                #[cfg(unix)]
                let serving = socket_is_serving(&atuin_daemon::client::socket_path(settings)).await;
                #[cfg(not(unix))]
                let serving = socket_is_serving(settings.daemon.tcp_port).await;

                if serving {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
            timeout,
        )
        .await
        .is_ok()
}

/// Terminate a daemon that is alive but has stopped serving, freeing the pidfile for its
/// replacement, and report whether one was terminated.
///
/// Without this a wedged daemon is never replaced: it holds the pidfile lock for as long as it
/// runs, so the spawn path waits out the whole timeout on that lock and fails, on every command,
/// until somebody kills it by hand.
async fn replace_wedged_daemon(settings: &Settings) -> bool {
    let pidfile_path = Path::new(&settings.daemon.pidfile_path);
    let daemon_alive = daemon_holds_pidfile(pidfile_path);
    // Only a daemon that is alive can still start serving, so the grace period is only ever paid
    // in the one case where it can change the answer.
    let socket_serving = daemon_alive && daemon_is_serving(settings, SERVING_GRACE).await;

    let Autostart::Replace = autostart_action(daemon_alive, socket_serving) else {
        return false;
    };

    let Some(pid) = pidfile::try_read_pid(pidfile_path).filter(|pid| *pid != 0) else {
        return false;
    };

    tracing::warn!("daemon (pid {pid}) is not serving its socket; replacing it");
    if let Err(e) = atuin_common::os::process::force_terminate(pid, WEDGE_EXIT_TIMEOUT).await {
        tracing::warn!("could not terminate wedged daemon (pid {pid}): {e}");
    }

    true
}

/// Ensure the daemon is running, starting it if necessary.
///
/// If the daemon is already running and up-to-date, this is a no-op.
/// If it is not running or needs a restart, this will spawn a new daemon
/// process and wait for it to become ready.
///
/// Returns an error if the daemon could not be started.
pub async fn ensure_daemon_running(settings: &Settings) -> Result<()> {
    ensure_autostart_supported(settings)?;

    let timeout = startup_timeout(settings);
    let pidfile_path = PathBuf::from(&settings.daemon.pidfile_path);
    let startup_lock_path = startup_lock_path(&pidfile_path);
    let startup_lock = LockOptions {
        create: true,
        mode: LockMode::Exclusive,
    }
    .wait(&startup_lock_path, timeout)
    .await
    .wrap_err_with(|| {
        format!("failed to acquire daemon startup lock at {}", startup_lock_path.display())
    })?;

    match probe(settings).await {
        Probe::Ready(_) => {
            drop(startup_lock);
            return Ok(());
        }
        Probe::NeedsRestart(_) => {
            request_shutdown(settings).await;
        }
        Probe::Unreachable(err) => {
            if is_legacy_daemon_error(&err) {
                return Err(err.wrap_err(LEGACY_DAEMON_RESTART_MESSAGE));
            }

            replace_wedged_daemon(settings).await;
        }
    }

    // This prevents rapid-fire hook invocations from racing daemon restart.
    wait_for_pidfile_available(&pidfile_path, timeout).await?;

    #[cfg(unix)]
    remove_stale_socket_if_present(settings)?;

    spawn_daemon_process()?;
    let _ = wait_until_ready(settings, timeout).await?;

    drop(startup_lock);
    Ok(())
}

async fn restart_daemon(settings: &Settings) -> Result<HistoryClient> {
    ensure_daemon_running(settings).await?;
    connect_client(settings).await
}

fn ensure_reply_compatible(settings: &Settings, version: &str, protocol: u32) -> Result<()> {
    if daemon_matches_expected(version, protocol) {
        return Ok(());
    }

    let message = daemon_mismatch_message(version, protocol);
    if settings.daemon.autostart {
        bail!("{message}");
    }

    bail!("{message}. Enable `daemon.autostart = true` or restart the daemon manually");
}

/// Acquire a [`HistoryClient`] connected to a daemon we expect to understand us, probing over the
/// wire-stable Status RPC first and restarting the daemon if it is absent or version/protocol
/// skewed.
///
/// TODO(markovejnovic): This is egregious slop, but not worse than the original solution. I want to
///                      remove this in a future PR: <https://github.com/atuinsh/atuin/pull/4002>
pub async fn ready_client(settings: &Settings) -> Result<HistoryClient> {
    match probe(settings).await {
        Probe::Ready(client) => return Ok(client),
        Probe::NeedsRestart(reason) if !settings.daemon.autostart => {
            bail!("{reason}. Enable `daemon.autostart = true` or restart the daemon manually");
        }
        Probe::Unreachable(err) if is_legacy_daemon_error(&err) => {
            return Err(err.wrap_err(LEGACY_DAEMON_RESTART_MESSAGE));
        }
        Probe::Unreachable(err) if !settings.daemon.autostart => return Err(err),
        Probe::Unreachable(err) if !should_retry_after_error(&err) => return Err(err),
        Probe::NeedsRestart(_) | Probe::Unreachable(_) => {}
    }

    restart_daemon(settings).await
}

/// Send a request to the daemon, first ensuring (via [`ready_client`]) that it is running and
/// speaks our version.
async fn try_with_restart<F, R>(settings: &Settings, send_request: F) -> Result<R>
where
    F: AsyncFnOnce(&mut HistoryClient) -> Result<R> + Sync,
    R: atuin_daemon::grpc::VersionedReply,
{
    let client = ready_client(settings).await?;
    send_checked(settings, client, send_request).await
}

/// Send a request to an already-running daemon that speaks our version.
///
/// This function never starts or restarts the daemon.
async fn try_without_restart<F, R>(settings: &Settings, send_request: F) -> Result<R>
where
    F: AsyncFnOnce(&mut HistoryClient) -> Result<R> + Sync,
    R: atuin_daemon::grpc::VersionedReply,
{
    let client = match probe(settings).await {
        Probe::Ready(client) => client,
        Probe::NeedsRestart(reason) => bail!(reason),
        Probe::Unreachable(err) => return Err(err),
    };
    send_checked(settings, client, send_request).await
}

/// Send a message to the daemon and ensure the response is [compatible](ensure_reply_compatible).
async fn send_checked<F, R>(
    settings: &Settings,
    mut client: HistoryClient,
    send_request: F,
) -> Result<R>
where
    F: AsyncFnOnce(&mut HistoryClient) -> Result<R> + Sync,
    R: atuin_daemon::grpc::VersionedReply,
{
    let resp = send_request(&mut client).await?;
    ensure_reply_compatible(settings, resp.version(), resp.protocol())?;
    Ok(resp)
}

pub async fn start_history(settings: &Settings, history: History) -> Result<HistoryId> {
    let resp =
        try_with_restart(settings, async |client| client.start_history(history).await).await?;
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

pub async fn compact_store(settings: &Settings) -> Result<u64> {
    let reply = try_with_restart(settings, async |client| client.compact_store().await).await?;
    Ok(reply.rewritten)
}

async fn status_cmd(settings: &Settings) -> Result<()> {
    match probe(settings).await {
        Probe::Ready(mut client) => {
            let status = client.status().await?;
            println!("Daemon running");
            println!("  PID:      {}", status.pid);
            println!("  Version:  {}", status.version);
            println!("  Protocol: {}", status.protocol);
            println!("  Healthy:  {}", status.healthy);
            #[cfg(unix)]
            println!("  Socket:   {}", atuin_daemon::client::socket_path(settings).display());
            #[cfg(not(unix))]
            println!("  Port:     {}", settings.daemon.tcp_port);
        }
        Probe::NeedsRestart(reason) => {
            println!("Daemon running (needs restart)");
            println!("  Reason: {reason}");
        }
        Probe::Unreachable(_) => {
            println!("Daemon is not running");
        }
    }

    Ok(())
}

async fn stop_cmd(settings: &Settings) -> Result<()> {
    let Ok(mut client) = connect_client(settings).await else {
        println!("Daemon is not running");
        return Ok(());
    };

    match client.shutdown().await {
        Ok(true) => {
            println!("Shutdown requested");

            let pidfile_path = PathBuf::from(&settings.daemon.pidfile_path);
            let timeout = Duration::from_secs(5);
            match wait_for_pidfile_available(&pidfile_path, timeout).await {
                Ok(()) => println!("Daemon stopped"),
                Err(_) => println!("Daemon may still be shutting down"),
            }

            Ok(())
        }
        Ok(false) => bail!("Daemon rejected shutdown request"),
        Err(err) => Err(err.wrap_err("Failed to send shutdown request")),
    }
}

pub(super) async fn restart_cmd(settings: &Settings) -> Result<()> {
    // Stop if running
    match probe(settings).await {
        Probe::Ready(_) | Probe::NeedsRestart(_) => {
            request_shutdown(settings).await;
            println!("Stopping daemon...");

            let pidfile_path = PathBuf::from(&settings.daemon.pidfile_path);
            let timeout = Duration::from_secs(5);
            wait_for_pidfile_available(&pidfile_path, timeout)
                .await
                .wrap_err("Timed out waiting for old daemon to stop")?;
        }
        Probe::Unreachable(_) => {
            if replace_wedged_daemon(settings).await {
                println!("Replaced a daemon that was no longer serving");
            } else {
                println!("No daemon running");
            }
        }
    }

    #[cfg(unix)]
    remove_stale_socket_if_present(settings)?;

    spawn_daemon_process()?;
    println!("Starting daemon...");

    let timeout = startup_timeout(settings);
    let status = wait_until_ready(settings, timeout).await?.status().await?;

    println!("Daemon restarted");
    println!("  PID:      {}", status.pid);
    println!("  Version:  {}", status.version);

    Ok(())
}

/// Daemonize the current process. Must be called before creating the tokio
/// runtime or opening database connections, since `fork()` inside an async
/// runtime corrupts its internal state.
#[cfg(unix)]
pub fn daemonize_current_process() -> Result<()> {
    let cwd =
        std::env::current_dir().wrap_err("could not determine current directory for daemon")?;

    Daemonize::new().working_directory(cwd).start().wrap_err("failed to daemonize process")?;

    Ok(())
}

async fn run(
    settings: Settings,
    store: SqliteStore,
    history_db: Sqlite,
    force: bool,
) -> Result<()> {
    if force {
        force_cleanup(&settings).await;
    }

    let _pidfile_guard = PidfileGuard::acquire(&settings.daemon)?;

    atuin_daemon::boot(settings, store, history_db).await?;

    Ok(())
}

/// Force cleanup: kill existing daemon process and remove socket.
async fn force_cleanup(settings: &Settings) {
    let pidfile_path = Path::new(&settings.daemon.pidfile_path);

    // Read and kill the existing process if pidfile exists
    if pidfile_path.exists() {
        if let Some(pid) = pidfile::try_read_pid(pidfile_path)
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

    // Remove the socket files
    #[cfg(unix)]
    if let Err(e) = remove_sockets(settings, |_| true) {
        tracing::warn!("{e}");
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    #[fixture]
    fn tmp_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Liveness on its own is not health: a daemon that is alive but has stopped serving holds the
    /// pidfile lock for as long as it runs, so it has to be replaced instead of assumed healthy.
    #[rstest]
    #[case::no_daemon(false, false, Autostart::Spawn)]
    #[case::serving_daemon(true, true, Autostart::Keep)]
    #[case::wedged_daemon(true, false, Autostart::Replace)]
    fn autostart_replaces_only_a_daemon_that_is_alive_without_serving(
        #[case] daemon_alive: bool,
        #[case] socket_serving: bool,
        #[case] expected: Autostart,
    ) {
        assert_eq!(autostart_action(daemon_alive, socket_serving), expected);
    }

    /// The lock is what says the daemon is alive, since a PID left in the pidfile can name a
    /// process that has long since exited.
    #[rstest]
    fn a_daemon_is_alive_for_as_long_as_it_holds_the_pidfile_lock(tmp_dir: TempDir) {
        let pidfile = tmp_dir.path().join("atuin-daemon.pid");

        assert!(!daemon_holds_pidfile(&pidfile), "no pidfile, no daemon");

        let held = LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .open(&pidfile)
        .unwrap();

        assert!(daemon_holds_pidfile(&pidfile), "a running daemon holds the lock");
        drop(held);
        assert!(!daemon_holds_pidfile(&pidfile), "the lock dies with the daemon");
    }

    /// What tells a serving daemon from a wedged one is a connect, so the check has to follow
    /// whatever is, or isn't, listening on the socket.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_socket_is_serving_only_while_something_listens_on_it(tmp_dir: TempDir) {
        let socket_path = tmp_dir.path().join("atuin.sock");

        assert!(!socket_is_serving(&socket_path).await, "no socket at all");

        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        drop(listener);
        fs::remove_file(&socket_path).unwrap();
        assert!(!socket_is_serving(&socket_path).await, "socket left behind");

        let _listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        assert!(socket_is_serving(&socket_path).await, "daemon listening");
    }

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

    #[rstest]
    #[case::matches(VERSION, PROTOCOL_VERSION, true)]
    #[case::wrong_version("0.0.0", PROTOCOL_VERSION, false)]
    #[case::wrong_protocol(VERSION, 999, false)]
    #[case::wrong_both("0.0.0", 999, false)]
    fn daemon_matches_expected_cases(
        #[case] version: &str,
        #[case] protocol: u32,
        #[case] expected: bool,
    ) {
        assert_eq!(daemon_matches_expected(version, protocol), expected);
    }

    #[rstest]
    #[case::out_of_date("0.0.0", PROTOCOL_VERSION, vec!["out of date", "0.0.0", VERSION])]
    #[case::protocol_mismatch(VERSION, 999, vec!["protocol mismatch"])]
    fn daemon_mismatch_message_cases(
        #[case] version: &str,
        #[case] protocol: u32,
        #[case] needles: Vec<&str>,
    ) {
        let msg = daemon_mismatch_message(version, protocol);
        for needle in needles {
            assert!(msg.contains(needle), "got: {msg}");
        }
    }

    #[rstest]
    #[case("/tmp/atuin-daemon.pid", "/tmp/atuin-daemon.pid.startup.lock")]
    #[case("/path/to/pidfile", "/path/to/pidfile.startup.lock")]
    fn test_startup_lock_path(#[case] pidfile_path: &str, #[case] expected: &str) {
        assert_eq!(startup_lock_path(Path::new(pidfile_path)), Path::new(expected));
    }
}
