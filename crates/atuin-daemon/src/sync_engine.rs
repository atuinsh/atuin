//! Background cloud sync.
//!
//! [`SyncEngine`] owns a single background task that periodically syncs the record store with the
//! configured server, rebuilds the local history / alias / var stores from whatever it downloaded,
//! and announces the result on the daemon event bus. Failed syncs are retried with exponential
//! backoff. The work itself is done by [`SyncEngineWorker`], which lives inside that task.
//!
//! It is deliberately *not* a [`crate::daemon::Component`]: it reacts to no bus event other than
//! [`DaemonEvent::ShutdownRequested`] and exposes no gRPC service, so the component lifecycle only
//! added boilerplate (a command channel whose sole command was "stop").

use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sync::{ClientSource, SyncEngine as ClientSyncEngine, SyncError};
use atuin_client::settings::Settings;
use atuin_common::futures::Backoff;
use atuin_domain::record::RecordId;
use atuin_dotfiles::store::AliasStore;
use atuin_dotfiles::store::var::VarStore;
use easy_cast::Conv;
use futures::StreamExt;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::{self, Interval, MissedTickBehavior};

use crate::daemon::DaemonHandle;
use crate::events::DaemonEvent;

/// A running sync engine.
///
/// This is a handle to the background task; the actual work happens in a [`SyncEngineWorker`]
/// inside it. Dropping the engine aborts the task, so hold onto it for as long as sync should run.
#[must_use = "dropping the engine aborts the sync task"]
pub struct SyncEngine {
    task: JoinHandle<()>,
}

impl SyncEngine {
    /// How long [`Self::shutdown`] waits for an in-flight tick before giving up on it.
    const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

    /// Spawn the sync engine.
    ///
    /// The bus subscription is taken *before* the task is spawned so a
    /// [`DaemonEvent::ShutdownRequested`] emitted immediately after this returns is never missed.
    pub fn spawn(handle: DaemonHandle) -> Self {
        let events = handle.subscribe();
        let task = tokio::spawn(async move {
            tracing::info!("sync engine starting");
            match SyncEngineWorker::new(handle, events).await {
                Ok(worker) => worker.run().await,
                Err(e) => {
                    tracing::error!("sync engine disabled for this daemon run: {e}");
                }
            }
            tracing::info!("sync engine stopped");
        });
        Self { task }
    }

