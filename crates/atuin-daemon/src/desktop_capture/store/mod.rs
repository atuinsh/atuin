//! Durable storage for desktop capture records.
//!
//! [`DesktopStore`] runs each operation on a blocking thread. The operations themselves live in
//! the modules below it:
//!
//! - [`keyspaces`]: typed reads and writes over the keyspaces of [`schema`].
//! - [`run`]: the segments that a helper run is writing to.
//! - [`write`](mod@write): the transactions that start, observe, and end segments.
//! - [`query`]: segments, snapshots, and search hits.
//! - [`reconcile`]: passes over the stored contents for the search index.
//! - [`evict`]: deletes the oldest records.
//! - [`delete`](mod@delete): deletes the records that match a filter.
//! - [`tree`] and [`delta`]: how a segment's contents are stored and expanded.
//! - [`format`](mod@format): the format of the stored data.
//! - [`rewrite`](mod@rewrite) and [`directory`]: replace the database with a fresh copy, so
//!   that the store's files don't keep any trace of removed entries.

mod delete;
mod delta;
mod directory;
mod evict;
mod format;
mod keyspaces;
mod model;
mod open;
mod query;
mod reconcile;
mod rewrite;
mod run;
mod schema;
mod tree;
mod write;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atuin_desktop_capture::{Record, SegmentId};
use atuin_domain::record::HostId;
use directory::StoreDir;
use evict::Evictor;
use keyspaces::{Carried, Keyspaces, Usage};
#[cfg(test)]
pub use model::SnapshotRead;
pub use model::{
    Admission, AppTally, ContentKey, Counts, DeleteFailed, Deleted, DialogText, Evicted, IndexItem,
    Kinds, Matched, MatchedContent, RecentActivity, RecordText, RecordTexts, SegmentFilter,
    SegmentKey, SegmentKind, SegmentSnapshots, SegmentSummary, Selection, SnapshotSummary,
    SnapshotText, StoreError, Tally, TextBudget, Which, Written,
};
use open::OpenSegments;
use parking_lot::Mutex;
pub use query::MatchedRecord;
use query::View;
pub use reconcile::{Batch, Expansion, Pass};
use rewrite::Rewritten;
pub use run::RunState;
use run::{BackgroundEnd, BackgroundStep, OpenSegment, RunSegment, Unstarted};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use tokio::time::Instant;
use tracing::{info, warn};
use uuid::Uuid;
use write::{ActiveStart, FirstObservation, Observation};

use crate::flusher::{Flusher, Persist as _};

/// What a write of an active segment's start did.
enum Start {
    Started(OpenSegment),
    Failed {
        source: StoreError,
        unstarted: Unstarted,
    },
}

/// The keyspaces and the process's own state of them.
struct Inner {
    keyspaces: Arc<Keyspaces>,
    open: OpenSegments,
    evictor: Evictor,
    /// Declared last, so that the store directory stays locked until the database has closed.
    dir: StoreDir,
}

impl Inner {
    fn view(&self) -> View<'_> {
        View::new(&self.keyspaces, &self.open)
    }

    /// Removes the generations that a failed removal left on disk.
    fn remove_replaced(&self) -> Result<(), StoreError> {
        if self.dir.owes_removal() {
            self.dir.remove_replaced().map_err(StoreError::Files)?;
        }
        Ok(())
    }
}

/// A store whose rewritten database failed to open.
///
/// The store directory stays locked, so that no other process opens the store while this
/// process tries to open the database again.
struct Unopened {
    carried: Carried,
    open: OpenSegments,
    evictor: Evictor,
    dir: StoreDir,
    error: Arc<StoreError>,
    /// The earliest time at which an operation tries to open the database again.
    retry_at: Instant,
}

impl Unopened {
    fn reopen(mut self: Box<Self>) -> Result<Inner, Box<Self>> {
        match Keyspaces::reopen(self.dir.database(), &self.carried) {
            Ok(keyspaces) => {
                let Self {
                    open, evictor, dir, ..
                } = *self;
                Ok(Inner {
                    keyspaces: Arc::new(keyspaces),
                    open,
                    evictor,
                    dir,
                })
            }
            Err(error) => {
                warn!(%error, "could not open the rewritten desktop capture store again");
                self.error = Arc::new(error);
                Err(self)
            }
        }
    }
}

