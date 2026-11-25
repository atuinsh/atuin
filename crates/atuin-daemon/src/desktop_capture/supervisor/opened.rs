//! The store and the index, and the tasks that maintain them.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use atuin_client::settings::{DiskUsageLimit, Settings};
use atuin_domain::record::HostId;
use parking_lot::Mutex;
use tokio::time::MissedTickBehavior;
use tokio_util::task::{AbortOnDropHandle, TaskTracker};
use tracing::{error, warn};

use crate::desktop_capture::index::{DesktopIndex, IndexError};
use crate::desktop_capture::reconciler::spawn_reconciler;
use crate::desktop_capture::retention::Retention;
use crate::desktop_capture::store::{DeleteFailed, Deleted, DesktopStore, Selection, StoreError};
use crate::desktop_capture::store_exists;
use crate::disk_budget::Gc;

#[derive(Debug, thiserror::Error)]
#[error("could not open the desktop capture store at {path}: {source}")]
pub struct OpenError {
    path: PathBuf,
    #[source]
    source: StoreError,
}

/// A deletion from the store and the index that failed.
#[derive(Debug, thiserror::Error)]
pub enum StorageDeleteError {
    /// The failure carries what the store removed before the store failed. The daemon also tries
    /// to remove that content from the search index, and logs a failure of that removal.
    #[error("could not delete from the desktop capture store: {0}")]
    Store(#[from] DeleteFailed),
    #[error("deleted from the store, but could not remove the content from the search index: {0}")]
    Index(#[from] IndexError),
}

/// The store and the index, once they are open.
pub struct Opened {
    store: DesktopStore,
    index: Option<DesktopIndex>,
    reconciler: Mutex<Option<AbortOnDropHandle<()>>>,
    /// The deletions in flight, which run in tasks of their own, so that a shutdown that aborts
    /// the supervisor cannot stop a deletion between the store and the index.
    deletions: TaskTracker,
    gc: Mutex<Option<Gc>>,
    /// Tries again the upkeep that the store and the index owe. `None` once closed.
    owed_upkeep: Mutex<Option<AbortOnDropHandle<()>>>,
}

/// How often the daemon tries again the upkeep that the store and the index owe.
const OWED_UPKEEP_INTERVAL: Duration = Duration::from_mins(15);

/// A step of the upkeep that the store and the index owe, which failed.
#[derive(Debug, thiserror::Error)]
enum UpkeepError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

impl Opened {
    /// Opens the store and the index. An index that fails to open is logged, and the store
    /// works without it.
    async fn open(storage: &Storage) -> Result<Self, OpenError> {
        let store =
            DesktopStore::open(&storage.dir, storage.host).await.map_err(|source| OpenError {
                path: storage.dir.clone(),
                source,
            })?;
        let index = storage.open_index().await;
        if let Some(index) = &index {
            follow_watermark(&store, index).await;
        }
        if let Some(index) = &index {
            catch_up_index(&store, index).await;
        }
        let budget = storage.max_disk_usage.resolve_for_path(&storage.dir).unwrap_or_else(|err| {
            warn!(?err, path = ?storage.dir, "could not resolve the desktop capture disk budget");
            None
        });
        let retention =
            budget.is_some().then(|| Arc::new(Retention::new(store.clone(), index.clone())));
        let gc = budget
            .zip(retention.as_ref())
            .map(|(budget, retention)| Gc::spawn(Arc::clone(retention), budget));
        let owed_upkeep = spawn_owed_upkeep(store.clone(), index.clone(), retention);
        let opened = Self {
            store,
            index,
            reconciler: Mutex::new(None),
            deletions: TaskTracker::new(),
            gc: Mutex::new(gc),
            owed_upkeep: Mutex::new(Some(owed_upkeep)),
        };
        *opened.reconciler.lock() = opened.spawn_reconciler();
        Ok(opened)
    }

    #[must_use]
    pub fn store(&self) -> &DesktopStore {
        &self.store
    }

    #[must_use]
    pub fn index(&self) -> Option<&DesktopIndex> {
        self.index.as_ref()
    }

    /// Ends the segments that no helper is writing to anymore.
    pub async fn close_open_segments(&self) {
        if let Err(err) = self.store.close_open_segments().await {
            warn!(?err, "could not end the open desktop capture segments");
        }
    }

    /// Deletes the segments that `selection` selects from the store, and their contents from the
    /// search index.
    ///
    /// Once the call returns, the files of the store and of the index don't hold any trace of
    /// the deleted segments, unless a step of the deletion failed. Each failure is logged. The
    /// store and the index note what they still owe, and the daemon tries the owed steps again
    /// at the next open and every [`OWED_UPKEEP_INTERVAL`] until they succeed.
    ///
    /// The caller guarantees that no helper run is writing, that deletions run one at a time,
    /// and that each call runs to its end. A deletion that has begun runs to its end in the
    /// store and the index even when the caller stops waiting, and [`close`](Self::close) waits
    /// for it.
    pub async fn delete(&self, selection: Selection) -> Result<Deleted, StorageDeleteError> {
        // A reconciliation that runs across the deletion could index content that the deletion
        // removes, after the index has removed it.
        self.stop_reconciler().await;
        let deletion =
            self.deletions.spawn(delete_stopped(self.store.clone(), self.index.clone(), selection));
        let deleted = match deletion.await {
            Ok(deleted) => deleted,
            Err(err) => std::panic::resume_unwind(err.into_panic()),
        };
        *self.reconciler.lock() = self.spawn_reconciler();
        deleted
    }

    fn spawn_reconciler(&self) -> Option<AbortOnDropHandle<()>> {
        self.index
            .clone()
            .map(|index| AbortOnDropHandle::new(spawn_reconciler(self.store.clone(), index)))
    }

    /// Aborts the reconciler and waits until it has stopped.
    async fn stop_reconciler(&self) {
        let reconciler = self.reconciler.lock().take();
        if let Some(reconciler) = reconciler {
            reconciler.abort();
            let _ = reconciler.await;
        }
    }

    /// Stops the reconciler, the GC, and the retries of owed upkeep, and closes the store.
    ///
    /// Closing waits for the reconciler, for the deletions in flight, and for a rewrite of the
    /// store that is already running, which can take a while on a large store.
    pub async fn close(&self) {
        drop(self.gc.lock().take());
        drop(self.owed_upkeep.lock().take());
        self.stop_reconciler().await;
        self.deletions.close();
        self.deletions.wait().await;
        if let Err(err) = self.store.close().await {
            error!(?err, "could not close the desktop capture store");
        }
    }
}

/// Whether a deletion removed anything from the store.
fn removed_any(deleted: &Result<Deleted, StorageDeleteError>) -> bool {
    match deleted {
        Ok(deleted) => !deleted.is_empty(),
        Err(StorageDeleteError::Store(failed)) => !failed.deleted.is_empty(),
        // The store removed the content before the index failed.
        Err(StorageDeleteError::Index(_)) => true,
    }
}

/// Deletes from the store and the index while no reconciler is running, and then scrubs the
/// index and rewrites the store so that their files drop the deleted content.
async fn delete_stopped(
    store: DesktopStore,
    index: Option<DesktopIndex>,
    selection: Selection,
) -> Result<Deleted, StorageDeleteError> {
    let deleted = delete_from_both(&store, index.as_ref(), selection).await;
    if !removed_any(&deleted) {
        return deleted;
    }
    if let Some(index) = &index
        && let Err(err) = index.scrub().await
    {
        warn!(
            ?err,
            "could not scrub the desktop capture search index after a deletion, so the index's \
             files can still hold the deleted text"
        );
    }
    if let Err(err) = store.rewrite().await {
        warn!(
            ?err,
            "could not rewrite the desktop capture store after a deletion, so the store's files \
             can still hold the deleted records until a later rewrite"
        );
    }
    deleted
}

/// Tries every [`OWED_UPKEEP_INTERVAL`] the upkeep that the store and the index owe: the
/// store's rewrite, the index's removals and scrub, and the rewrite that evictions call for.
fn spawn_owed_upkeep(
    store: DesktopStore,
    index: Option<DesktopIndex>,
    retention: Option<Arc<Retention>>,
) -> AbortOnDropHandle<()> {
    AbortOnDropHandle::new(tokio::spawn(async move {
        let mut interval = tokio::time::interval(OWED_UPKEEP_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The first tick completes at once, and the open has just carried out the upkeep.
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(err) = store.rewrite_if_owed().await {
                warn!(?err, "could not carry out an owed rewrite of the desktop capture store");
            }
            if let Some(index) = &index {
                catch_up_index(&store, index).await;
            }
            if let Some(retention) = &retention {
                retention.rewrite_if_due().await;
            }
        }
    }))
}

/// Removes from the index the segments that the store removed while the index didn't follow,
/// and scrubs the index when a removal owes a scrub. A failure is only logged, because the
/// store and the index keep their notes until the steps succeed.
async fn catch_up_index(store: &DesktopStore, index: &DesktopIndex) {
    if let Err(err) = remove_what_the_store_lacks(store, index).await {
        warn!(
            ?err,
            "could not remove the segments that the desktop capture store removed from the index"
        );
    }
    match index.owes_scrub().await {
        Ok(false) => {}
        Ok(true) => {
            if let Err(err) = index.scrub().await {
                warn!(?err, "could not carry out an owed scrub of the desktop capture index");
            }
        }
        Err(err) => warn!(?err, "could not read whether the desktop capture index owes a scrub"),
    }
}

/// Removes from the index every segment that the store doesn't hold, when the store notes that
/// the index owes a removal.
async fn remove_what_the_store_lacks(
    store: &DesktopStore,
    index: &DesktopIndex,
) -> Result<(), UpkeepError> {
    let _following = index.follow_store().await;
    if !store.owes_index_removal().await? {
        return Ok(());
    }
    let missing = store.missing_segments(index.indexed_segments().await?).await?;
    if !missing.is_empty() {
        index.remove(&missing).await?;
    }
    store.settle_index_removal().await?;
    Ok(())
}

/// Deletes from the store, and then removes what the store deleted from the index.
async fn delete_from_both(
    store: &DesktopStore,
    index: Option<&DesktopIndex>,
    selection: Selection,
) -> Result<Deleted, StorageDeleteError> {
    let Some(index) = index else {
        return Ok(store.delete(selection).await?);
    };
    // An eviction between this deletion and the index's removal could otherwise settle the
    // store's note of this deletion's removal from the index.
    let _following = index.follow_store().await;
    let owed_before = store.owes_index_removal().await.unwrap_or_else(|err| {
        warn!(?err, "could not read whether the desktop capture index owes a removal");
        true
    });
    let deleted = match store.delete(selection).await {
        Ok(deleted) => deleted,
        Err(failed) => {
            if let Err(err) = index.remove(&failed.deleted.segments).await {
                warn!(
                    ?err,
                    "could not remove the content of a failed deletion from the desktop capture \
                     search index"
                );
            }
            return Err(failed.into());
        }
    };
    index.remove(&deleted.segments).await?;
    // A removal that the index owed before this deletion is still waiting for the daemon's retry.
    if !owed_before
        && !deleted.segments.is_empty()
        && let Err(err) = store.settle_index_removal().await
    {
        warn!(?err, "could not note that the desktop capture index followed a deletion");
    }
    Ok(deleted)
}

/// Makes the store's new records follow the index's reconciliation watermark, so that
/// reconciliation reaches them.
async fn follow_watermark(store: &DesktopStore, index: &DesktopIndex) {
    match index.reconciled_through().await {
        Ok(Some(watermark)) => {
            if let Err(err) = store.write_after(watermark).await {
                warn!(?err, "could not move the desktop capture record ids past the watermark");
            }
        }
        Ok(None) => {}
        Err(err) => warn!(?err, "could not read the desktop capture reconciliation watermark"),
    }
}

/// The file name of the search index, in the data directory.
const INDEX_FILE: &str = "desktop-capture-index.sqlite";

/// Where the store and the index live, and the slot that holds them once they open.
pub struct Storage {
    dir: PathBuf,
    index: PathBuf,
    host: HostId,
    max_disk_usage: DiskUsageLimit,
    opened: OnceLock<Opened>,
    /// Whether the store may open soon: from the start until the supervisor leaves the store
    /// closed, and during each attempt to open the store.
    opening: tokio::sync::watch::Sender<bool>,
    /// Counts the attempts to open the store that have ended, for the tests.
    #[cfg(test)]
    attempts: tokio::sync::watch::Sender<u32>,
    /// Whether the index opens with the store. A test that pauses the clock can't wait for the
    /// index.
    #[cfg(test)]
    opens_index: bool,
}

impl Storage {
    pub fn new(settings: &Settings, host: HostId) -> Self {
        Self {
            dir: settings.desktop_capture_dir(),
            index: settings.data_dir.join(INDEX_FILE),
            host,
            max_disk_usage: settings.desktop_capture.max_disk_usage,
            opened: OnceLock::new(),
            opening: tokio::sync::watch::Sender::new(true),
            #[cfg(test)]
            attempts: tokio::sync::watch::Sender::new(0),
            #[cfg(test)]
            opens_index: crate::desktop_capture::testing::opens_index(&settings.data_dir),
        }
    }

    /// Opens the index. An index that fails to open is logged, and the store works without it.
    async fn open_index(&self) -> Option<DesktopIndex> {
        #[cfg(test)]
        if !self.opens_index {
            return None;
        }
        DesktopIndex::open(&self.index)
            .await
            .inspect_err(|err| {
                error!(?err, path = ?self.index, "desktop capture search is unavailable");
            })
            .ok()
    }

    /// Counts the attempts to open the store that have ended.
    #[cfg(test)]
    pub fn open_attempts(&self) -> tokio::sync::watch::Receiver<u32> {
        self.attempts.subscribe()
    }

    /// Whether an earlier run left a store behind.
    pub async fn has_store(&self) -> bool {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || store_exists(&dir))
            .await
            .expect("the check for a desktop capture store panicked")
    }

    /// The store and the index, when they are open.
    pub fn opened(&self) -> Option<&Opened> {
        self.opened.get()
    }

    /// The store and the index, once they are open. If the store may open soon, the call waits
    /// up to `patience` for the store to open.
    pub async fn opened_within(&self, patience: Duration) -> Option<&Opened> {
        if let Some(opened) = self.opened.get() {
            return Some(opened);
        }
        let mut opening = self.opening.subscribe();
        let _ = tokio::time::timeout(patience, opening.wait_for(|opening| !opening)).await;
        self.opened.get()
    }

    /// Notes that the store stays closed until a later attempt opens it, so that the reads that
    /// wait for the store fail.
    pub fn leave_closed(&self) {
        self.opening.send_replace(false);
    }

    /// The store and the index. The call opens them when they are not open yet.
    pub async fn get_or_open(&self) -> Result<&Opened, OpenError> {
        if let Some(opened) = self.opened.get() {
            return Ok(opened);
        }
        let _attempt = OpenAttempt::begin(self);
        #[cfg(test)]
        crate::desktop_capture::testing::held_open(&self.dir).await;
        let opened = Opened::open(self).await;
        #[cfg(test)]
        self.attempts.send_modify(|attempts| *attempts += 1);
        let opened = opened?;
        Ok(self.opened.get_or_init(|| opened))
    }
}

/// Shows an attempt to open the store in [`Storage::opening`] until the attempt ends or the
/// caller drops the attempt.
struct OpenAttempt<'s>(&'s Storage);