    /// Wait for the worker to finish after a [`DaemonEvent::ShutdownRequested`] was emitted.
    ///
    /// The worker stops on its own once it sees that event; this gives an in-flight tick up to
    /// [`Self::SHUTDOWN_GRACE`] to complete. Whatever is still running afterwards (a sync, or a
    /// backoff sleep between retries) is aborted when `self` drops.
    pub async fn shutdown(mut self) {
        if time::timeout(Self::SHUTDOWN_GRACE, &mut self.task).await.is_err() {
            tracing::warn!("sync engine did not stop within {:?}", Self::SHUTDOWN_GRACE);
        }
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The state behind a [`SyncEngine`]: everything one sync tick needs, owned by the background task.
struct SyncEngineWorker {
    handle: DaemonHandle,
    events: broadcast::Receiver<DaemonEvent>,
    history_store: HistoryStore,
    alias_store: AliasStore,
    var_store: VarStore,
    ticker: Interval,
    /// The configured sync period the ticker was built with, to detect a settings change.
    period: Duration,
}

impl SyncEngineWorker {
    /// Never wait longer than this between retries of a failed sync.
    const MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);

    async fn new(
        handle: DaemonHandle,
        events: broadcast::Receiver<DaemonEvent>,
    ) -> eyre::Result<Self> {
        let host_id = Settings::host_id().await?;
        let key = handle.encryption_key();
        let history_store = HistoryStore::new(handle.store().clone(), host_id, key.clone());
        let alias_store = AliasStore::new(handle.store().clone(), host_id, key.clone());
        let var_store = VarStore::new(handle.store().clone(), host_id, key.clone());

        let period = sync_period(&*handle.settings().await);

        Ok(Self {
            handle,
            events,
            history_store,
            alias_store,
            var_store,
            ticker: new_ticker(period),
            period,
        })
    }

    /// The sync loop. Runs until shutdown is requested or the event bus closes.
    async fn run(mut self) {
        loop {
            tokio::select! {
                _ = self.ticker.tick() => {}
                () = shutdown_requested(&mut self.events) => break,
            }
            self.tick().await;
        }
    }

    /// One tick of the loop: sync, retrying with backoff until it succeeds, then re-arm the ticker.
    async fn tick(&mut self) {
        if !self.handle.settings().await.auto_sync {
            tracing::debug!("auto_sync disabled, skipping periodic sync tick");
            return;
        }

        tracing::info!("sync tick");

        // Once a sync has failed, keep retrying regardless of `auto_sync` (the old behaviour).
        let backoff = Backoff::Exponential {
            initial: self.period.saturating_mul(2),
            max: Self::MAX_BACKOFF,
            factor: NonZeroU32::new(2).unwrap(),
        };
        // `Duration::MAX` never elapses; a sync that can't reach the server retries until shutdown.
        if let Err(e) = backoff.retry(|| self.attempt(), Duration::MAX).await {
            tracing::error!("gave up retrying sync: {e}");
        }

        // Pick up a changed `daemon.sync_frequency`, and in any case make the next tick a full
        // period from *now* rather than from the tick that just completed.
        let period = sync_period(&*self.handle.settings().await);
        if period != self.period {
            self.period = period;
            self.ticker = new_ticker(period);
        }
        self.ticker.reset();
    }

    /// One sync attempt, in [`Backoff::retry`]'s terms: `Break` when there is nothing left to
    /// retry, `Continue` when the sync failed.
    async fn attempt(&self) -> ControlFlow<(), SyncError> {
        // Clone rather than hold the read guard across the network round-trip, so settings reloads
        // aren't blocked for the duration of a sync. Re-read per attempt so a retry sequence picks
        // up a changed server address or token.
        let settings = self.handle.settings().await.clone();
        match self.sync_once(&settings).await {
            Ok(()) => ControlFlow::Break(()),
            Err(e) => {
                tracing::error!("sync tick failed with {e}, backing off");
                self.handle.emit(DaemonEvent::SyncFailed {
                    error: e.to_string(),
                });
                ControlFlow::Continue(e)
            }
        }
    }

    /// One sync.
    ///
    /// `Ok(())` means either the sync succeeded or there was nothing to do (not logged in, or the
    /// login check itself failed). `Err` means the sync itself failed and should be retried.
    async fn sync_once(&self, settings: &Settings) -> Result<(), SyncError> {
        let logged_in = match settings.logged_in().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("failed to check login status, skipping sync tick: {e}");
                return Ok(());
            }
        };

        if let Err(e) = self.handle.caps().refresh().await {
            tracing::debug!("capability refresh failed, keeping cached document: {e}");
        }

        if !logged_in {
            tracing::debug!("not logged in, skipping sync tick");
            return Ok(());
        }

        let engine = ClientSyncEngine::builder()
            .store(self.handle.store().clone())
            .client_source(ClientSource::FromSettings {
                settings,
                caps: Some(self.handle.caps().clone()),
            })
            .build()
            .connect()
            .await?;
        let (uploaded, downloaded) = engine.keyed(self.handle.encryption_key()).sync().await?;

        tracing::info!(uploaded, downloaded = downloaded.len(), "sync complete");

        self.rebuild_history(&downloaded).await;

        self.handle.emit(DaemonEvent::SyncCompleted {
            uploaded: usize::conv(uploaded),
            downloaded: downloaded.len(),
        });

        self.rebuild_dotfiles().await;

        if let Err(e) = Settings::save_sync_time().await {
            tracing::error!("failed to save sync time: {e}");
        }

