use atuin_client::settings::Settings;
use eyre::{Context, Result};

use crate::components::history::HistoryGrpcService;
use crate::components::search::SearchGrpcService;
use crate::components::semantic::SemanticGrpcService;
use crate::control::ControlService;
use crate::control::control_server::ControlServer;
use crate::daemon::DaemonHandle;
use crate::history::history_server::HistoryServer;
use crate::search::search_server::SearchServer;
use crate::semantic::semantic_server::SemanticServer;

/// How often to update the socket's modification time so it doesn't get automatically deleted by
/// temporary file cleaners.
#[cfg(unix)]
const SOCKET_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

/// Run the gRPC server with the given services.
///
/// This starts the gRPC server in the background and returns immediately.
/// The server will shut down when a ShutdownRequested event is received.
#[cfg(unix)]
#[allow(clippy::unused_async, reason = "needs to match the cfg(not(unix)) version")]
pub async fn run_grpc_server(
    settings: Settings,
    history_service: HistoryServer<HistoryGrpcService>,
    search_service: SearchServer<SearchGrpcService>,
    semantic_service: SemanticServer<SemanticGrpcService>,
    control_service: ControlServer<ControlService>,
    handle: DaemonHandle,
) -> Result<()> {
    use tokio_stream::wrappers::UnixListenerStream;

    let socket_path = settings.daemon.socket_path();

    let (uds, cleanup_path) = if cfg!(target_os = "linux") && settings.daemon.systemd_socket {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::net::SocketAddr;
            use std::path::PathBuf;

            use eyre::{OptionExt, WrapErr};
            use tokio::net::UnixListener;

            tracing::info!("getting systemd socket");
            let listener = listenfd::ListenFd::from_env()
                .take_unix_listener(0)?
                .ok_or_eyre("missing systemd socket")?;
            listener.set_nonblocking(true)?;
            let actual_path: Result<PathBuf, eyre::Report> = listener
                .local_addr()
                .context("getting systemd socket's path")
                .and_then(|addr: SocketAddr| {
                    addr.as_pathname()
                        .ok_or_eyre("systemd socket missing path")
                        .map(|path: &std::path::Path| path.to_owned())
                });
            match actual_path {
                Ok(actual_path) => {
                    tracing::info!("listening on systemd socket: {actual_path:?}");
                    if actual_path != socket_path.as_path() {
                        tracing::warn!(
                            "systemd socket is not at configured client path: {:?}",
                            socket_path.as_path(),
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "could not detect systemd socket path, ensure that it's at the configured \
                         path: {:?}, error: {err:?}",
                        socket_path.as_path(),
                    );
                }
            }
            (UnixListener::from_std(listener)?, None)
        }
        #[cfg(not(target_os = "linux"))]
        unreachable!()
    } else {
        use atuin_common::path::DisplayRichExt;

        socket_path.create_default_dir_if_needed()?;
        tracing::info!("listening on unix socket {:?}", socket_path.as_path());
        let listener = bind_reclaiming_stale_socket(socket_path.as_path())
            .context(format!("reading socket: {}", socket_path.display_rich().relative_to_cwd()))?;
        (listener, Some(socket_path.into_owned()))
    };

    let uds_stream = UnixListenerStream::new(uds);

    // Periodically update the socket's modification time so it doesn't get automatically deleted by
    // temporary file cleaners.
    let socket_updater = cleanup_path.clone().map(|path| {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SOCKET_KEEPALIVE_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(e) = atuin_common::os::unix::touch_file(&path) {
                    tracing::warn!("failed to refresh {}: {e}", path.display());
                }
            }
        })
    });

    // Create shutdown signal from daemon handle
    let shutdown_signal = async move {
        let mut rx = handle.subscribe();
        loop {
            use crate::DaemonEvent;

            match rx.recv().await {
                Ok(DaemonEvent::ShutdownRequested) => break,
                Ok(_) => continue,
                Err(_) => break, // Channel closed
            }
        }
        if let Some(handle) = socket_updater {
            handle.abort();
        }
        if let Some(path) = cleanup_path {
            eprintln!("Removing socket...");
            if let Err(e) = std::fs::remove_file(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!("failed to remove socket: {e}");
            }
        }
        eprintln!("Shutting down gRPC server...");
    };

    // Spawn the server in the background
    tokio::spawn(async move {
        use tonic::transport::Server;

        if let Err(e) = Server::builder()
            .add_service(history_service)
            .add_service(search_service)
            .add_service(semantic_service)
            .add_service(control_service)
            .serve_with_incoming_shutdown(uds_stream, shutdown_signal)
            .await
        {
            tracing::error!("gRPC server error: {e}");
        }
    });

    Ok(())
}