/// What the store holds.
enum Slot {
    Open(Arc<Inner>),
    Unopened(Box<Unopened>),
    Closed,
}

impl Slot {
    fn open(&self) -> Option<&Inner> {
        match self {
            Self::Open(inner) => Some(inner),
            Self::Unopened(_) | Self::Closed => None,
        }
    }

    /// The error of an operation on a store that isn't open.
    fn unavailable(&self) -> StoreError {
        match self {
            Self::Unopened(unopened) => StoreError::Reopen(Arc::clone(&unopened.error)),
            Self::Open(_) | Self::Closed => StoreError::Closed,
        }
    }
}

/// The name of the store in the flusher's log.
const FLUSHER_NAME: &str = "desktop capture";

/// How long a rewrite waits before each further attempt to open its rewritten database.
const REOPEN_DELAYS: [Duration; 2] = [Duration::from_millis(100), Duration::from_secs(1)];

/// The shortest time between two attempts of the operations to open a rewritten database that
/// failed to open.
const REOPEN_INTERVAL: Duration = Duration::from_mins(1);

/// What keeps the open store's files current, and what shows a failure of the store.
#[derive(Clone, Default)]
struct Upkeep {
    flusher: Arc<Mutex<Option<Flusher>>>,
    /// Why the store isn't open, when its rewritten database fails to open.
    failure: Arc<Mutex<Option<Arc<StoreError>>>>,
}

impl Upkeep {
    /// Puts what an attempt to open a rewritten database left in `slot`.
    fn settle(
        &self,
        slot: &mut Slot,
        reopened: Result<Inner, Box<Unopened>>,
    ) -> Result<(), StoreError> {
        match reopened {
            Ok(inner) => {
                self.start_flusher(&inner);
                *self.failure.lock() = None;
                *slot = Slot::Open(Arc::new(inner));
                Ok(())
            }
            Err(mut unopened) => {
                unopened.retry_at = Instant::now() + REOPEN_INTERVAL;
                *self.failure.lock() = Some(Arc::clone(&unopened.error));
                *slot = Slot::Unopened(unopened);
                Err(slot.unavailable())
            }
        }
    }

    fn start_flusher(&self, inner: &Inner) {
        let flusher = Flusher::spawn(Arc::downgrade(&inner.keyspaces), FLUSHER_NAME);
        *self.flusher.lock() = Some(flusher);
    }

    async fn stop_flusher(&self) {
        let flusher = self.flusher.lock().take();
        if let Some(flusher) = flusher {
            flusher.stop().await;
        }
    }
}

/// The records of desktop capture, in fjall.
#[derive(Clone, derive_more::Debug)]
pub struct DesktopStore {
    /// Each operation holds a read guard while it runs, so that [`close`](Self::close) and
    /// [`rewrite`](Self::rewrite) wait for the operations in flight.
    #[debug(skip)]
    inner: Arc<RwLock<Slot>>,
    #[debug(skip)]
    upkeep: Upkeep,
    #[debug(skip)]
    usage: Arc<Usage>,
}

impl DesktopStore {
    /// Opens the store in `dir`, which stores every new record as a record of `host`.
    ///
    /// Only the current user can open the directory, because the store holds what capture read
    /// from every window. The open finishes a rewrite that the store owes, and logs a failure of
    /// that rewrite. The open fails when the store holds a format other than this daemon's.
    pub async fn open(dir: impl Into<PathBuf>, host: HostId) -> Result<Self, StoreError> {
        let dir = dir.into();
        atuin_common::os::private::ensure_private_dir(&dir).await.map_err(StoreError::Directory)?;
        let inner = tokio::task::spawn_blocking(move || {
            let dir = StoreDir::lock(dir)?;
            let keyspaces = Keyspaces::open(dir.database(), host)?;
            Ok::<_, StoreError>(Inner {
                keyspaces: Arc::new(keyspaces),
                open: OpenSegments::default(),
                evictor: Evictor::default(),
                dir,
            })
        })
        .await
        .expect("a desktop capture store task panicked")?;
        let usage = inner.keyspaces.usage();
        let upkeep = Upkeep::default();
        upkeep.start_flusher(&inner);
        let store = Self {
            inner: Arc::new(RwLock::new(Slot::Open(Arc::new(inner)))),
            upkeep,
            usage,
        };
        if let Err(err) = store.rewrite_if_owed().await {
            warn!(
                ?err,
                "could not finish an owed rewrite of the desktop capture store, so the store's \
                 files can still hold removed records"
            );
        }
        Ok(store)
    }