        Ok(())
    }

    /// Fold freshly downloaded records into the history database, announcing new history on the bus.
    async fn rebuild_history(&self, downloaded: &[RecordId]) {
        // `incremental_build` already yields in bounded batches - an initial sync (on backfill,
        // eg.) risks being dozens of GB of RAM otherwise.
        let mut batches = std::pin::pin!(
            self.history_store.incremental_build(self.handle.history_db(), downloaded)
        );
        while let Some(batch) = batches.next().await {
            match batch {
                Ok(histories) if !histories.is_empty() => {
                    // Only the IDs go on the bus; the rows themselves are already in sqlite.
                    let ids: Arc<[HistoryId]> = histories.iter().map(|h| h.id).collect();
                    self.handle.emit(DaemonEvent::HistorySynced(ids));
                }
                Ok(_) => {}
                // Legacy behavior was to abort on the first error.
                Err(e) => {
                    tracing::error!("failed to build history from downloaded records: {e}");
                    break;
                }
            }
        }
    }

    /// Rebuild the alias and var stores from the record store.
    async fn rebuild_dotfiles(&self) {
        if let Err(e) = self.alias_store.build().await {
            tracing::error!("failed to rebuild alias store: {e}");
        }
        if let Err(e) = self.var_store.build().await {
            tracing::error!("failed to rebuild var store: {e}");
        }
    }
}

/// Resolve once the daemon asks for shutdown (or the bus is gone). Every other event is ignored.
///
/// `broadcast::Receiver::recv` is cancel-safe, so this is safe to race in a `select!`.
async fn shutdown_requested(events: &mut broadcast::Receiver<DaemonEvent>) {
    loop {
        match events.recv().await {
            Ok(DaemonEvent::ShutdownRequested) | Err(RecvError::Closed) => return,
            // This is only polled between ticks (raced in `select!` against `ticker.tick()`), so
            // if more than the bus capacity worth of events arrive during one long-running sync, a
            // `ShutdownRequested` can be dropped along with the rest of the lagged batch. The only
            // event this engine emits at any real rate is its own `HistorySynced`, and
            // `SyncEngine::shutdown` bounds the wait on the task, so a lost event here just means
            // shutdown takes up to that long rather than hanging.
            Ok(_) | Err(RecvError::Lagged(_)) => {}
        }
    }
}

fn sync_period(settings: &Settings) -> Duration {
    // `tokio::time::interval` panics if given a zero period, and `sync_frequency` isn't validated
    // elsewhere, so clamp to at least one second.
    Duration::from_secs(settings.daemon.sync_frequency.max(1))
}

/// A ticker whose first tick fires immediately, and which never tries to "catch up" on ticks
/// missed while a slow sync was running.
fn new_ticker(period: Duration) -> Interval {
    let mut ticker = time::interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rstest::rstest;
    use tokio::sync::broadcast;

    use super::shutdown_requested;
    use crate::events::DaemonEvent;

    #[rstest]
    #[tokio::test]
    async fn shutdown_requested_returns_on_shutdown_event() {
        let (tx, mut rx) = broadcast::channel(4);
        tx.send(DaemonEvent::ShutdownRequested).unwrap();
        tokio::time::timeout(Duration::from_secs(1), shutdown_requested(&mut rx))
            .await
            .expect("should return once ShutdownRequested is received");
    }

    #[rstest]
    #[tokio::test]
    async fn shutdown_requested_returns_when_bus_closes() {
        let (tx, mut rx) = broadcast::channel::<DaemonEvent>(4);
        drop(tx);
        tokio::time::timeout(Duration::from_secs(1), shutdown_requested(&mut rx))
            .await
            .expect("should return once the sender is gone");
    }

    #[rstest]
    #[tokio::test]
    async fn shutdown_requested_ignores_other_events() {
        let (tx, mut rx) = broadcast::channel(4);
        tx.send(DaemonEvent::SettingsReloaded).unwrap();
        tx.send(DaemonEvent::SyncCompleted {
            uploaded: 0,
            downloaded: 0,
        })
        .unwrap();
        let waited =
            tokio::time::timeout(Duration::from_millis(50), shutdown_requested(&mut rx)).await;
        assert!(waited.is_err(), "must keep waiting through non-shutdown events");
    }
}
