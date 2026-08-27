use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::ops::ControlFlow;
#[cfg(unix)]
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::history::History;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::futures::Backoff;
use atuin_daemon::DaemonEvent;
use atuin_daemon::client::{ControlClient, DaemonClientErrorKind, HistoryClient, classify_error};
use clap::Subcommand;
#[cfg(unix)]
use daemonize::Daemonize;
use eyre::{Result, WrapErr, bail, eyre};

#[derive(clap::Args, Debug)]
pub struct Cmd {
    /// Internal flag for daemonization
    #[arg(long, hide = true)]
    daemonize: bool,

    /// Also write daemon logs to the console (useful for debugging)
    #[arg(long)]
    show_logs: bool,

    #[command(subcommand)]
    subcmd: Option<SubCmd>,
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum SubCmd {
    /// Start the daemon server
    Start {
        #[arg(long, hide = true)]
        daemonize: bool,

        /// Also write daemon logs to the console (useful for debugging)
        #[arg(long)]
        show_logs: bool,

        /// Force start: kill existing daemon process and reset the socket
        #[arg(long)]
        force: bool,
    },

    /// Show the daemon's current status
    Status,

    /// Stop the daemon gracefully
    Stop,

    /// Restart the daemon (stop, then start in background)
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

const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");
const DAEMON_PROTOCOL_VERSION: u32 = 1;
const STARTUP_POLL: Duration = Duration::from_millis(40);
const LOCK_POLL: Duration = Duration::from_millis(20);
const LEGACY_DAEMON_RESTART_MESSAGE: &str = "legacy daemon detected; restart daemon manually";

struct PidfileGuard {
    file: File,
}

impl PidfileGuard {
    fn acquire(path: &Path) -> Result<Self> {
        let mut file = open_lock_file(path)?;

        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                bail!("daemon already running (pidfile lock busy at {})", path.display())
            }
            Err(TryLockError::Error(err)) => {
                return Err(err)
                    .wrap_err_with(|| format!("could not lock daemon pidfile {}", path.display()));
            }
        }

        file.set_len(0)
            .wrap_err_with(|| format!("could not truncate daemon pidfile {}", path.display()))?;
        writeln!(file, "{}", std::process::id())
            .and_then(|()| writeln!(file, "{DAEMON_VERSION}"))
            .wrap_err_with(|| format!("could not write daemon pidfile {}", path.display()))?;

        Ok(Self { file })
    }
}

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

enum Probe {
    Ready(HistoryClient),
    NeedsRestart(String),
    Unreachable(eyre::Report),
}

fn daemon_matches_expected(version: &str, protocol: u32) -> bool {
    version == DAEMON_VERSION && protocol == DAEMON_PROTOCOL_VERSION
}

fn daemon_mismatch_message(version: &str, protocol: u32) -> String {
    if protocol == DAEMON_PROTOCOL_VERSION {
        format!("daemon is out of date: expected {DAEMON_VERSION}, got {version}")
    } else {
        format!("daemon protocol mismatch: expected {DAEMON_PROTOCOL_VERSION}, got {protocol}")
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

fn daemon_startup_lock_path(pidfile_path: &Path) -> PathBuf {
    let mut os = pidfile_path.as_os_str().to_os_string();
    os.push(".startup.lock");
    PathBuf::from(os)
}

fn open_lock_file(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("could not create lock directory {}", parent.display()))?;
    }

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .wrap_err_with(|| format!("could not open lock file {}", path.display()))
}

async fn wait_for_lock(path: &Path, timeout: Duration) -> Result<File> {
    let file = open_lock_file(path)?;

    let outcome = Backoff::Linear(LOCK_POLL)
        .retry_sync(
            || match file.try_lock() {
                Ok(()) => ControlFlow::Break(Ok(())),
                Err(TryLockError::WouldBlock) => ControlFlow::Continue(()),
                Err(TryLockError::Error(err)) => {
                    ControlFlow::Break(Err(eyre!("could not lock {}: {err}", path.display())))
                }
            },
            timeout,
        )
        .await;

    match outcome {
        Ok(Ok(())) => Ok(file),
        Ok(Err(err)) => Err(err),
        Err(()) => bail!("timed out waiting for lock at {}", path.display()),
    }
}

async fn wait_for_pidfile_available(path: &Path, timeout: Duration) -> Result<()> {
    let file = wait_for_lock(path, timeout).await?;
    file.unlock().wrap_err_with(|| format!("failed to unlock {}", path.display()))?;
    Ok(())
}