    /// Why the store fails every operation, when its rewritten database fails to open.
    pub fn failure(&self) -> Option<Arc<StoreError>> {
        self.upkeep.failure.lock().clone()
    }

    /// Makes every record that the store writes from now on follow `id` in key order.
    ///
    /// The search index reconciles only the records above its watermark. When the newest
    /// records were deleted and the clock was set back, a new id could fall below the
    /// watermark and never be reconciled.
    pub async fn write_after(&self, id: Uuid) -> Result<(), StoreError> {
        self.blocking(move |inner| {
            inner.keyspaces.follow(id);
            Ok(())
        })
        .await
    }

    /// Persists every write and closes the database, so that the directory can be opened again
    /// as soon as this returns.
    ///
    /// Close waits for the operations in flight, in every clone of the store. Later
    /// operations fail with [`StoreError::Closed`].
    pub async fn close(&self) -> Result<(), StoreError> {
        let slot = std::mem::replace(&mut *self.inner.write().await, Slot::Closed);
        let Slot::Open(inner) = slot else {
            return Ok(());
        };
        self.upkeep.stop_flusher().await;
        // No operation and no flusher holds the store now, so this is the last reference, and
        // dropping it at the end of the task closes the database.
        tokio::task::spawn_blocking(move || inner.keyspaces.persist())
            .await
            .expect("a desktop capture store task panicked")?;
        Ok(())
    }

    async fn blocking<T, F>(&self, work: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Inner) -> Result<T, StoreError> + Send + 'static,
    {
        let inner = self.open_inner().await?;
        tokio::task::spawn_blocking(move || work(&inner))
            .await
            .expect("a desktop capture store task panicked")
    }

    /// A read guard of the open store.
    ///
    /// When the store's rewritten database failed to open, the call first tries to open the
    /// database again, at most once per [`REOPEN_INTERVAL`].
    async fn open_inner(&self) -> Result<OwnedRwLockReadGuard<Slot, Inner>, StoreError> {
        let guard = Arc::clone(&self.inner).read_owned().await;
        let guard = match OwnedRwLockReadGuard::try_map(guard, Slot::open) {
            Ok(inner) => return Ok(inner),
            Err(guard) => guard,
        };
        if !matches!(*guard, Slot::Unopened(_)) {
            return Err(guard.unavailable());
        }
        drop(guard);
        let slot = Arc::clone(&self.inner).write_owned().await;
        // The attempt runs in a task of its own, so that a caller that stops waiting can't
        // leave the slot empty.
        let upkeep = self.upkeep.clone();
        let reopen = tokio::spawn(async move {
            let mut slot = slot;
            let Slot::Unopened(unopened) = &*slot else {
                return slot;
            };
            if Instant::now() < unopened.retry_at {
                return slot;
            }
            let Slot::Unopened(unopened) = std::mem::replace(&mut *slot, Slot::Closed) else {
                unreachable!("the slot holds an unopened store");
            };
            let reopened = tokio::task::spawn_blocking(move || unopened.reopen())
                .await
                .expect("a desktop capture store task panicked");
            if upkeep.settle(&mut slot, reopened).is_ok() {
                info!("opened the rewritten desktop capture store");
            }
            slot
        });
        let slot = reopen.await.expect("a desktop capture store reopen panicked");
        OwnedRwLockReadGuard::try_map(OwnedRwLockWriteGuard::downgrade(slot), Slot::open)
            .map_err(|guard| guard.unavailable())
    }

