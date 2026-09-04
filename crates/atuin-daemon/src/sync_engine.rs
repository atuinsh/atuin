//! Background cloud sync.
//!
//! The sync engine is a single spawned task that periodically syncs the record store with the
//! configured server, rebuilds the local history / alias / var stores from whatever it downloaded,
//! and announces the result on the daemon event bus. Failed syncs are retried with exponential
//! backoff.
//!
//! It is deliberately *not* a [`crate::daemon::Component`]: it reacts to no bus event other than
//! [`DaemonEvent::ShutdownRequested`] and exposes no gRPC service, so the component lifecycle only
//! added boilerplate (a command channel whose sole command was "stop").

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sync::{ClientSource, SyncEngine, SyncError};
use atuin_client::settings::Settings;
use atuin_domain::record::{HostId, RecordId};
use atuin_dotfiles::store::AliasStore;
use atuin_dotfiles::store::var::VarStore;
use easy_cast::Conv;
use futures::StreamExt;
use rand::Rng;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio::time::{self, Interval, MissedTickBehavior};

use crate::daemon::DaemonHandle;
use crate::events::DaemonEvent;

/// How long [`spawn`]'s caller should be willing to wait for an in-flight tick on shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Spawn the sync engine.
///
/// The bus subscription is taken *before* the task is spawned so a
/// [`DaemonEvent::ShutdownRequested`] emitted immediately after this returns is never missed.
/// The returned handle resolves once the engine has observed shutdown and finished any in-flight
/// tick; callers should bound their wait with [`SHUTDOWN_GRACE`].
pub fn spawn(handle: DaemonHandle) -> JoinHandle<()> {
    let events = handle.subscribe();
    tokio::spawn(run(handle, events))
}

/// The sync loop. Runs until shutdown is requested or the event bus closes.
async fn run(handle: DaemonHandle, mut events: broadcast::Receiver<DaemonEvent>) {
    tracing::info!("sync engine starting");

    let host_id = match Settings::host_id().await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("failed to get host id, sync disabled: {e}");
            return;
        }
    };
    let stores = Stores::new(&handle, host_id);

    let mut period = sync_period(&*handle.settings().await);
    let mut ticker = new_ticker(period);
    let mut backoff = Backoff::new(period);

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = shutdown_requested(&mut events) => break,
        }

        // Clone rather than hold the read guard across the network round-trip, so settings reloads
        // aren't blocked for the duration of a sync.
        let settings = handle.settings().await.clone();

        // Skip periodic ticks if auto_sync is disabled AND we're not retrying a previous failure.
        // Retries must continue regardless of auto_sync.
        if !settings.auto_sync && !backoff.is_retrying() {
            tracing::debug!("auto_sync disabled, skipping periodic sync tick");
            continue;
        }

        tracing::info!("sync tick");
        match sync_once(&handle, &stores, &settings).await {
            Ok(()) => {
                backoff.reset();
                // Pick up a changed `daemon.sync_frequency`.
                let new_period = sync_period(&settings);
                if new_period != period {
                    period = new_period;
                    ticker = new_ticker(period);
                    ticker.reset_after(period);
                }
            }
            Err(e) => {
                tracing::error!("sync tick failed with {e}");
                handle.emit(DaemonEvent::SyncFailed {
                    error: e.to_string(),
                });
                let delay = backoff.next();
                tracing::error!("backing off, next sync tick in {delay:?}");
                ticker.reset_after(delay);
            }
        }
    }

    tracing::info!("sync engine stopped");
}

/// Resolve once the daemon asks for shutdown (or the bus is gone). Every other event is ignored.
///
/// `broadcast::Receiver::recv` is cancel-safe, so this is safe to race in a `select!`.
async fn shutdown_requested(events: &mut broadcast::Receiver<DaemonEvent>) {
    loop {
        match events.recv().await {
            Ok(DaemonEvent::ShutdownRequested) | Err(RecvError::Closed) => return,
            Ok(_) | Err(RecvError::Lagged(_)) => {}
        }
    }
}

fn sync_period(settings: &Settings) -> Duration {
    Duration::from_secs(settings.daemon.sync_frequency)
}