impl<'s> OpenAttempt<'s> {
    fn begin(storage: &'s Storage) -> Self {
        storage.opening.send_replace(true);
        Self(storage)
    }
}

impl Drop for OpenAttempt<'_> {
    fn drop(&mut self) {
        self.0.leave_closed();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use rstest::rstest;
    use sqlx::Connection as _;

    use super::*;
    use crate::desktop_capture::store::{SegmentFilter, SegmentKey, Selection};
    use crate::desktop_capture::testing::records::{
        dialog, everything, snapshot, started, window, written,
    };
    use crate::desktop_capture::testing::{HOST, Home, eventually, files_holding, home};

    async fn indexed(opened: &Opened) -> Vec<SegmentKey> {
        let mut segments =
            opened.index().expect("an index").indexed_segments().await.expect("indexed segments");
        segments.sort_by_key(|segment| segment.0);
        segments
    }

    /// New records follow the index's reconciliation watermark, also when the watermark is
    /// ahead of every stored record.
    ///
    /// That happens after a deletion of the newest records and a clock that was set back.
    #[rstest]
    #[tokio::test]
    async fn new_records_follow_the_reconciliation_watermark(home: Home) {
        let settings = home.settings(&["gedit"]);
        let now = SystemTime::now();
        let ahead = SystemTime::now() + Duration::from_hours(24);
        let millis = ahead.duration_since(SystemTime::UNIX_EPOCH).expect("after 1970").as_millis();
        let millis = u64::try_from(millis).expect("a small time");
        let watermark = uuid::Builder::from_unix_timestamp_millis(millis, &[0; 10]).into_uuid();
        let index = DesktopIndex::open(&settings.data_dir.join(INDEX_FILE)).await.expect("open");
        index.add_reconciled(Vec::new(), watermark).await.expect("set the watermark");

        for _ in 0..2 {
            let storage = Storage::new(&settings, HOST);
            let opened = storage.get_or_open().await.expect("open");
            let items = written(opened.store(), [
                started(1, "gedit", "notes", now),
                snapshot(1, now, window("zebra")),
            ])
            .await;
            assert!(items[0].key.segment.0 > watermark, "a new record follows the watermark");
            opened.close().await;
        }
    }

    /// A deletion whose caller stops waiting between the store and the index still removes the
    /// content from the index, and closing waits for it.
    ///
    /// Runs in real time, because the index's pool times out while the clock is paused.
    #[rstest]
    #[tokio::test]
    async fn an_abandoned_deletion_still_leaves_the_index(home: Home) {
        let settings = home.settings(&["gedit"]);
        let now = SystemTime::now();
        let store = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");
        let _ =
            written(&store, [started(1, "gedit", "notes", now), snapshot(1, now, window("zebra"))])
                .await;
        store.close().await.expect("close");
        drop(store);
        let storage = Arc::new(Storage::new(&settings, HOST));
        let opened = storage.get_or_open().await.expect("open");
        let _ = eventually(async || indexed(opened).await, |segments| segments.len() == 1).await;
        // A connection that holds the index's write lock keeps the deletion between the store
        // and the index.
        let index_path = settings.data_dir.join(INDEX_FILE);
        let mut holder = sqlx::SqliteConnection::connect(&index_path.to_string_lossy())
            .await
            .expect("connect to the index");
        atuin_common::db::query("BEGIN IMMEDIATE").execute(&mut holder).await.expect("lock");

        let deletion = tokio::spawn({
            let storage = Arc::clone(&storage);
            async move {
                let opened = storage.opened().expect("open");
                opened.delete(everything().into()).await
            }
        });
        let store = opened.store();
        let _ = eventually(
            async || store.segments(everything()).await.expect("segments"),
            Vec::is_empty,
        )
        .await;
        deletion.abort();
        let _ = deletion.await;
        atuin_common::db::query("ROLLBACK").execute(&mut holder).await.expect("unlock");
        drop(holder);
        opened.close().await;

        let index = DesktopIndex::open(&index_path).await.expect("open the index");
        assert_eq!(index.indexed_segments().await.expect("indexed segments"), []);
    }

    /// What a test deletes.
    #[derive(Debug, Clone, Copy)]
    enum Deletion {
        ById,
        ByApp,
        ByTime,
        Everything,
    }

    async fn segment_keys(store: &DesktopStore) -> Vec<SegmentKey> {
        let segments = store.segments(everything()).await.expect("segments");
        segments.into_iter().map(|segment| segment.key).collect()
    }

    /// A deletion doesn't leave any copy of the deleted text in any file of the store or the index,
    /// also after the store opens again.
    ///
    /// fjall keeps every write in its journal until the journal grows past 64 MB, and a removal
    /// alone leaves the journal as it is. SQLite keeps removed rows in the index's journal and
    /// free pages. The index's full-text segments hold each word of the text on its own.
    #[rstest]
    #[tokio::test]
    async fn a_deletion_leaves_no_trace_in_the_data_files(
        home: Home,
        #[values(Deletion::ById, Deletion::ByApp, Deletion::ByTime, Deletion::Everything)]
        deletion: Deletion,
    ) {
        const SECRET: &str = "zanzibar quokka 7319";
        let settings = home.settings(&["gedit"]);
        let dir = &settings.data_dir;
        let holding_secret =
            || SECRET.split(' ').flat_map(|word| files_holding(dir, word)).collect::<Vec<_>>();
        let earlier = SystemTime::now() - Duration::from_hours(2);
        let now = SystemTime::now();
        let storage = Storage::new(&settings, HOST);
        let opened = storage.get_or_open().await.expect("open");
        let secret =
            written(opened.store(), [dialog("kate", SECRET, earlier, window(SECRET))]).await;
        let kept = written(opened.store(), [dialog("gedit", "kept", now, window("kept"))]).await;
        let index = opened.index().expect("an index");
        index.add(secret.iter().chain(&kept).cloned()).await.expect("index");
        let holding = holding_secret();
        assert!(
            holding.iter().any(|path| path.starts_with(settings.desktop_capture_dir())),
            "the store's journal holds the text"
        );
        assert!(
            holding.iter().any(|path| !path.starts_with(settings.desktop_capture_dir())),
            "the index holds the text"
        );

        let selection = match deletion {
            Deletion::ById => Selection::Segment(secret[0].key.segment),
            Deletion::ByApp => Selection::from(SegmentFilter {
                app: Some("kate".to_owned()),
                ..everything()
            }),
            Deletion::ByTime => Selection::from(SegmentFilter {
                to: now - Duration::from_hours(1),
                ..everything()
            }),
            Deletion::Everything => Selection::from(everything()),
        };
        let _ = opened.delete(selection).await.expect("delete");

        assert_eq!(holding_secret(), Vec::<PathBuf>::new());
        let expected = match deletion {
            Deletion::Everything => Vec::new(),
            Deletion::ById | Deletion::ByApp | Deletion::ByTime => vec![kept[0].key.segment],
        };
        assert_eq!(segment_keys(opened.store()).await, expected);
        opened.close().await;

        let storage = Storage::new(&settings, HOST);
        let opened = storage.get_or_open().await.expect("open again");
        assert_eq!(segment_keys(opened.store()).await, expected);
        assert_eq!(holding_secret(), Vec::<PathBuf>::new());
        opened.close().await;
    }

    /// The next open removes from the index the segments and text that a deletion left behind. For
    /// example, the daemon can stop between the deletion from the store and the removal from the
    /// index.
    #[rstest]
    #[tokio::test]
    async fn the_next_open_removes_a_deletion_that_the_index_missed(home: Home) {
        const SECRET: &str = "zanzibar quokka 7319";
        let settings = home.settings(&["gedit"]);
        let dir = &settings.data_dir;
        let holding_secret =
            || SECRET.split(' ').flat_map(|word| files_holding(dir, word)).collect::<Vec<_>>();
        let now = SystemTime::now();
        let store = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");
        let index = DesktopIndex::open(&settings.data_dir.join(INDEX_FILE)).await.expect("open");
        let secret = written(&store, [dialog("kate", SECRET, now, window(SECRET))]).await;
        let kept = written(&store, [dialog("gedit", "kept", now, window("kept"))]).await;
        index.add(secret.iter().chain(&kept).cloned()).await.expect("index");
        store.delete(Selection::Segment(secret[0].key.segment)).await.expect("delete");
        store.close().await.expect("close");
        drop(store);
        drop(index);
        assert!(!holding_secret().is_empty(), "the index holds the text");

        for _ in 0..2 {
            let storage = Storage::new(&settings, HOST);
            let opened = storage.get_or_open().await.expect("open");

            assert_eq!(indexed(opened).await, [kept[0].key.segment]);
            assert_eq!(holding_secret(), Vec::<PathBuf>::new());
            assert!(!opened.store().owes_index_removal().await.expect("read the note"));
            opened.close().await;
        }
    }

    #[rstest]
    #[tokio::test]
    async fn a_deleted_segment_leaves_the_index_for_good(home: Home) {
        let settings = home.settings(&["gedit"]);
        let now = SystemTime::now();
        // The reconciler indexes the store once when the reconciler starts, so the first two
        // segments are in the store before the reconciler starts.
        let store = DesktopStore::open(settings.desktop_capture_dir(), HOST).await.expect("open");
        let zebra =
            written(&store, [started(1, "kate", "notes", now), snapshot(1, now, window("zebra"))])
                .await;
        let yak =
            written(&store, [started(2, "gedit", "notes", now), snapshot(2, now, window("yak"))])
                .await;
        store.close().await.expect("close");
        drop(store);
        let zebra = zebra[0].key.segment;
        let yak = yak[0].key.segment;

        let storage = Storage::new(&settings, HOST);
        let opened = storage.get_or_open().await.expect("open");
        let _ = eventually(async || indexed(opened).await, |segments| segments.len() == 2).await;
        // The reconciler indexes this segment only once a deletion restarts it.
        let xerus = written(opened.store(), [
            started(3, "gedit", "other", now),
            snapshot(3, now, window("xerus")),
        ])
        .await;
        let xerus = xerus[0].key.segment;

        let deleted = opened
            .delete(Selection::from(SegmentFilter {
                app: Some("kate".to_owned()),
                ..everything()
            }))
            .await
            .expect("delete");

        assert_eq!(deleted.segments, [zebra]);
        assert!(!indexed(opened).await.contains(&zebra));
        let mut rest = vec![yak, xerus];
        rest.sort_by_key(|segment| segment.0);
        let _ =
            eventually(async || indexed(opened).await, |segments| segments.contains(&xerus)).await;
        assert_eq!(indexed(opened).await, rest);
        opened.close().await;
    }
}