    /// Writes one record from a helper run. `run` holds the helper's segment ids for this run, so
    /// pass the same `RunState` for every record of the run.
    ///
    /// When the store can't write the start of an active segment, the segment's next record
    /// tries the start again. The records before a start that succeeds aren't stored.
    pub async fn write(&self, run: &mut RunState, record: Record) -> Result<Written, StoreError> {
        let nothing_new = Written { item: None };
        match record {
            Record::SegmentStarted(segment) => {
                let ActiveStart { id, start } = segment.into();
                self.start(run, id, start).await?;
                Ok(nothing_new)
            }
            Record::Snapshot(snapshot) => {
                if let Some(unstarted) = run.take_unstarted(snapshot.segment) {
                    self.start(run, snapshot.segment, unstarted).await?;
                }
                let segment = run.segment(snapshot.segment)?;
                self.observe(segment, snapshot.into()).await
            }
            Record::SegmentEnded { segment, at } => {
                match run.take_segment(segment)? {
                    RunSegment::Open(segment) => self.end(segment.key, at).await?,
                    // The store holds nothing of the segment.
                    RunSegment::Unstarted(_) => {}
                }
                Ok(nothing_new)
            }
            Record::Dialog(dialog) => self.write_one_shot(FirstObservation::from(dialog)).await,
            Record::Notification(notification) => {
                self.write_one_shot(FirstObservation::from(notification)).await
            }
            Record::Background(read) => {
                let window = read.window;
                let first = FirstObservation::from(read);
                let written = match run.background_step(window, first.start(), first.at()) {
                    BackgroundStep::Continue(segment) => {
                        self.observe(segment, first.into_observation()).await
                    }
                    BackgroundStep::Start { old } => {
                        let started = self
                            .blocking(move |inner| {
                                write::start_background(&inner.keyspaces, &inner.open, old, first)
                            })
                            .await?;
                        Ok(Written {
                            item: run.started_background(window, started),
                        })
                    }
                };
                run.trim_background_trees();
                written
            }
        }
    }

    async fn write_one_shot(&self, first: FirstObservation) -> Result<Written, StoreError> {
        let item = self.blocking(move |inner| write::one_shot(&inner.keyspaces, first));
        Ok(Written { item: item.await? })
    }

    /// Starts the active segment `id` of `run`. When the write fails, `run` keeps the start for
    /// the segment's next record.
    async fn start(
        &self,
        run: &mut RunState,
        id: SegmentId,
        unstarted: Unstarted,
    ) -> Result<(), StoreError> {
        let started = self.blocking(move |inner| {
            let started =
                write::start(&inner.keyspaces, &inner.open, unstarted.at, &unstarted.start);
            Ok(match started {
                Ok(segment) => Start::Started(segment),
                Err(source) => Start::Failed { source, unstarted },
            })
        });
        match started.await? {
            Start::Started(segment) => {
                run.add_segment(id, RunSegment::Open(segment));
                Ok(())
            }
            Start::Failed { source, unstarted } => {
                run.add_segment(id, RunSegment::Unstarted(unstarted));
                Err(source)
            }
        }
    }

    async fn end(&self, key: SegmentKey, at: SystemTime) -> Result<(), StoreError> {
        self.blocking(move |inner| write::end(&inner.keyspaces, &inner.open, key, at)).await
    }

    async fn observe(
        &self,
        segment: &mut OpenSegment,
        observation: Observation,
    ) -> Result<Written, StoreError> {
        let pending = segment.pending(observation);
        let observed = self.blocking(move |inner| pending.store(&inner.keyspaces)).await?;
        Ok(Written {
            item: segment.observed(observed),
        })
    }

    /// Ends the background segments of `run` that went without a write for
    /// [`MAX_BACKGROUND_AGE`](run::MAX_BACKGROUND_AGE) by `now`.
    ///
    /// The end of each segment is recorded at [`MAX_BACKGROUND_AGE`](run::MAX_BACKGROUND_AGE) after
    /// the segment's last write. A window whose content stops changing doesn't get any more reads,
    /// so its segment would otherwise stay open, and kept from eviction, until the run ends. The
    /// next read of such a window starts a new segment. A segment whose end fails to write is ended
    /// when the run ends.
    pub async fn end_idle_background(
        &self,
        run: &mut RunState,
        now: SystemTime,
    ) -> Result<(), StoreError> {
        let idle = run.take_idle_background(now);
        self.end_background_segments(idle).await
    }

    /// Ends every background segment of `run` at `now`. The next read of a window starts a new
    /// segment.
    pub async fn end_background(
        &self,
        run: &mut RunState,
        now: SystemTime,
    ) -> Result<(), StoreError> {
        let ends = run.take_background(now);
        self.end_background_segments(ends).await
    }

