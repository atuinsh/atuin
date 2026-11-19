//! Background worker which periodically syncs data with the main server.

use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::{DeletedLiveRows, Sqlite};
use atuin_client::history::store::{BuildChunk, HistoryStore};
use atuin_client::record::sync::{ClientSource, SyncError as ClientSyncError, SyncSession};
use atuin_client::settings::Settings;
use atuin_common::futures::Backoff;
use atuin_domain::record::RecordId;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time;

use super::rebuild::ReplayedStores;
use crate::daemon::DaemonHandle;
use crate::search::{RebuildSettings, SharedSearchIndex};

/// Cap on the exponential backoff between failed sync attempts. Once a failing sync has ramped to
/// this interval it keeps retrying at that cadence until it succeeds -- it is never abandoned, and
/// the ramp is never reset out from under an ongoing outage.
const MAX_BACKOFF: Duration = Duration::from_mins(30);

/// Factor by which the sync retry backoff grows after each failure.
const BACKOFF_FACTOR: NonZeroU32 = NonZeroU32::new(2).unwrap();

/// How often the loop re-checks settings while periodic sync is disabled (`sync_frequency = 0`).
const DISABLED_SYNC_POLL: Duration = Duration::from_secs(300);

/// How often the loop checks for a sign-in while signed out: `atuin login` doesn't tell the
/// daemon, and the first sync shouldn't wait out a whole interval.
const SIGNED_OUT_POLL: Duration = Duration::from_secs(10);

/// Owns everything the sync loop needs across ticks.
pub struct Worker {
    handle: DaemonHandle,
    index: Arc<SharedSearchIndex>,
    history_store: HistoryStore,
    /// The kv and scripts dbs, rebuilt after a sync that brought records of theirs.
    replayed: ReplayedStores,
    /// Tells the AI-session projector that records were downloaded; `None` without one.
    ai_session_projector: Option<mpsc::UnboundedSender<()>>,
}

/// Errors that prevent the sync worker from starting.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("failed to get host id: {0}")]
    HostId(eyre::Report),
}

/// Why a single [`Worker::sync_once`] attempt did not complete a sync.
#[derive(Debug, thiserror::Error)]
pub enum SyncTickError {
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
    pub async fn new(
        handle: DaemonHandle,
        index: Arc<SharedSearchIndex>,
        ai_session_projector: Option<mpsc::UnboundedSender<()>>,
    ) -> Result<Self, StartError> {
        let host_id = handle.settings().await.host_id().await.map_err(StartError::HostId)?;

        let encryption_key = handle.encryption_key();
        // TODO(markovejnovic): We should definitely not be creating new stores, but this is pending
        //                      us having the concept of a "store bundle".
        let history_store =
            HistoryStore::new(handle.store().clone(), host_id, encryption_key.clone());
        let replayed = ReplayedStores::new(
            handle.store().clone(),
            handle.kv_db().clone(),
            handle.scripts_db().clone(),
            host_id,
            encryption_key.clone(),
        );
        Ok(Self {
            handle,
            index,
            history_store,
            replayed,
            ai_session_projector,
        })
    }

