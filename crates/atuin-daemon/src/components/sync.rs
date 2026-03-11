//! Sync component.
//!
//! Handles periodic synchronization with the Atuin cloud server.

use eyre::Result;
use rand::Rng;
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};

use atuin_client::{history::store::HistoryStore, record::sync, settings::Settings};
use atuin_dotfiles::store::{AliasStore, var::VarStore};

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
};

/// Commands that can be sent to the sync task.
enum SyncCommand {
    /// Trigger an immediate sync.
    ForceSync,
    /// Stop the sync loop.
    Stop,
}

/// Sync component - handles periodic cloud synchronization.
///
/// This component:
/// - Runs a background sync loop on a configurable interval
/// - Implements exponential backoff on sync failures
/// - Responds to ForceSync events for immediate sync
/// - Emits SyncCompleted/SyncFailed events
pub struct SyncComponent {
    task_handle: Option<tokio::task::JoinHandle<()>>,
    command_tx: Option<mpsc::Sender<SyncCommand>>,
}

impl SyncComponent {
    /// Create a new sync component.
    pub fn new() -> Self {
        Self {
            task_handle: None,
            command_tx: None,
        }
    }
}

impl Default for SyncComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl Component for SyncComponent {
    fn name(&self) -> &'static str {
        "sync"
    }

    async fn start(&mut self, handle: DaemonHandle) -> Result<()> {
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        self.command_tx = Some(cmd_tx);

        // Spawn the sync loop with its own copy of the handle
        self.task_handle = Some(tokio::spawn(sync_loop(handle, cmd_rx)));

        tracing::info!("sync component started");
        Ok(())
    }

    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
        if let DaemonEvent::ForceSync = event {
            tracing::info!("force sync requested");
            if let Some(tx) = &self.command_tx {
                let _ = tx.send(SyncCommand::ForceSync).await;
            }
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(SyncCommand::Stop).await;
        }
        if let Some(handle) = self.task_handle.take() {
            // Give the task a moment to shut down gracefully
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
        tracing::info!("sync component stopped");
        Ok(())
    }
}

/// The main sync loop.
///
/// This runs in a spawned task and handles periodic sync as well as
/// force sync requests.
async fn sync_loop(handle: DaemonHandle, mut cmd_rx: mpsc::Receiver<SyncCommand>) {
    tracing::info!("sync loop starting");

    // Clone settings since we need them across await points
    let settings = handle.settings().await.clone();
    let host_id = match Settings::host_id().await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("failed to get host id, sync disabled: {e}");
            return;
        }
    };

    // Create the stores we need
    let encryption_key = *handle.encryption_key();
    let history_store = HistoryStore::new(handle.store().clone(), host_id, encryption_key);
    let alias_store = AliasStore::new(handle.store().clone(), host_id, encryption_key);
    let var_store = VarStore::new(handle.store().clone(), host_id, encryption_key);

    // Don't backoff by more than 30 mins (with a random jitter of up to 1 min)
    let max_interval: f64 = 60.0 * 30.0 + rand::thread_rng().gen_range(0.0..60.0);

    let mut ticker = time::interval(time::Duration::from_secs(settings.daemon.sync_frequency));

    // IMPORTANT: without this, if we miss ticks because a sync takes ages or is otherwise delayed,
    // we may end up running a lot of syncs in a hot loop.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                do_sync_tick(
                    &handle,
                    &history_store,
                    &alias_store,
                    &var_store,
                    &mut ticker,
                    max_interval,
                ).await;
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SyncCommand::ForceSync) => {
                        tracing::info!("executing force sync");
                        do_sync_tick(
                            &handle,
                            &history_store,
                            &alias_store,
                            &var_store,
                            &mut ticker,
                            max_interval,
                        ).await;
                    }
                    Some(SyncCommand::Stop) | None => {
                        tracing::info!("sync loop stopping");
                        break;
                    }
                }
            }
        }
    }
}

/// Execute a single sync tick.
async fn do_sync_tick(
    handle: &DaemonHandle,
    history_store: &HistoryStore,
    alias_store: &AliasStore,
    var_store: &VarStore,
    ticker: &mut time::Interval,
    max_interval: f64,
) {
    // Clone settings since we need them across await points
    let settings = handle.settings().await.clone();

    tracing::info!("sync tick");

    // Check if logged in
    let logged_in = match settings.logged_in().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("failed to check login status, skipping sync tick: {e}");
            return;
        }
    };

    if !logged_in {
        tracing::debug!("not logged in, skipping sync tick");
        return;
    }

    // Perform the sync
    let res = sync::sync(&settings, handle.store()).await;

    match res {
        Err(e) => {
            tracing::error!("sync tick failed with {e}");

            // Emit failure event
            handle.emit(DaemonEvent::SyncFailed {
                error: e.to_string(),
            });

            // Exponential backoff
            let mut rng = rand::thread_rng();
            let mut new_interval = ticker.period().as_secs_f64() * rng.gen_range(2.0..2.2);

            if new_interval > max_interval {
                new_interval = max_interval;
            }

            *ticker = time::interval(time::Duration::from_secs(new_interval as u64));
            ticker.reset_after(time::Duration::from_secs(new_interval as u64));

            tracing::error!("backing off, next sync tick in {new_interval}");
        }
        Ok((uploaded_count, downloaded_records)) => {
            tracing::info!(
                uploaded = uploaded_count,
                downloaded = downloaded_records.len(),
                "sync complete"
            );

            // Build history from downloaded records
            if let Err(e) = history_store
                .incremental_build(handle.history_db(), &downloaded_records)
                .await
            {
                tracing::error!("failed to build history from downloaded records: {e}");
            }

            // Emit the records added event (for search indexing)
            handle.emit(DaemonEvent::RecordsAdded(downloaded_records.clone()));

            // Emit sync completed event
            handle.emit(DaemonEvent::SyncCompleted {
                uploaded: uploaded_count as usize,
                downloaded: downloaded_records.len(),
            });

            // Rebuild alias and var stores
            if let Err(e) = alias_store.build().await {
                tracing::error!("failed to rebuild alias store: {e}");
            }
            if let Err(e) = var_store.build().await {
                tracing::error!("failed to rebuild var store: {e}");
            }

            // Reset backoff on success
            if ticker.period().as_secs() != settings.daemon.sync_frequency {
                *ticker = time::interval(time::Duration::from_secs(settings.daemon.sync_frequency));
            }

            // Store sync time
            if let Err(e) = Settings::save_sync_time().await {
                tracing::error!("failed to save sync time: {e}");
            }
        }
    }
}