    async fn end_background_segments(&self, ends: Vec<BackgroundEnd>) -> Result<(), StoreError> {
        if ends.is_empty() {
            return Ok(());
        }
        self.blocking(move |inner| write::end_background(&inner.keyspaces, &inner.open, &ends))
            .await
    }

    /// Ends every open segment at its last record, including those that an earlier process left
    /// open.
    ///
    /// Call it when no helper run is writing, before a new run starts.
    pub async fn close_open_segments(&self) -> Result<(), StoreError> {
        self.blocking(|inner| write::close_open_segments(&inner.keyspaces, &inner.open)).await
    }

    /// The segments that overlap the filter's time range, oldest first.
    ///
    /// A segment that an earlier process left open counts as ended at its last record.
    pub async fn segments(&self, filter: SegmentFilter) -> Result<Vec<SegmentSummary>, StoreError> {
        self.blocking(move |inner| inner.view().segments(&filter)).await
    }

    /// The newest `limit` segments, by their start, that overlap the filter's time range,
    /// oldest first.
    ///
    /// The store reads segments from the newest back, and stops once it has found `limit`.
    pub async fn newest_segments(
        &self,
        filter: SegmentFilter,
        limit: usize,
    ) -> Result<Vec<SegmentSummary>, StoreError> {
        self.blocking(move |inner| inner.view().newest_segments(&filter, limit)).await
    }

    /// The active segments, dialogs, and notifications between `since` and `now`, oldest first.
    /// The text of each dialog is cut to `dialog_text_bytes`.
    pub async fn recent_activity(
        &self,
        since: SystemTime,
        now: SystemTime,
        dialog_text_bytes: usize,
    ) -> Result<RecentActivity, StoreError> {
        self.blocking(move |inner| inner.view().recent_activity(since, now, dialog_text_bytes))
            .await
    }

    /// Reads one snapshot of a segment. `None` when the segment or the snapshot is unknown.
    #[cfg(test)]
    pub async fn read(
        &self,
        segment: SegmentKey,
        which: Which,
    ) -> Result<Option<SnapshotRead>, StoreError> {
        self.blocking(move |inner| inner.view().read(segment, which)).await
    }

    /// Reads one snapshot as text of at most `max_bytes`. `id` names a segment or one of its
    /// snapshots, and `which` chooses among the snapshots of that segment. Without `which`, the
    /// read takes the snapshot that `id` names, or the latest snapshot when `id` names a segment.
    /// `None` when the segment or the snapshot is unknown.
    pub async fn read_text(
        &self,
        id: Uuid,
        which: Option<Which>,
        max_bytes: usize,
    ) -> Result<Option<SnapshotText>, StoreError> {
        self.blocking(move |inner| inner.view().read_text_by_id(id, which, max_bytes)).await
    }

    /// Reads the snapshots `records` of the segment that `id` names, as texts within `budget`,
    /// in the order of the segment's snapshots. `id` names the segment or one of its snapshots.
    ///
    /// A record that isn't a snapshot of the segment is left out. `None` when the segment is
    /// unknown.
    pub async fn read_snapshots(
        &self,
        id: Uuid,
        records: HashSet<Uuid>,
        budget: TextBudget,
    ) -> Result<Option<RecordTexts>, StoreError> {
        self.blocking(move |inner| inner.view().read_snapshots(id, &records, budget)).await
    }

    /// Lists the snapshots of the segment that `id` names: the segment, or one of its snapshots.
    /// `None` when the segment is unknown.
    pub async fn snapshots(&self, id: Uuid) -> Result<Option<SegmentSnapshots>, StoreError> {
        self.blocking(move |inner| inner.view().snapshots(id)).await
    }

    /// The totals of the segments that overlap the filter's time range, per application. A
    /// segment that is still open lasts until the end of the range.
    pub async fn tally(&self, filter: SegmentFilter) -> Result<Tally, StoreError> {
        self.blocking(move |inner| inner.view().tally(&filter)).await
    }