    /// Runs the sync engine in the background.
    ///
    /// This blocks the active task forever.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn run(self) {
        loop {
            let settings = self.handle.settings().await.clone();

            let Some(interval) = settings.daemon.sync_frequency else {
                tracing::debug!("periodic sync disabled (sync_frequency = 0)");
                time::sleep(DISABLED_SYNC_POLL).await;
                continue;
            };
            let interval = interval.get();

            if settings.auto_sync {
                let backoff = Backoff::Exponential {
                    initial: interval,
                    max: MAX_BACKOFF,
                    factor: BACKOFF_FACTOR,
                };
                // Retry a transient failure forever, ramping to MAX_BACKOFF and holding there until
                // it succeeds. Crucially there is no episode timeout: the old `retry(.., timeout)`
                // both abandoned a still-failing sync (resetting the ramp) and could cancel a tick
                // mid-build -- the two regressions this refactor introduced.
                backoff.retry_forever(|| self.sync_tick(&settings)).await;
            } else {
                tracing::debug!("auto_sync disabled, skipping periodic sync tick");
            }

            let wait = match settings.logged_in().await {
                Ok(false) => SIGNED_OUT_POLL,
                _ => interval,
            };
            tokio::select! {
                () = time::sleep(wait) => {}
                () = self.handle.sync_requested() => {}
            }
        }
    }

    /// Run one [`Self::sync_once`], log any failure, and translate it into a retry decision.
    async fn sync_tick(&self, settings: &Arc<Settings>) -> ControlFlow<()> {
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
    async fn sync_once(&self, settings: &Arc<Settings>) -> Result<(), SyncTickError> {
        let logged_in = settings.logged_in().await.map_err(SyncTickError::LoginCheck)?;

        if !logged_in {
            return Err(SyncTickError::NotLoggedIn);
        }

        let caps = self.handle.caps();
        if !caps.is_fetched()
            && let Err(e) = caps.refresh().await
        {
            tracing::warn!("failed to fetch the server's capabilities: {e}");
        }

        // Perform the sync
        //
        // TODO(markovejnovic): The fact that we rebuild the SyncSession on every single iteration
        //                      of the loop feels wrong to me, but the legacy behavior was this, and
        //                      in favor of not breaking anything, I prefer to keep it this way.
        let session = SyncSession::builder()
            .store(self.handle.store().clone())
            .client_source(ClientSource::FromSettings {
                settings,
                caps: Some(self.handle.caps().clone()),
            })
            .build()
            .connect()
            .await?;
        let (uploaded_count, downloaded_records) =
            session.keyed(self.handle.encryption_key()).sync().await?;

        tracing::info!(
            uploaded = uploaded_count,
            downloaded = downloaded_records.len(),
            "sync complete"
        );

        // Queued rather than awaited: the projector may still be waiting out startup recovery, and
        // that must not hold up this sync or the next.
        if let Some(projector) = &self.ai_session_projector
            && !downloaded_records.is_empty()
        {
            let _ = projector.send(());
        }

        self.index_downloaded_records(&downloaded_records).await;
        self.replayed.rebuild_changed().await;

        // Store sync time
        if let Err(e) = settings.save_sync_time().await {
            tracing::error!("failed to save sync time: {e}");
        }

        Ok(())
    }

    /// Build freshly-downloaded records into `history_db` and index the resulting rows.
    ///
    /// If a downloaded delete removed a live row, the commands of the deleted rows are updated in
    /// the index afterwards. See [`build_downloaded_records`].
    async fn index_downloaded_records(&self, ids: &[RecordId]) {
        let db = self.handle.history_db();
        let deleted = build_downloaded_records(&self.history_store, db, &self.index, ids).await;
        if !deleted.is_empty() {
            // Every permit from this build is dropped by now. So the update can't wait on one of
            // them.
            let latest = |settings: RebuildSettings<'_>| Some(settings.latest.clone());
            self.index.sync_deleted(db, deleted.into_commands(), latest).await;
        }
    }
}

