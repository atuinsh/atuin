use atuin_client::settings::Settings;
#[cfg(unix)]
use eyre::Context;
use eyre::Result;

use crate::components::search::SearchGrpcService;
use crate::daemon::DaemonHandle;
use crate::grpc;
use crate::grpc::ai::session::pb::ai_session_server::AiSessionServer;
use crate::grpc::history::pb::history_server::HistoryServer;
use crate::grpc::versioning::StrictVersionLayer;
use crate::search::search_server::SearchServer;

/// How often to update the socket's modification time so it doesn't get automatically deleted by
/// temporary file cleaners.
#[cfg(unix)]
const SOCKET_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

/// The layer that ensures that all RPCs, except History/Shutdown (which we need in order to restart
/// the daemon), are the same between the client and the server version.
pub const STRICT_VERSION_LAYER: StrictVersionLayer =
    StrictVersionLayer::new(&["/history.History/Shutdown"]);

/// Where the daemon gets the socket it serves clients on.
#[derive(Debug)]
pub enum SocketSource {
    /// The configured socket, which the daemon binds and removes again on exit. On Windows, a TCP
    /// port.
    Configured,
    /// The socket systemd bound and passed in through socket activation.
    #[cfg(unix)]
    Systemd(std::os::unix::net::UnixListener),
}

impl SocketSource {
    /// The socket systemd passed this process in `LISTEN_FDS`, if it passed one, otherwise the
    /// configured one.
    ///
    /// Takes the socket out of the environment, so call it once, before anything else reads it.
    pub fn from_env() -> std::io::Result<Self> {
        #[cfg(target_os = "linux")]
        if let Some(listener) = listenfd::ListenFd::from_env().take_unix_listener(0)? {
            return Ok(Self::Systemd(listener));
        }
        Ok(Self::Configured)
    }

    /// The path of the socket the daemon listens on, if it has one.
    #[cfg(unix)]
    #[must_use]
    pub fn path(&self, daemon: &atuin_client::settings::Daemon) -> Option<std::path::PathBuf> {
        match self {
            Self::Configured => Some(daemon.socket_path().into_owned()),
            Self::Systemd(listener) => {
                listener.local_addr().ok()?.as_pathname().map(std::path::Path::to_path_buf)
            }
        }
    }
}

/// Run the gRPC server with the given services.
///
/// This starts the gRPC server in the background and returns immediately.
/// The server will shut down when a ShutdownRequested event is received.
#[cfg(unix)]
#[allow(clippy::unused_async, reason = "needs to match the cfg(not(unix)) version")]
pub async fn run_grpc_server(
    settings: &Settings,
    socket: SocketSource,
    history_service: HistoryServer<grpc::HistoryService>,
    search_service: SearchServer<SearchGrpcService>,
    ai_session_service: AiSessionServer<grpc::AiSessionService>,
    handle: DaemonHandle,
) -> Result<()> {
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;

    let (uds, cleanup_path) = match socket {
        SocketSource::Systemd(listener) => {
            listener.set_nonblocking(true)?;
            let path = listener.local_addr()?;
            tracing::info!("listening on systemd socket {path:?}");
            // Clients find a running daemon through its pidfile, but a stopped one only at a path
            // they look for sockets at.
            if !path.as_pathname().is_some_and(|path| {
                crate::client::potential_socket_paths(settings).any(|candidate| candidate == path)
            }) {
                tracing::warn!(
                    "clients won't find systemd socket {path:?} while the daemon is stopped; set \
                     `daemon.socket_path` to it"
                );
            }
            (UnixListener::from_std(listener)?, None)
        }
        SocketSource::Configured => {
            use atuin_common::path::DisplayRichExt;

            let socket_path = settings.daemon.socket_path();
            socket_path.create_default_dir_if_needed()?;
            tracing::info!("listening on unix socket {:?}", socket_path.as_path());
            (
                UnixListener::bind(&socket_path).context(format!(
                    "reading socket: {}",
                    socket_path.display_rich().relative_to_cwd()
                ))?,
                Some(socket_path.into_owned()),
            )
        }
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
                Ok(_) => {}
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
            .layer(STRICT_VERSION_LAYER)
            .add_service(history_service)
            .add_service(search_service)
            .add_service(ai_session_service)
            .serve_with_incoming_shutdown(uds_stream, shutdown_signal)
            .await
        {
            tracing::error!("gRPC server error: {e}");
        }
    });

    Ok(())
}

/// Run the gRPC server with the given services (Windows/TCP version).
#[cfg(not(unix))]
pub async fn run_grpc_server(
    settings: &Settings,
    socket: SocketSource,
    history_service: HistoryServer<grpc::HistoryService>,
    search_service: SearchServer<SearchGrpcService>,
    ai_session_service: AiSessionServer<grpc::AiSessionService>,
    handle: DaemonHandle,
) -> Result<()> {
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::Server;

    let SocketSource::Configured = socket;
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
                Ok(_) => {}
                Err(_) => break, // Channel closed
            }
        }
        eprintln!("Shutting down gRPC server...");
    };

    // Spawn the server in the background
    tokio::spawn(async move {
        if let Err(e) = Server::builder()
            .layer(STRICT_VERSION_LAYER)
            .add_service(history_service)
            .add_service(search_service)
            .add_service(ai_session_service)
            .serve_with_incoming_shutdown(tcp_stream, shutdown_signal)
            .await
        {
            tracing::error!("gRPC server error: {e}");
        }
    });

    Ok(())
}
