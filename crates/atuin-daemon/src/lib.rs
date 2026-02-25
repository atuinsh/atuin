use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::{record::sqlite_store::SqliteStore, settings::Settings};
use eyre::Result;

pub mod client;
pub mod components;
pub mod control;
pub mod daemon;
pub mod events;
pub mod history;
pub mod search;
pub mod server;

// Re-export core daemon types for convenience
pub use daemon::{Component, Daemon, DaemonBuilder, DaemonHandle};
pub use events::DaemonEvent;

// Re-export components
pub use components::{HistoryComponent, SearchComponent, SyncComponent};

// Re-export client helpers
pub use client::{ControlClient, emit_event, emit_event_with_settings};

/// Boot the daemon using the new component-based architecture.
///
/// This creates a daemon with the standard components (history, search, sync),
/// starts the gRPC server with their services, and runs the event loop.
pub async fn boot(
    settings: Settings,
    store: SqliteStore,
    history_db: HistoryDatabase,
) -> Result<()> {
    // Create the components
    let history_component = HistoryComponent::new();
    let search_component = SearchComponent::new();
    let sync_component = SyncComponent::new();

    // Get the gRPC services before moving components into the daemon
    // (The services share state with the components via Arc)
    let history_service = history_component.grpc_service();
    let search_service = search_component.grpc_service();

    // Build the daemon
    let mut daemon = Daemon::builder(settings.clone())
        .store(store)
        .history_db(history_db)
        .component(history_component)
        .component(search_component)
        .component(sync_component)
        .build()
        .await?;

    // Get a handle for the control service and gRPC server shutdown
    let handle = daemon.handle();

    // Create the control service
    let control_service = control::ControlService::new(handle.clone());

    // Start all components first (so gRPC services can work)
    daemon.start_components().await?;

    // Spawn signal handler to emit ShutdownRequested on Ctrl+C/SIGTERM
    let signal_handle = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("received shutdown signal");
        signal_handle.shutdown();
    });

    // Start the gRPC server in the background
    server::run_grpc_server(
        settings,
        history_service,
        search_service,
        control_service.into_server(),
        handle,
    )
    .await?;

    // Run the daemon event loop
    daemon.run_event_loop().await?;

    // Stop all components on shutdown
    daemon.stop_components().await;

    tracing::info!("daemon shut down complete");
    Ok(())
}

/// Wait for a shutdown signal (Ctrl+C or SIGTERM).
#[cfg(unix)]
async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register sigterm handler");
    let mut int = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("failed to register sigint handler");

    tokio::select! {
        _ = term.recv() => {},
        _ = int.recv() => {},
    }
}

/// Wait for a shutdown signal (Ctrl+C).
#[cfg(not(unix))]
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
}