/// A ticker whose first tick fires immediately, and which never tries to "catch up" on ticks
/// missed while a slow sync was running.
fn new_ticker(period: Duration) -> Interval {
    let mut ticker = time::interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

/// The local stores rebuilt from downloaded records after each successful sync.
struct Stores {
    history: HistoryStore,
    alias: AliasStore,
    var: VarStore,
}

impl Stores {
    fn new(handle: &DaemonHandle, host_id: HostId) -> Self {
        let key = handle.encryption_key();
        Self {
            history: HistoryStore::new(handle.store().clone(), host_id, key.clone()),
            alias: AliasStore::new(handle.store().clone(), host_id, key.clone()),
            var: VarStore::new(handle.store().clone(), host_id, key.clone()),
        }
    }

    /// Fold freshly downloaded records into the local databases, announcing new history on the bus.
    async fn rebuild(&self, handle: &DaemonHandle, downloaded: &[RecordId]) {
        // `incremental_build` already yields in bounded batches - an initial sync (on backfill,
        // eg.) risks being dozens of GB of RAM otherwise.
        let mut batches =
            std::pin::pin!(self.history.incremental_build(handle.history_db(), downloaded));
        while let Some(batch) = batches.next().await {
            match batch {
                Ok(histories) if !histories.is_empty() => {
                    // Only the IDs go on the bus; the rows themselves are already in sqlite.
                    let ids: Arc<[HistoryId]> = histories.iter().map(|h| h.id).collect();
                    handle.emit(DaemonEvent::HistorySynced(ids));
                }
                Ok(_) => {}
                // Legacy behavior was to abort on the first error.
                Err(e) => {
                    tracing::error!("failed to build history from downloaded records: {e}");
                    break;
                }
            }
        }

        if let Err(e) = self.alias.build().await {
            tracing::error!("failed to rebuild alias store: {e}");
        }
        if let Err(e) = self.var.build().await {
            tracing::error!("failed to rebuild var store: {e}");
        }
    }
}

/// One sync attempt.
///
/// `Ok(())` means either the sync succeeded or there was nothing to do (not logged in, or the login
/// check itself failed); both clear any backoff. `Err` means the sync itself failed and should be
/// retried.
async fn sync_once(
    handle: &DaemonHandle,
    stores: &Stores,
    settings: &Settings,
) -> Result<(), SyncError> {
    let logged_in = match settings.logged_in().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("failed to check login status, skipping sync tick: {e}");
            return Ok(());
        }
    };

    if let Err(e) = handle.caps().refresh().await {
        tracing::debug!("capability refresh failed, keeping cached document: {e}");
    }

    if !logged_in {
        tracing::debug!("not logged in, skipping sync tick");
        return Ok(());
    }

    let engine = SyncEngine::builder()
        .store(handle.store().clone())
        .client_source(ClientSource::FromSettings {
            settings,
            caps: Some(handle.caps().clone()),
        })
        .build()
        .connect()
        .await?;
    let (uploaded, downloaded) = engine.keyed(handle.encryption_key()).sync().await?;

    tracing::info!(uploaded, downloaded = downloaded.len(), "sync complete");

    stores.rebuild(handle, &downloaded).await;

    handle.emit(DaemonEvent::SyncCompleted {
        uploaded: usize::conv(uploaded),
        downloaded: downloaded.len(),
    });

    if let Err(e) = Settings::save_sync_time().await {
        tracing::error!("failed to save sync time: {e}");
    }

    Ok(())
}

/// Exponential backoff between failed sync attempts.
///
/// Replaces the old `SyncState { Idle, Retrying }` enum: we are "retrying" exactly when a delay
/// has been handed out and not yet reset.
struct Backoff {
    /// The delay the first retry grows from — the configured sync period.
    base: Duration,
    /// Hard ceiling on any delay: [`Self::MAX_BASE`] plus a per-engine jitter.
    max: Duration,
    /// The delay used for the most recent retry; `None` while sync is healthy.
    current: Option<Duration>,
}

impl Backoff {
    /// Each retry waits this much longer than the previous one.
    const GROWTH: Range<f64> = 2.0..2.2;
    /// Never back off by more than this (plus [`Self::MAX_JITTER`]).
    const MAX_BASE: Duration = Duration::from_secs(30 * 60);
    /// Random jitter added to [`Self::MAX_BASE`] once per engine, so fleets don't sync in lockstep.
    const MAX_JITTER: Duration = Duration::from_secs(60);

    fn new(base: Duration) -> Self {
        let jitter = rand::thread_rng().gen_range(Duration::ZERO..Self::MAX_JITTER);
        Self {
            base,
            max: Self::MAX_BASE + jitter,
            current: None,
        }
    }

    /// Whether the last sync failed and we're waiting to retry it.
    fn is_retrying(&self) -> bool {
        self.current.is_some()
    }

    /// The sync succeeded (or there was nothing to sync): go back to the regular cadence.
    fn reset(&mut self) {
        self.current = None;
    }

    /// The sync failed: compute and record how long to wait before the next attempt.
    fn next(&mut self) -> Duration {
        let prev = self.current.unwrap_or(self.base);
        let delay = prev.mul_f64(rand::thread_rng().gen_range(Self::GROWTH)).min(self.max);
        self.current = Some(delay);
        delay
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rstest::{fixture, rstest};

    use super::Backoff;

    const BASE: Duration = Duration::from_secs(300);

    #[fixture]
    fn backoff() -> Backoff {
        Backoff::new(BASE)
    }

    /// `delay` is within `[lo, hi]` — the growth factor is random in `2.0..2.2`.
    fn assert_within(delay: Duration, prev: Duration) {
        assert!(delay >= prev.mul_f64(2.0), "{delay:?} < 2 * {prev:?}");
        assert!(delay <= prev.mul_f64(2.2), "{delay:?} > 2.2 * {prev:?}");
    }

    #[rstest]
    fn starts_idle(backoff: Backoff) {
        assert!(!backoff.is_retrying());
    }

    #[rstest]
    fn first_delay_grows_from_base(mut backoff: Backoff) {
        let delay = backoff.next();
        assert_within(delay, BASE);
        assert!(backoff.is_retrying());
    }

    #[rstest]
    fn delays_grow_geometrically(mut backoff: Backoff) {
        let first = backoff.next();
        let second = backoff.next();
        assert_within(second, first);
    }

    #[rstest]
    fn delay_is_capped_at_max_with_jitter() {
        let mut backoff = Backoff::new(Duration::from_secs(20 * 60));
        // 20 min * 2.0 already exceeds the 30-31 min cap, so every delay is the cap.
        for _ in 0..4 {
            assert_eq!(backoff.next(), backoff.max);
        }
        assert!(backoff.max >= Backoff::MAX_BASE);
        assert!(backoff.max < Backoff::MAX_BASE + Backoff::MAX_JITTER);
    }

    #[rstest]
    fn reset_returns_to_idle_and_restarts_from_base(mut backoff: Backoff) {
        backoff.next();
        backoff.next();
        backoff.reset();
        assert!(!backoff.is_retrying());
        assert_within(backoff.next(), BASE);
    }
}