/// Bind a Unix socket at `path`, reclaiming it first if a previous daemon left it behind.
///
/// `bind` fails with [`std::io::ErrorKind::AddrInUse`] whenever the path already exists, whether
/// or not anything is still listening on it. A daemon that dies without running its shutdown
/// handler (killed by an OOM killer, say) leaves its socket file in place, and every start after
/// that fails to bind, so the daemon can never come back on its own.
///
/// Unlinking unconditionally would be worse than the bug it fixes: a second daemon would steal the
/// socket of a healthy first one, leaving that one running but serving nobody. So we only remove a
/// socket we have established is dead. `connect` failing with
/// [`std::io::ErrorKind::ConnectionRefused`] means the file exists but nothing is listening on it.
/// A successful `connect` means another daemon owns it, and any other error is not evidence either
/// way, so in both of those cases we leave the socket alone.
#[cfg(unix)]
fn bind_reclaiming_stale_socket(
    path: &std::path::Path,
) -> std::io::Result<tokio::net::UnixListener> {
    use std::io::{Error, ErrorKind};
    use std::os::unix::net::UnixStream;

    use tokio::net::UnixListener;

    let bind_error = match UnixListener::bind(path) {
        Ok(listener) => return Ok(listener),
        Err(e) if e.kind() == ErrorKind::AddrInUse => e,
        Err(e) => return Err(e),
    };

    match UnixStream::connect(path) {
        Ok(_) => {
            let msg = format!("another daemon is already listening on {}", path.display());
            Err(Error::new(ErrorKind::AddrInUse, msg))
        }
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
            tracing::warn!("removing stale socket left behind at {}", path.display());
            std::fs::remove_file(path)?;
            UnixListener::bind(path)
        }
        Err(e) => {
            tracing::warn!("could not probe socket at {}: {e}", path.display());
            Err(bind_error)
        }
    }
}

/// Run the gRPC server with the given services (Windows/TCP version).
#[cfg(not(unix))]
pub async fn run_grpc_server(
    settings: Settings,
    history_service: HistoryServer<HistoryGrpcService>,
    search_service: SearchServer<SearchGrpcService>,
    semantic_service: SemanticServer<SemanticGrpcService>,
    control_service: ControlServer<ControlService>,
    handle: DaemonHandle,
) -> Result<()> {
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;

    let port = settings.daemon.tcp_port;
    let url = format!("127.0.0.1:{port}");
    let tcp = TcpListener::bind(&url).await?;
    let tcp_stream = TcpListenerStream::new(tcp);

    tracing::info!("listening on tcp port {:?}", port);

    // Create shutdown signal from daemon handle
    let shutdown_signal = async move {
        use crate::DaemonEvent;

        let mut rx = handle.subscribe();
        loop {
            match rx.recv().await {
                Ok(DaemonEvent::ShutdownRequested) => break,
                Ok(_) => continue,
                Err(_) => break, // Channel closed
            }
        }
        eprintln!("Shutting down gRPC server...");
    };

    // Spawn the server in the background
    tokio::spawn(async move {
        if let Err(e) = Server::builder()
            .add_service(history_service)
            .add_service(search_service)
            .add_service(semantic_service)
            .add_service(control_service)
            .serve_with_incoming_shutdown(tcp_stream, shutdown_signal)
            .await
        {
            tracing::error!("gRPC server error: {e}");
        }
    });

    Ok(())
}

#[cfg(all(unix, test))]
mod unix_tests {
    use std::io::ErrorKind;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};

    use rstest::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    use super::bind_reclaiming_stale_socket;

    /// A socket path inside a scratch directory, which is removed when the test ends.
    struct Socket {
        path: PathBuf,
        _tmp: TempDir,
    }

    impl Socket {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    #[fixture]
    fn socket() -> Socket {
        let tmp = tempfile::tempdir().unwrap();
        Socket {
            path: tmp.path().join("atuin.sock"),
            _tmp: tmp,
        }
    }

    #[rstest]
    #[tokio::test]
    async fn binds_a_path_that_does_not_exist_yet(socket: Socket) {
        let path = socket.path();

        let _listener = bind_reclaiming_stale_socket(path).unwrap();

        UnixStream::connect(path).expect("the new socket should accept connections");
    }

    /// A daemon killed with `SIGKILL` runs no shutdown handler, so it leaves its socket
    /// file behind. The next start has to reclaim it, or it can never bind again.
    #[rstest]
    #[tokio::test]
    async fn reclaims_a_socket_left_behind_by_a_dead_daemon(socket: Socket) {
        let path = socket.path();

        // Closing a listener does not unlink its path, so this leaves behind exactly
        // what a process killed before its shutdown handler ran leaves behind.
        drop(UnixListener::bind(path).unwrap());
        assert!(path.exists(), "the stale socket file should still be there");
        let probe = UnixStream::connect(path).unwrap_err();
        assert_eq!(probe.kind(), ErrorKind::ConnectionRefused);

        let _listener = bind_reclaiming_stale_socket(path).unwrap();

        UnixStream::connect(path).expect("the reclaimed socket should accept connections");
    }

    /// The socket of a daemon that is alive and listening must never be unlinked: that
    /// would leave the first daemon running, but bound to a path no client reaches.
    #[rstest]
    #[tokio::test]
    async fn refuses_to_steal_a_live_daemons_socket(socket: Socket) {
        let path = socket.path();

        let _live = UnixListener::bind(path).unwrap();

        let error = bind_reclaiming_stale_socket(path).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::AddrInUse);
        assert!(path.exists(), "the live daemon's socket must not be removed");
        UnixStream::connect(path).expect("the live daemon should still be reachable");
    }
}