/// Build the downloaded records `ids` into `db`, and add the new rows to `index`.
///
/// Each chunk of created entries is registered with the search index before it's saved. The
/// entries are added to the index after the save. See [`AddPermit`](crate::search::AddPermit).
///
/// A created entry whose id already has a row in `db` is skipped. The row may be live, and then
/// the index already has it. The row may also be a tombstone from an older version, and then the
/// entry must stay out of the index.
///
/// Returns the live rows that downloaded deletes removed. Their commands need updating in the
/// index. That includes a row that was created earlier in this same build. A delete of a missing
/// row or a tombstone removes no live row.
async fn build_downloaded_records(
    history_store: &HistoryStore,
    db: &Sqlite,
    index: &SharedSearchIndex,
    ids: &[RecordId],
) -> DeletedLiveRows {
    let mut deleted = DeletedLiveRows::default();
    let mut chunks = pin!(history_store.incremental_build(db, ids));
    while let Some(chunk) = chunks.next().await {
        match chunk {
            BuildChunk::Create(mut chunk) => {
                if let Err(e) = chunk.skip_existing().await {
                    tracing::error!("failed to check downloaded history against the db: {e}");
                    continue;
                }
                let permit = index.begin_add().await;
                permit.register(chunk.histories());
                match chunk.commit().await {
                    Ok(()) => permit.add_history(chunk.histories()),
                    Err(e) => {
                        permit.unregister(chunk.histories().iter().map(|h| h.id));
                        drop(permit);
                        tracing::error!("failed to build history from downloaded records: {e}");
                    }
                }
            }
            BuildChunk::Delete(chunk) => match chunk.commit().await {
                Ok(chunk_deleted) => deleted.extend(chunk_deleted),
                Err(e) => {
                    tracing::error!("failed to apply downloaded history deletes: {e}");
                }
            },
        }
    }
    deleted
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use atuin_client::database::Sqlite;
    use atuin_client::history::History;
    use atuin_client::history::store::HistoryStore;
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_domain::record::HostId;
    use jiff::Timestamp;
    use rstest::{fixture, rstest};

    use super::build_downloaded_records;
    use crate::search::{SearchIndex, SearchIndexSettings, SharedSearchIndex};

    /// An empty history db.
    #[fixture]
    async fn db() -> Sqlite {
        Sqlite::in_memory(Duration::from_secs(5)).await.unwrap()
    }

    /// An empty history record store.
    #[fixture]
    async fn history_store() -> HistoryStore {
        let store = SqliteStore::in_memory(Duration::from_secs(5)).await.unwrap();
        HistoryStore::new(store, HostId(atuin_common::utils::uuid_v7()), Key::generate())
    }

    fn history(command: &str) -> History {
        History::import()
            .timestamp(Timestamp::now())
            .command(command)
            .cwd("/tmp")
            .author("test-user")
            .build()
            .into()
    }

    /// A downloaded create for an id that history.db holds as a tombstone isn't indexed. The
    /// tombstone isn't a live row. So a later downloaded delete of that id removes no live row and
    /// triggers no rebuild. The entry must not be in the index by then.
    #[rstest]
    #[tokio::test]
    async fn downloaded_create_of_a_tombstone_is_not_indexed(
        #[future(awt)] db: Sqlite,
        #[future(awt)] history_store: HistoryStore,
    ) {
        let history = history("echo deleted");
        let mut tombstone = history.clone();
        tombstone.deleted_at = Some(Timestamp::now());
        db.save_bulk([&tombstone]).await.unwrap();
        let index = SharedSearchIndex::from(SearchIndex::default());

        let (create, _) = history_store.push(history.clone()).await.unwrap();
        let deleted = build_downloaded_records(&history_store, &db, &index, &[create]).await;
        assert!(deleted.is_empty());
        assert_eq!(index.read().invocations("echo deleted"), 0);

        let (delete, _) = history_store.delete(history.id).await.unwrap();
        let deleted = build_downloaded_records(&history_store, &db, &index, &[delete]).await;
        assert!(deleted.is_empty(), "only the tombstone was removed");
        assert_eq!(index.read().invocations("echo deleted"), 0);
    }

    /// A downloaded create for an id that history.db already holds as a live row isn't indexed
    /// again. The index already counts that row.
    #[rstest]
    #[tokio::test]
    async fn downloaded_create_of_an_existing_row_is_counted_once(
        #[future(awt)] db: Sqlite,
        #[future(awt)] history_store: HistoryStore,
    ) {
        let history = history("echo existing");
        db.save(&history).await.unwrap();
        let index = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        index.fill(&db).await.unwrap();
        assert_eq!(index.read().invocations("echo existing"), 1);

        let (create, _) = history_store.push(history).await.unwrap();
        let deleted = build_downloaded_records(&history_store, &db, &index, &[create]).await;
        assert!(deleted.is_empty());
        assert_eq!(index.read().invocations("echo existing"), 1);
    }

    /// A downloaded delete of a live row reports the row's command, so that only that command is
    /// updated in the index.
    #[rstest]
    #[tokio::test]
    async fn downloaded_delete_of_a_live_row_reports_its_command(
        #[future(awt)] db: Sqlite,
        #[future(awt)] history_store: HistoryStore,
    ) {
        let history = history("echo doomed");
        db.save(&history).await.unwrap();
        let index = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        index.fill(&db).await.unwrap();

        let (delete, _) = history_store.delete(history.id).await.unwrap();
        let deleted = build_downloaded_records(&history_store, &db, &index, &[delete]).await;

        assert_eq!(deleted.len(), 1);
        assert_eq!(
            deleted.commands(),
            &std::collections::HashSet::from(["echo doomed".to_owned()])
        );
    }
}