    /// The next batch of a reconciliation pass: the contents of the `Observed` records among
    /// the next `limit` records after the pass's watermark, each content once per pass.
    ///
    /// The batch lists the contents without expanding them. Expand the ones that the search
    /// index lacks with [`expand_candidates`](Self::expand_candidates).
    pub async fn reconcile_batch(&self, pass: Pass, limit: usize) -> Result<Batch, StoreError> {
        self.blocking(move |inner| pass.next_batch(&inner.keyspaces.read(), limit)).await
    }

    /// The items of the search index for `wanted`, which are candidates of the pass's last
    /// batch.
    ///
    /// A content whose stored tree is damaged is logged and left out.
    pub async fn expand_candidates(
        &self,
        mut pass: Pass,
        wanted: Vec<ContentKey>,
    ) -> Result<Expansion, StoreError> {
        self.blocking(move |inner| {
            let items = pass.expand(&inner.keyspaces.read(), wanted)?;
            Ok(Expansion { items, pass })
        })
        .await
    }

    /// For each search hit of `keys`, in order, the last record that observed the hit's
    /// content, when the hit's segment passes `filter` and overlaps the filter's time range.
    ///
    /// The store judges every hit in one snapshot. A hit that the store can't read gets its own
    /// error.
    pub async fn admit_all(
        &self,
        keys: Vec<ContentKey>,
        filter: Arc<SegmentFilter>,
    ) -> Result<Vec<Admission>, StoreError> {
        self.blocking(move |inner| {
            let view = inner.view();
            let admit = |key| Admission {
                key,
                matched: view.admit(key, &filter),
            };
            Ok(keys.into_iter().map(admit).collect())
        })
        .await
    }

    /// The content of a search hit that [`admit_all`](Self::admit_all) found.
    pub async fn matched_content(
        &self,
        key: ContentKey,
        found: MatchedRecord,
    ) -> Result<MatchedContent, StoreError> {
        self.blocking(move |inner| inner.view().matched_content(key, found)).await
    }

    /// How many segments and records the store holds.
    ///
    /// The record count is fjall's estimate.
    pub async fn counts(&self) -> Result<Counts, StoreError> {
        self.blocking(|inner| {
            Ok(Counts {
                segments: inner.keyspaces.segment_count(),
                records: inner.keyspaces.approximate_record_count(),
            })
        })
        .await
    }

    /// The bytes that the store uses, by its running estimate.
    pub fn estimated_disk_space(&self) -> u64 {
        self.usage.get()
    }

    /// Deletes the oldest ended segments until they add up to at least `bytes`, or until none is
    /// left.
    ///
    /// A segment that is still open is kept. Each segment is deleted whole. A damaged record is
    /// deleted as well.
    pub async fn evict(&self, bytes: u64) -> Result<Evicted, StoreError> {
        self.blocking(move |inner| inner.evictor.evict(&inner.keyspaces, bytes)).await
    }

    /// Deletes the segments that `selection` selects.
    ///
    /// A filter removes every segment that [`segments`](Self::segments) would list for it,
    /// whole. The filter also removes each damaged segment or record that it can select, unless
    /// the damage hides whether the filter selects it. [`Deleted::undecided`] names what stays
    /// for that reason. A segment key removes that one segment, and a key of a segment that is
    /// already gone doesn't delete anything. A failure carries what the deletion removed before
    /// it.
    ///
    /// The caller guarantees that no helper run is writing.
    pub async fn delete(&self, selection: impl Into<Selection>) -> Result<Deleted, DeleteFailed> {
        let selection = selection.into();
        let closed = |source| DeleteFailed {
            deleted: Box::default(),
            source,
        };
        self.blocking(move |inner| Ok(delete::delete(&inner.keyspaces, &inner.open, &selection)))
            .await
            .map_err(closed)?
    }

    /// The segment that the record `id` belongs to, or `None` when the store doesn't hold the
    /// record.
    pub async fn lookup(&self, id: Uuid) -> Result<Option<SegmentSummary>, StoreError> {
        self.blocking(move |inner| inner.view().lookup(id)).await
    }

    /// The key of the segment that the record `id` belongs to, or `None` when the store doesn't
    /// hold the record or its segment.
    ///
    /// Unlike [`lookup`](Self::lookup), this reads one record, however long the segment is.
    pub async fn segment_of(&self, id: Uuid) -> Result<Option<SegmentKey>, StoreError> {
        self.blocking(move |inner| inner.view().segment_of(id)).await
    }