async fn connect_client(settings: &Settings) -> Result<HistoryClient> {
    HistoryClient::new(
        #[cfg(not(unix))]
        settings.daemon.tcp_port,
        #[cfg(unix)]
        settings.daemon.existing_socket_path().into_owned(),
    )
    .await
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

fn startup_timeout(settings: &Settings) -> Result<Duration> {
    Duration::try_from_secs_f64(settings.local_timeout.max(0.5) + 2.0)
        .wrap_err("invalid local_timeout setting")
}

/// An error that occurred while trying to remove a socket.
#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
#[error("failed to remove daemon socket {}: {source}", .path.display())]
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
    Backoff::Linear(STARTUP_POLL)
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

/// Ensure the daemon is running, starting it if necessary.
///
/// If the daemon is already running and up-to-date, this is a no-op.
/// If it is not running or needs a restart, this will spawn a new daemon
/// process and wait for it to become ready.
///
/// Returns an error if the daemon could not be started.
pub async fn ensure_daemon_running(settings: &Settings) -> Result<()> {
    ensure_autostart_supported(settings)?;

    let timeout = startup_timeout(settings)?;
    let pidfile_path = PathBuf::from(&settings.daemon.pidfile_path);
    let startup_lock_path = daemon_startup_lock_path(&pidfile_path);
    let startup_lock = wait_for_lock(&startup_lock_path, timeout).await?;

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

#[derive(Clone, Copy)]
struct TryWithRestartOptions {
    /// Whether to resend the command if the daemon isn't the expected version.
    pub retry_on_version_mismatch: bool,
}

/// Try to send a request to the daemon, restarting it and retrying if necessary.
///
/// `context` will be passed to the closure. Compared to capturing the needed context in the
/// closure, this may reduce the number of required clones.
async fn try_with_restart<C, F, R>(
    settings: &Settings,
    send_request: F,
    context: C,
    options: TryWithRestartOptions,
) -> Result<R>
where
    C: Clone + Sync,
    F: AsyncFn(&mut HistoryClient, C) -> Result<R> + Sync,
    R: atuin_daemon::history::VersionedReply,
{
    match async { send_request(&mut connect_client(settings).await?, context.clone()).await }.await
    {
        Ok(resp) => {
            if daemon_matches_expected(resp.version(), resp.protocol()) {
                return Ok(resp);
            }

            if !settings.daemon.autostart {
                return Err(eyre!(
                    "{}. Enable `daemon.autostart = true` or restart the daemon manually",
                    daemon_mismatch_message(resp.version(), resp.protocol())
                ));
            }

            if !options.retry_on_version_mismatch {
                // We don't need to retry the request, so only restart to make subsequent hook calls
                // target the expected version.
                let _ = restart_daemon(settings).await;
                return Ok(resp);
            }
        }
        Err(err) if !settings.daemon.autostart => return Err(err),
        Err(err) if !should_retry_after_error(&err) => return Err(err),
        Err(_) => {}
    }

    let resp = send_request(&mut restart_daemon(settings).await?, context).await?;
    ensure_reply_compatible(settings, resp.version(), resp.protocol())?;
    Ok(resp)
}

pub async fn start_history(settings: &Settings, history: History) -> Result<String> {
    let resp = try_with_restart(
        settings,
        async |client, history| client.start_history(history).await,
        history,
        TryWithRestartOptions {
            retry_on_version_mismatch: true,
        },
    )
    .await?;
    Ok(resp.id)
}

pub async fn end_history(settings: &Settings, id: String, duration: u64, exit: i64) -> Result<()> {
    try_with_restart(
        settings,
        async |client, id| client.end_history(id, duration, exit).await,
        id,
        TryWithRestartOptions {
            retry_on_version_mismatch: false,
        },
    )
    .await?;
    Ok(())
}

pub async fn cancel_history(settings: &Settings, id: String) -> Result<()> {
    try_with_restart(
        settings,
        async |client, id| client.cancel_history(id).await,
        id,
        TryWithRestartOptions {
            retry_on_version_mismatch: false,
        },
    )
    .await?;
    Ok(())
}

/// Emit a daemon event, auto-starting the daemon if it is not running.
///
/// If the daemon is not reachable and `daemon.autostart` is enabled, this
/// will start the daemon and retry the event. If the daemon cannot be
/// started or the retry fails, a warning is printed to stderr.
pub async fn emit_event(settings: &Settings, event: DaemonEvent) {
    // Try to connect and send
    match ControlClient::from_settings(settings).await {
        Ok(mut client) => {
            if let Err(e) = client.send_event(event).await {
                tracing::debug!(?e, "failed to send event to daemon");
            }
            return;
        }
        Err(e) if !settings.daemon.autostart || !should_retry_after_error(&e) => {
            tracing::debug!(?e, "daemon not available, skipping event emission");
            return;
        }
        Err(_) => {}
    }

    // Auto-start the daemon and retry
    if let Err(e) = ensure_daemon_running(settings).await {
        eprintln!("Could not start daemon: {e}");
        return;
    }

    match ControlClient::from_settings(settings).await {
        Ok(mut client) => {
            if let Err(e) = client.send_event(event).await {
                eprintln!("Daemon started but failed to send event: {e}");
            }
        }
        Err(e) => {
            eprintln!("Daemon started but failed to connect: {e}");
        }
    }
}

pub async fn tail_client(settings: &Settings) -> Result<HistoryClient> {
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
            println!("  Socket:   {}", settings.daemon.existing_socket_path().display());
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

async fn restart_cmd(settings: &Settings) -> Result<()> {
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
            println!("No daemon running");
        }
    }

    #[cfg(unix)]
    remove_stale_socket_if_present(settings)?;

    spawn_daemon_process()?;
    println!("Starting daemon...");

    let timeout = startup_timeout(settings)?;
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

    let pidfile_path = PathBuf::from(&settings.daemon.pidfile_path);
    let _pidfile_guard = PidfileGuard::acquire(&pidfile_path)?;

    atuin_daemon::boot(settings, store, history_db).await?;

    Ok(())
}

/// Force cleanup: kill existing daemon process and remove socket.
async fn force_cleanup(settings: &Settings) {
    let pidfile_path = Path::new(&settings.daemon.pidfile_path);

    // Read and kill the existing process if pidfile exists
    if pidfile_path.exists() {
        if let Ok(contents) = fs::read_to_string(pidfile_path)
            && let Some(pid_str) = contents.lines().next()
            && let Ok(pid) = pid_str.parse::<u32>()
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

    use super::*;

    #[rstest]
    #[case::matches(DAEMON_VERSION, DAEMON_PROTOCOL_VERSION, true)]
    #[case::wrong_version("0.0.0", DAEMON_PROTOCOL_VERSION, false)]
    #[case::wrong_protocol(DAEMON_VERSION, 999, false)]
    #[case::wrong_both("0.0.0", 999, false)]
    fn daemon_matches_expected_cases(
        #[case] version: &str,
        #[case] protocol: u32,
        #[case] expected: bool,
    ) {
        assert_eq!(daemon_matches_expected(version, protocol), expected);
    }

    #[rstest]
    #[case::out_of_date("0.0.0", DAEMON_PROTOCOL_VERSION, vec!["out of date", "0.0.0", DAEMON_VERSION])]
    #[case::protocol_mismatch(DAEMON_VERSION, 999, vec!["protocol mismatch"])]
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

    #[test]
    fn test_startup_lock_path() {
        let pidfile = Path::new("/tmp/atuin-daemon.pid");
        let lock = daemon_startup_lock_path(pidfile);
        assert_eq!(lock, PathBuf::from("/tmp/atuin-daemon.pid.startup.lock"));
    }

    #[fixture]
    fn pidfile() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("daemon.pid");
        (tmp, path)
    }

    #[rstest]
    fn test_pidfile_guard_acquire_and_drop(
        #[from(pidfile)] (_tmp, pidfile): (tempfile::TempDir, PathBuf),
    ) {
        {
            let _guard = PidfileGuard::acquire(&pidfile).unwrap();
            // Guard holds an exclusive lock — on Windows other handles cannot
            // read the file, so we verify contents after the guard is dropped.
        }

        let contents = std::fs::read_to_string(&pidfile).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], std::process::id().to_string());
        assert_eq!(lines[1], DAEMON_VERSION);

        // After guard is dropped, lock should be released — acquiring again must succeed.
        let _guard2 = PidfileGuard::acquire(&pidfile).unwrap();
    }

    #[rstest]
    fn test_pidfile_guard_prevents_double_acquire(
        #[from(pidfile)] (_tmp, pidfile): (tempfile::TempDir, PathBuf),
    ) {
        let _guard = PidfileGuard::acquire(&pidfile).unwrap();
        let result = PidfileGuard::acquire(&pidfile);
        assert!(result.is_err());
    }
}
