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

/// Where the daemon gets the socket it listens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketSource {
    /// Where the settings say: a socket the daemon binds itself, or the one systemd passes in with
    /// `daemon.systemd_socket`. On Windows, a TCP port.
    Configured,
    /// The socket launchd holds at `daemon.socket_path` for its agent.
    #[cfg(target_os = "macos")]
    Launchd,
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

    let socket_path = settings.daemon.socket_path();

    // The socket's file, kept fresh against temporary file cleaners, and whether to remove it on
    // exit.
    let (uds, socket_file, remove_on_exit) = match socket {
        #[cfg(target_os = "macos")]
        SocketSource::Launchd => {
            // launchd can't place the socket until its directory exists, and starts the agent
            // again until it has.
            socket_path.create_default_dir_if_needed()?;
            tracing::info!("listening on launchd socket {:?}", socket_path.as_path());
            let listener = crate::agent::Launchd::activate_socket()
                .context("checking in with launchd for the daemon's socket")?;
            listener.set_nonblocking(true)?;
            // launchd goes on listening once this agent exits, to start the next one.
            (UnixListener::from_std(listener)?, Some(socket_path.into_owned()), false)
        }
        SocketSource::Configured if cfg!(target_os = "linux") && settings.daemon.systemd_socket => {
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::net::SocketAddr;
                use std::path::PathBuf;

                use eyre::{OptionExt, WrapErr};
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
                            "could not detect systemd socket path, ensure that it's at the \
                             configured path: {:?}, error: {err:?}",
                            socket_path.as_path(),
                        );
                    }
                }
                (UnixListener::from_std(listener)?, None, false)
            }
            #[cfg(not(target_os = "linux"))]
            unreachable!()
        }
        SocketSource::Configured => {
            use atuin_common::path::DisplayRichExt;

            socket_path.create_default_dir_if_needed()?;
            tracing::info!("listening on unix socket {:?}", socket_path.as_path());
            (
                UnixListener::bind(&socket_path).context(format!(
                    "reading socket: {}",
                    socket_path.display_rich().relative_to_cwd()
                ))?,
                Some(socket_path.into_owned()),
                true,
            )
        }
    };

    let uds_stream = UnixListenerStream::new(uds);

    // Periodically update the socket's modification time so it doesn't get automatically deleted by
    // temporary file cleaners.
    let socket_updater = socket_file.clone().map(|path| {
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
        if remove_on_exit && let Some(path) = socket_file {
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