    /// Replaces the store's files with a fresh copy of the records that the store holds.
    ///
    /// The copy reclaims the space of removed records, and the store's files don't keep any trace
    /// of those records. Every other operation waits while the rewrite runs. A rewrite that fails
    /// keeps the store as it was, unless the failure comes after the copy has replaced the old
    /// files. In that case the rewrite tries to open the copy a few more times. As long as the copy
    /// fails to open, later operations fail with [`StoreError::Reopen`], [`failure`](Self::failure)
    /// reports the error, and the operations try to open the copy again from time to time. A
    /// rewrite that fails to remove the old files after the copy replaced them reports that
    /// failure, and [`rewrite_if_owed`](Self::rewrite_if_owed) tries the removal again. A rewrite
    /// that has begun runs to its end even when the caller stops waiting.
    pub async fn rewrite(&self) -> Result<(), StoreError> {
        let slot = Arc::clone(&self.inner).write_owned().await;
        let upkeep = self.upkeep.clone();
        let rewrite = tokio::spawn(async move {
            let mut slot = slot;
            let inner = match std::mem::replace(&mut *slot, Slot::Closed) {
                Slot::Open(inner) => inner,
                other => {
                    *slot = other;
                    return Err(slot.unavailable());
                }
            };
            // The flusher holds the keyspaces during a persist, so it stops before the rewrite
            // takes the keyspaces over.
            upkeep.stop_flusher().await;
            let outcome = tokio::task::spawn_blocking(move || rewrite_inner(inner))
                .await
                .expect("a desktop capture store task panicked");
            match outcome {
                RewriteOutcome::Open { inner, result } => {
                    upkeep.settle(&mut slot, Ok(inner))?;
                    result
                }
                RewriteOutcome::Unopened(unopened) => {
                    let reopened = reopen_with_delays(unopened).await;
                    upkeep.settle(&mut slot, reopened)
                }
            }
        });
        rewrite.await.expect("a desktop capture store rewrite panicked")
    }

    /// Rewrites the store when a removal noted that the store owes a rewrite, and removes the
    /// old generations that an earlier rewrite failed to remove.
    ///
    /// A removal notes the rewrite in its own transaction, so the store still owes the rewrite
    /// when the daemon stopped before the rewrite, or when the rewrite failed.
    pub async fn rewrite_if_owed(&self) -> Result<(), StoreError> {
        let removed = self.blocking(Inner::remove_replaced).await;
        let owed = self.blocking(|inner| inner.keyspaces.read().owes_rewrite()).await?;
        let rewritten = if owed {
            self.rewrite().await
        } else {
            Ok(())
        };
        removed.and(rewritten)
    }

    /// Whether the search index can still hold a segment that the store removed.
    ///
    /// Each removal of a segment notes the index's removal in the removal's own transaction,
    /// so the note outlives a daemon that stopped before the index followed the removal.
    pub async fn owes_index_removal(&self) -> Result<bool, StoreError> {
        self.blocking(|inner| inner.keyspaces.read().owes_index_removal()).await
    }

    /// Notes that the search index holds only the segments that the store holds.
    ///
    /// The caller guarantees that no removal from the store happens between the index's
    /// removal and this call.
    pub async fn settle_index_removal(&self) -> Result<(), StoreError> {
        self.blocking(|inner| {
            let mut writer = inner.keyspaces.write()?;
            writer.settle_index_removal();
            writer.commit()
        })
        .await
    }

    /// The segments of `segments` that the store doesn't hold.
    pub async fn missing_segments<I>(&self, segments: I) -> Result<Vec<SegmentKey>, StoreError>
    where
        I: IntoIterator<Item = SegmentKey> + Send + 'static,
    {
        self.blocking(move |inner| {
            let reader = inner.keyspaces.read();
            let mut missing = Vec::new();
            for segment in segments {
                if !reader.has_segment(segment)? {
                    missing.push(segment);
                }
            }
            Ok(missing)
        })
        .await
    }

    /// The bytes that evictions freed since the last rewrite of the store.
    pub async fn evicted_since_rewrite(&self) -> Result<u64, StoreError> {
        self.blocking(|inner| inner.keyspaces.read().evicted()).await
    }

