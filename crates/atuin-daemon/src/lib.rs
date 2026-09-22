use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_client::settings::watcher::global_settings_watcher;
use eyre::{Result, WrapErr};

use crate::grpc::history::pb::history_server::HistoryServer;

pub mod client;
pub mod components;
pub mod daemon;
pub mod events;
pub mod grpc;
pub(crate) mod history_journal;
mod output_capture;
pub mod search;
pub mod server;
mod shutdown;
mod sync;

// Re-export core daemon types for convenience
// Re-export client helpers
pub use client::HistoryClient;
// Re-export components
pub use components::SearchComponent;
pub use daemon::{AnyComponent, Daemon, DaemonBuilder, DaemonHandle};
pub use events::DaemonEvent;
pub use history_journal::{
    CmdCancelError, CmdDeleteError, CmdEvent, CmdFinishError, CmdRebuildError, FinishedCmd,
    GetCmdInFlightError, HistoryJournal, RegisterOutputError,
};
pub use output_capture::{
    CaptureError, DeleteOutputError, GetOutputError, OutputCaptureEngine, OutputLine, OutputMatch,
};

/// How long shutdown waits for the gRPC server to drain. Long-lived streams never do.
const SERVER_DRAIN: Duration = Duration::from_secs(1);

/// Boot the daemon using the new component-based architecture.
///
/// This creates a daemon with the search component, spawns the background sync
/// engine, starts the gRPC server with their services, and runs the event loop.
pub async fn boot(
    settings: Settings,
    store: SqliteStore,
    history_db: HistoryDatabase,
) -> Result<()> {
    // Create the components
    let search_component = SearchComponent::new();

    let output_capture = match settings.output.limits() {
        Some(limits) => {
            OutputCaptureEngine::open(Settings::command_capture_dir(), limits.max_disk_usage).await
        }
        None => OutputCaptureEngine::nop(),
    };

    // Get the gRPC services before moving components into the daemon
    // (The services share state with the components via Arc)
    let search_service = search_component.grpc_service(output_capture.store());
    let search_index = search_component.index();

    // Build the daemon
    let mut daemon = Daemon::builder(settings.clone())
        .store(store)
        .history_db(history_db)
        .component(search_component)
        .build()?;

    let handle = daemon.handle();

    let _sync_engine = sync::SyncEngine::spawn(handle.clone(), search_index.clone());

    let host_id = Settings::host_id().await?;
    let history_store =
        HistoryStore::new(handle.store().clone(), host_id, handle.encryption_key().clone());
    let journal = Arc::new(HistoryJournal::new(
        handle.caps().clone(),
        history_store,
        handle.history_db().clone(),
        search_index,
        output_capture,
    ));
    let history_service = HistoryServer::new(grpc::HistoryService::new(journal, handle.clone()));

    // Start all components first (so gRPC services can work)
    daemon.start_components().await?;

    // Spawn config file watcher to reload settings on changes
    if let Ok(watcher) = global_settings_watcher() {
        let mut settings_rx = watcher.subscribe();
        let watcher_handle = handle.clone();
        tokio::spawn(async move {
            tracing::info!("config file watcher started");
            while settings_rx.changed().await.is_ok() {
                // Use the already-loaded settings from the watcher
                // (avoids parsing the config file twice)
                let new_settings = (*settings_rx.borrow()).clone();
                watcher_handle.apply_settings((*new_settings).clone()).await;
            }
            tracing::debug!("config file watcher stopped");
        });
    } else {
        tracing::warn!(
            "failed to start config file watcher; settings changes will require daemon restart"
        );
    }

    // Shut down gracefully on SIGTERM/SIGINT, and forcibly if that stalls
    let signal_handle = handle.clone();
    shutdown::install(shutdown::GRACE, move || signal_handle.shutdown())
        .wrap_err("failed to install signal handlers")?;

    // Start the gRPC server in the background
    let server = server::run_grpc_server(
        settings,
        history_service,
        search_service.build(handle.clone()),
        handle,
    )
    .await?;

    // Run the daemon event loop
    daemon.run_event_loop().await?;

    // Stop all components on shutdown
    daemon.stop_components().await;

    if tokio::time::timeout(SERVER_DRAIN, server).await.is_err() {
        tracing::warn!("gRPC server did not drain within {SERVER_DRAIN:?}");
    }

    tracing::info!("daemon shut down complete");
    Ok(())
}
