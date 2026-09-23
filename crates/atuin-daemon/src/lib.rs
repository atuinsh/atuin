use std::num::NonZeroUsize;
use std::sync::Arc;

use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore};
use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_client::settings::watcher::global_settings_watcher;
use atuin_common::sync::BlockingPool;
use eyre::Result;

use crate::grpc::ai_session::pb::ai_session_server::AiSessionServer;
use crate::grpc::history::pb::history_server::HistoryServer;
use crate::session_capture::AiHarnessSessionCapture;

pub mod client;
pub mod components;
pub mod daemon;
pub mod events;
pub mod grpc;
pub(crate) mod history_journal;
mod output_capture;
pub mod search;
pub mod server;
pub mod session_capture;
mod sync;

// Re-export core daemon types for convenience
// Re-export client helpers
pub use client::{AiClient, HistoryClient};
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

/// Blocking work running at once in the daemon's [`BlockingPool`]. Tokio's own blocking pool
/// allows 512 threads, past macOS's default soft limit of 256 open files.
const MAX_BLOCKING_WORKERS: NonZeroUsize = NonZeroUsize::new(32).expect("32 is non-zero");

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
    let blocking_pool = BlockingPool::new(MAX_BLOCKING_WORKERS);

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

    let host_id = Settings::host_id().await?;

    let ai_session_db_path = Settings::effective_data_dir().join("ai_harness_sessions.db");
    let ai_session_db = match AiSessionDatabase::open(&ai_session_db_path).await {
        Ok(db) => Some(db),
        Err(err) => {
            tracing::error!(
                ?err,
                path = ?ai_session_db_path,
                "failed to open the ai-session sidecar; ai-session capture is disabled"
            );
            None
        }
    };
    let ai_session_capture = Arc::new(match &ai_session_db {
        Some(db) => {
            let records = AiSessionStore::builder()
                .store(handle.store().clone())
                .host_id(host_id)
                .key(handle.encryption_key().clone())
                .build();

            // Reproject the sidecar from the synced record store before capture starts. The record
            // store is the source of truth; a sidecar that missed an append (transient error,
            // crash between the two writes, or a lost db file) is repaired here instead of being
            // stranded until — or re-pushed as duplicate records by — file re-capture. append's
            // ON CONFLICT keying makes the replay idempotent.
            let recovered = match records.build(db).await {
                Ok(()) => true,
                Err(err) => {
                    tracing::error!(
                        ?err,
                        "failed to reproject ai-session sidecar; capture and import disabled \
                         until restart"
                    );
                    false
                }
            };

            AiHarnessSessionCapture::open(
                records,
                db.clone(),
                settings.ai.capture_sessions,
                recovered,
                blocking_pool.clone(),
            )
        }
        None => AiHarnessSessionCapture::nop().await,
    });

    let _sync_engine = sync::SyncEngine::spawn(handle.clone(), search_index.clone(), ai_session_db);

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
    let ai_session_service = AiSessionServer::new(grpc::AiSessionService::new(ai_session_capture));

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
        search_service.build(handle.clone()),
        ai_session_service,
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
    tokio::signal::ctrl_c().await.expect("failed to listen for ctrl+c");
}
