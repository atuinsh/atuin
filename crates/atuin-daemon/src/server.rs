use eyre::Result;

use crate::components::search::SearchGrpcService;
use crate::daemon::DaemonHandle;
use crate::grpc;
use crate::grpc::ai::session::pb::ai_session_server::AiSessionServer;
use crate::grpc::history::pb::history_server::HistoryServer;
use crate::grpc::request::RequestSpanLayer;
use crate::grpc::userio::pb::user_io_server::UserIoServer;
use crate::grpc::versioning::StrictVersionLayer;
use crate::search::search_server::SearchServer;
use crate::service::address::DaemonListener;

/// The layer that ensures that all RPCs, except History/Shutdown (which we need in order to restart
/// the daemon), are the same between the client and the server version.
pub const STRICT_VERSION_LAYER: StrictVersionLayer =
    StrictVersionLayer::new(&["/history.History/Shutdown"]);

/// Run the gRPC server with the given services.
///
/// This starts the gRPC server in the background and returns immediately.
/// The server will shut down when a ShutdownRequested event is received.
pub fn run_grpc_server(
    listener: DaemonListener,
    history_service: HistoryServer<grpc::HistoryService>,
    search_service: SearchServer<SearchGrpcService>,
    ai_session_service: AiSessionServer<grpc::AiSessionService>,
    userio_service: UserIoServer<grpc::UserIoService>,
    handle: DaemonHandle,
) -> Result<()> {
    use tonic::transport::Server;

    let incoming = listener.into_incoming()?;

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
            .layer(RequestSpanLayer)
            .add_service(history_service)
            .add_service(search_service)
            .add_service(ai_session_service)
            .add_service(userio_service)
            .serve_with_incoming_shutdown(incoming, shutdown_signal)
            .await
        {
            tracing::error!("gRPC server error: {e}");
        }
    });

    Ok(())
}
