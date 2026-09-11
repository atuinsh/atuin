//! Background worker which periodically syncs data with the main server.

use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::store::HistoryStore;
use atuin_client::record::sync::{ClientSource, SyncEngine, SyncError as ClientSyncError};
use atuin_client::settings::Settings;
use atuin_common::futures::Backoff;
use atuin_dotfiles::store::AliasStore;
use atuin_dotfiles::store::var::VarStore;
use futures::StreamExt;
use tokio::sync::RwLock;
use tokio::time;

use crate::daemon::DaemonHandle;
use crate::search::SearchIndex;

/// Cap on the exponential backoff between failed sync attempts, and the budget for a single retry
/// episode: once a failing sync has been retried for this long we fall back to the normal periodic
/// cadence and start a fresh ramp on the next tick.
const MAX_BACKOFF: Duration = Duration::from_mins(30);

/// Factor by which the sync retry backoff grows after each failure.
const BACKOFF_FACTOR: NonZeroU32 = NonZeroU32::new(2).unwrap();

/// Owns everything the sync loop needs across ticks.
struct Worker {
    handle: DaemonHandle,
    index: Arc<RwLock<SearchIndex>>,
    /// TODO(markovejnovic): Would be good to have a StoreCtx which is a bundle of all these stores.
    history_store: HistoryStore,
    alias_store: AliasStore,
    var_store: VarStore,
}

/// Errors that prevent the sync worker from starting.
#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error("failed to get host id: {0}")]
    HostId(eyre::Report),
}

/// Why a single [`Worker::sync_once`] attempt did not complete a sync.
#[derive(Debug, thiserror::Error)]
enum SyncTickError {
    #[error("failed to check login status: {0}")]
    LoginCheck(eyre::Report),
    #[error("not logged in")]
    NotLoggedIn,
    #[error("sync failed: {0}")]
    Sync(#[from] ClientSyncError),
}

impl SyncTickError {
    /// Emit the failure at a level matching its severity.
    fn log(&self) {
        match self {
            Self::Sync(_) => tracing::error!("{self}"),
            Self::LoginCheck(_) => tracing::warn!("{self}"),
            Self::NotLoggedIn => tracing::debug!("{self}"),
        }
    }
}

impl From<SyncTickError> for ControlFlow<()> {
    /// A failed sync is transient - retry it with backoff. Anything else means there is nothing
    /// to sync right now, so stop retrying and resume the normal cadence.
    fn from(err: SyncTickError) -> Self {
        match err {
            SyncTickError::Sync(_) => Self::Continue(()),
            SyncTickError::LoginCheck(_) | SyncTickError::NotLoggedIn => Self::Break(()),
        }
    }
}

impl Worker {
    async fn new(
        handle: DaemonHandle,
        index: Arc<RwLock<SearchIndex>>,
    ) -> Result<Self, StartError> {
        let host_id = Settings::host_id().await.map_err(StartError::HostId)?;

        let encryption_key = handle.encryption_key();
        // TODO(markovejnovic): We should definitely not be creating new stores, but this is pending
        //                      us having the concept of a "store bundle".
        let history_store =
            HistoryStore::new(handle.store().clone(), host_id, encryption_key.clone());
        let alias_store = AliasStore::new(handle.store().clone(), host_id, encryption_key.clone());
        let var_store = VarStore::new(handle.store().clone(), host_id, encryption_key.clone());

        Ok(Self {
            handle,
            index,
            history_store,
            alias_store,
            var_store,
        })
    }

    /// Runs the sync engine in the background.
    ///
    /// This blocks the active task forever.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn run(self) {
        loop {
            let settings = self.handle.settings().await.clone();
            let interval = Duration::from_secs(settings.daemon.sync_frequency);

            if settings.auto_sync {
                let backoff = Backoff::Exponential {
                    initial: interval,
                    max: MAX_BACKOFF,
                    factor: BACKOFF_FACTOR,
                };
                let _ = backoff.retry(|| self.sync_tick(&settings), MAX_BACKOFF).await;
            } else {
                tracing::debug!("auto_sync disabled, skipping periodic sync tick");
            }

            time::sleep(interval).await;
        }
    }

    /// Run one [`Self::sync_once`], log any failure, and translate it into a retry decision.
    async fn sync_tick(&self, settings: &Settings) -> ControlFlow<()> {
        match self.sync_once(settings).await {
            Ok(()) => ControlFlow::Break(()),
            Err(err) => {
                err.log();
                err.into()
            }
        }
    }

    /// Attempt a single sync.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn sync_once(&self, settings: &Settings) -> Result<(), SyncTickError> {
        let logged_in = settings.logged_in().await.map_err(SyncTickError::LoginCheck)?;

        if !logged_in {
            return Err(SyncTickError::NotLoggedIn);
        }

        // Perform the sync
        let engine = SyncEngine::builder()
            .store(self.handle.store().clone())
            .client_source(ClientSource::FromSettings {
                settings,
                caps: Some(self.handle.caps().clone()),
            })
            .build()
            .connect()
            .await?;
        let (uploaded_count, downloaded_records) =
            engine.keyed(self.handle.encryption_key()).sync().await?;

        tracing::info!(
            uploaded = uploaded_count,
            downloaded = downloaded_records.len(),
            "sync complete"
        );

        let history_build = async {
            let batches =
                self.history_store.incremental_build(self.handle.history_db(), &downloaded_records);
            futures::pin_mut!(batches);

            while let Some(batch) = batches.next().await {
                match batch {
                    // The rows are already in sqlite; push them straight into the live index.
                    Ok(histories) if !histories.is_empty() => {
                        self.index.read().await.add_histories(&histories);
                    }
                    Ok(_) => {}
                    // Legacy behavior was to abort on the first error.
                    Err(e) => {
                        tracing::error!("failed to build history from downloaded records: {e}");
                        break;
                    }
                }
            }
        };

        let alias_build = async {
            if let Err(e) = self.alias_store.build().await {
                tracing::error!("failed to rebuild alias store: {e}");
            }
        };

        let var_build = async {
            if let Err(e) = self.var_store.build().await {
                tracing::error!("failed to rebuild var store: {e}");
            }
        };

        tokio::join!(history_build, alias_build, var_build);

        // Store sync time
        if let Err(e) = Settings::save_sync_time().await {
            tracing::error!("failed to save sync time: {e}");
        }

        Ok(())
    }
}

/// Entry point for the spawned sync task.
pub(super) async fn run(handle: DaemonHandle, index: Arc<RwLock<SearchIndex>>) {
    match Worker::new(handle, index).await {
        Ok(worker) => worker.run().await,
        Err(e) => tracing::error!("sync disabled: {e}"),
    }
}
