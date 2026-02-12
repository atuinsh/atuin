use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use atuin_client::{
    database::Sqlite, history::History, record::sqlite_store::SqliteStore, settings::Settings,
};
use atuin_daemon::{
    client::{DaemonClientErrorKind, HistoryClient, classify_error},
    server::listen,
};
#[cfg(unix)]
use daemonize::Daemonize;
use eyre::{Result, WrapErr, bail, eyre};
use fs4::fs_std::FileExt;
use tokio::time::sleep;

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

        if !file.try_lock_exclusive()? {
            bail!(
                "daemon already running (pidfile lock busy at {})",
                path.display()
            );
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

fn should_retry_after_error(err: &eyre::Report) -> bool {
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
    let start = Instant::now();

    loop {
        match file.try_lock_exclusive() {
            Ok(true) => return Ok(file),
            Ok(false) => {
                if start.elapsed() >= timeout {
                    bail!("timed out waiting for lock at {}", path.display());
                }

                sleep(LOCK_POLL).await;
            }
            Err(err) => {
                return Err(eyre!("could not lock {}: {err}", path.display()));
            }
        }
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
        settings.daemon.socket_path.clone(),
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
            if !daemon_matches_expected(&status.version, status.protocol) {
                Probe::NeedsRestart(daemon_mismatch_message(&status.version, status.protocol))
            } else {
                Probe::Ready(client)
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
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    cmd.arg("--daemonize");

    cmd.spawn().wrap_err("failed to spawn daemon process")?;

    Ok(())
}

fn startup_timeout(settings: &Settings) -> Duration {
    Duration::from_secs_f64(settings.local_timeout.max(0.5) + 2.0)
}

#[cfg(unix)]
fn remove_stale_socket_if_present(settings: &Settings) -> Result<()> {
    if settings.daemon.systemd_socket {
        return Ok(());
    }

    let socket_path = Path::new(&settings.daemon.socket_path);
    if !socket_path.exists() {
        return Ok(());
    }

    match StdUnixStream::connect(socket_path) {
        Ok(stream) => {
            drop(stream);
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::ConnectionRefused => {
            fs::remove_file(socket_path).wrap_err_with(|| {
                format!(
                    "failed to remove stale daemon socket {}",
                    socket_path.display()
                )
            })?;
            Ok(())
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Ok(()),
    }
}

async fn wait_until_ready(settings: &Settings, timeout: Duration) -> Result<HistoryClient> {
    let start = Instant::now();
    let mut last_error = eyre!("daemon did not become ready");

    loop {
        match probe(settings).await {
            Probe::Ready(client) => return Ok(client),
            Probe::NeedsRestart(reason) => {
                last_error = eyre!(reason);
            }
            Probe::Unreachable(err) => {
                if is_legacy_daemon_error(&err) {
                    return Err(err.wrap_err(LEGACY_DAEMON_RESTART_MESSAGE));
                }
                last_error = err;
            }
        }

        if start.elapsed() >= timeout {
            return Err(last_error.wrap_err(format!(
                "timed out waiting for daemon startup after {}ms",
                timeout.as_millis()
            )));
        }

        sleep(STARTUP_POLL).await;
    }
}

fn ensure_autostart_supported(settings: &Settings) -> Result<()> {
    #[cfg(unix)]
    if settings.daemon.systemd_socket {
        bail!(
            "daemon autostart is incompatible with `daemon.systemd_socket = true`; use systemd to manage the daemon"
        );
    }
    #[cfg(not(unix))]
    let _ = settings;

    Ok(())
}

async fn restart_daemon(settings: &Settings) -> Result<HistoryClient> {
    ensure_autostart_supported(settings)?;

    let timeout = startup_timeout(settings);
    let pidfile_path = PathBuf::from(&settings.daemon.pidfile_path);
    let startup_lock_path = daemon_startup_lock_path(&pidfile_path);
    let startup_lock = wait_for_lock(&startup_lock_path, timeout).await?;

    match probe(settings).await {
        Probe::Ready(client) => {
            drop(startup_lock);
            return Ok(client);
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
    let client = wait_until_ready(settings, timeout).await?;

    drop(startup_lock);
    Ok(client)
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

pub async fn start_history(settings: &Settings, history: History) -> Result<String> {
    match async { connect_client(settings).await?.start_history(history.clone()).await }.await {
        Ok(resp) => {
            if daemon_matches_expected(&resp.version, resp.protocol) {
                return Ok(resp.id);
            }

            if !settings.daemon.autostart {
                return Err(eyre!(
                    "{}. Enable `daemon.autostart = true` or restart the daemon manually",
                    daemon_mismatch_message(&resp.version, resp.protocol)
                ));
            }
        }
        Err(err) if !settings.daemon.autostart => return Err(err),
        Err(err) if !should_retry_after_error(&err) => return Err(err),
        Err(_) => {}
    }

    let resp = restart_daemon(settings)
        .await?
        .start_history(history)
        .await?;
    ensure_reply_compatible(settings, &resp.version, resp.protocol)?;
    Ok(resp.id)
}

pub async fn end_history(settings: &Settings, id: String, duration: u64, exit: i64) -> Result<()> {
    match async { connect_client(settings).await?.end_history(id.clone(), duration, exit).await }.await {
        Ok(resp) => {
            if daemon_matches_expected(&resp.version, resp.protocol) {
                return Ok(());
            }

            if !settings.daemon.autostart {
                return Err(eyre!(
                    "{}. Enable `daemon.autostart = true` or restart the daemon manually",
                    daemon_mismatch_message(&resp.version, resp.protocol)
                ));
            }

            // End succeeded on the running daemon, so avoid replaying it.
            // We only restart to make subsequent hook calls target the expected version.
            let _ = restart_daemon(settings).await;
            return Ok(());
        }
        Err(err) if !settings.daemon.autostart => return Err(err),
        Err(err) if !should_retry_after_error(&err) => return Err(err),
        Err(_) => {}
    }

    let resp = restart_daemon(settings)
        .await?
        .end_history(id, duration, exit)
        .await?;
    ensure_reply_compatible(settings, &resp.version, resp.protocol)?;
    Ok(())
}

#[cfg(unix)]
fn daemonize_current_process() -> Result<()> {
    let cwd =
        std::env::current_dir().wrap_err("could not determine current directory for daemon")?;

    Daemonize::new()
        .working_directory(cwd)
        .start()
        .wrap_err("failed to daemonize process")?;

    Ok(())
}

pub async fn run(
    settings: Settings,
    store: SqliteStore,
    history_db: Sqlite,
    daemonize: bool,
) -> Result<()> {
    #[cfg(unix)]
    if daemonize {
        daemonize_current_process()?;
    }
    #[cfg(not(unix))]
    let _ = daemonize;

    let pidfile_path = PathBuf::from(&settings.daemon.pidfile_path);
    let _pidfile_guard = PidfileGuard::acquire(&pidfile_path)?;

    listen(settings, store, history_db).await?;

    Ok(())
}