    /// Notes that the store owes a rewrite, which [`rewrite_if_owed`](Self::rewrite_if_owed)
    /// and the next open carry out.
    pub async fn owe_rewrite(&self) -> Result<(), StoreError> {
        self.blocking(|inner| {
            let mut writer = inner.keyspaces.write()?;
            writer.owe_rewrite();
            writer.commit()
        })
        .await
    }
}

/// What a rewrite of the store left.
enum RewriteOutcome {
    /// The store to use from now on, which is the old store when the rewrite failed before its
    /// commit.
    Open {
        inner: Inner,
        result: Result<(), StoreError>,
    },
    Unopened(Box<Unopened>),
}

fn rewrite_inner(inner: Arc<Inner>) -> RewriteOutcome {
    let Ok(Inner {
        keyspaces,
        open,
        evictor,
        mut dir,
    }) = Arc::try_unwrap(inner)
    else {
        unreachable!("an operation holds the store while the store is locked for writing");
    };
    let Ok(keyspaces) = Arc::try_unwrap(keyspaces) else {
        unreachable!("an operation or the flusher holds the keyspaces during a rewrite");
    };
    let (keyspaces, result) = match rewrite::rewrite(keyspaces, &mut dir) {
        Rewritten::Done { keyspaces, removal } => (keyspaces, removal),
        Rewritten::Kept { keyspaces, error } => (keyspaces, Err(error)),
        Rewritten::Unopened { carried, error } => {
            return RewriteOutcome::Unopened(Box::new(Unopened {
                carried,
                open,
                evictor,
                dir,
                error: Arc::new(error),
                retry_at: Instant::now(),
            }));
        }
    };
    RewriteOutcome::Open {
        inner: Inner {
            keyspaces: Arc::new(keyspaces),
            open,
            evictor,
            dir,
        },
        result,
    }
}

/// Tries to open a rewritten database again after each of [`REOPEN_DELAYS`].
async fn reopen_with_delays(mut unopened: Box<Unopened>) -> Result<Inner, Box<Unopened>> {
    for delay in REOPEN_DELAYS {
        tokio::time::sleep(delay).await;
        let reopened = tokio::task::spawn_blocking(move || unopened.reopen())
            .await
            .expect("a desktop capture store task panicked");
        match reopened {
            Ok(inner) => return Ok(inner),
            Err(failed) => unopened = failed,
        }
    }
    Err(unopened)
}

#[cfg(test)]
impl DesktopStore {
    fn keyspaces(&self) -> Arc<Keyspaces> {
        let slot = self.inner.try_read().expect("no store is closing in a test");
        Arc::clone(&slot.open().expect("the store is open").keyspaces)
    }

    /// Makes the next `count` opens of a rewritten database fail.
    pub fn fail_reopens(&self, count: u32) {
        self.keyspaces().fail_reopens(count);
    }

    /// How many stored contents the store has expanded since it opened.
    pub fn expansions(&self) -> usize {
        self.keyspaces().expansions()
    }

    /// Writes `value` under the record key `id`, in place of whatever the store holds there.
    pub fn put_raw_record(&self, id: Uuid, value: &[u8]) {
        self.keyspaces().put_raw_record(id, value);
    }

    /// Writes `value` as the store's format.
    pub fn put_raw_format(&self, value: &[u8]) {
        self.keyspaces().put_raw_format(value);
    }

    /// The format that the store notes.
    pub fn format(&self) -> Option<u32> {
        self.keyspaces().read().format().expect("read the format")
    }

    /// Adds `key` to the segment records.
    pub fn put_raw_segment_record_key(&self, key: &[u8]) {
        self.keyspaces().put_raw_segment_record_key(key);
    }

    /// Writes `value` under the content key `key`, in place of whatever the store holds there.
    pub fn put_raw_content(&self, key: ContentKey, value: &[u8]) {
        self.keyspaces().put_raw_content(key, value);
    }

    /// Adds `bytes` to the bytes that evictions freed since the last rewrite.
    pub fn add_evicted(&self, bytes: u64) {
        let keyspaces = self.keyspaces();
        let mut writer = keyspaces.write().expect("a transaction");
        writer.add_evicted(bytes).expect("read the evicted bytes");
        writer.commit().expect("a commit");
    }
}
