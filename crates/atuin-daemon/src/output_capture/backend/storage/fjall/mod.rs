//! Durable storage for captured command output.
//!
//! Output is keyed by `history_id` (16 UUID bytes) in a fjall keyspace; the
//! value is the encoded `history::CommandCapture`. Each write runs in an
//! optimistic transaction so the check-then-insert is atomic against concurrent
//! writers, and all blocking fjall I/O runs on tokio's blocking pool via
//! `spawn_blocking`.
mod schema;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use atuin_client::history::{CommandCapture, HistoryId};
use fjall::{OptimisticTxDatabase, OptimisticTxKeyspace, PersistMode, Readable};
use schema::{Schema as _, SchemaV2};
use tokio::task::JoinHandle;
use tracing::error;

use super::{CaptureError, DeleteOutputError, GetOutputError, Storage};

/// The schema currently in use for stored output.
type ActiveSchema = SchemaV2;

/// The store and every operation on it.
///
/// This structure is shared between the [`FjallStorage`] and the task in [`Flusher`].
struct FjallStorageInner {
    db: OptimisticTxDatabase,
    keyspace: OptimisticTxKeyspace,
    /// Set on every mutation; the flusher clears it and persists. See `Flusher` for the
    /// memory-ordering rationale.
    dirty: Arc<AtomicBool>,
}

impl FjallStorageInner {
    /// On-disk bytes used by the store's segments and blob files.
    ///
    /// Might over/under-report by a couple dozen MB.
    fn estimated_disk_space(&self) -> u64 {
        self.keyspace.inner().disk_space()
    }

    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError> {
        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        let dirty = self.dirty.clone();
        let key = ActiveSchema::serialize_key(id).expect("history id serialization is infallible");
        let value = ActiveSchema::serialize_value(capture)
            .map_err(|err| CaptureError::Serialize(Box::new(err)))?;

        tokio::task::spawn_blocking(move || {
            let mut tx = db.write_tx().map_err(|err| CaptureError::Storage(Box::new(err)))?;
            if tx
                .contains_key(&keyspace, key)
                .map_err(|err| CaptureError::Storage(Box::new(err)))?
            {
                return Err(CaptureError::AlreadyExists);
            }

            tx.insert(&keyspace, key, value);
            match tx.commit().map_err(|err| CaptureError::Storage(Box::new(err)))? {
                Ok(()) => {
                    dirty.store(true, Ordering::Release);
                    Ok(())
                }
                // Another writer committed this key first, so it's already captured.
                Err(fjall::Conflict) => Err(CaptureError::AlreadyExists),
            }
        })
        .await
        .expect("output-capture write task panicked")
    }

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        let keyspace = self.keyspace.clone();
        let key = ActiveSchema::serialize_key(id).expect("history id serialization is infallible");

        tokio::task::spawn_blocking(move || {
            match keyspace.get(key).map_err(|err| GetOutputError::Storage(Box::new(err)))? {
                Some(slice) => {
                    let capture = ActiveSchema::deserialize_value(slice.to_vec())
                        .expect("stored value is a valid CommandCapture");
                    Ok(Some(capture))
                }
                None => Ok(None),
            }
        })
        .await
        .expect("output-capture read task panicked")
    }

    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), DeleteOutputError> {
        let keys: Vec<_> = ids
            .map(|id| {
                ActiveSchema::serialize_key(id).expect("history id serialization is infallible")
            })
            .collect();
        if keys.is_empty() {
            return Ok(());
        }

        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        let dirty = self.dirty.clone();
        tokio::task::spawn_blocking(move || {
            let mut tx = db.write_tx().map_err(|err| DeleteOutputError::Storage(Box::new(err)))?;
            for key in keys {
                tx.remove(&keyspace, key);
            }
            match tx.commit().map_err(|err| DeleteOutputError::Storage(Box::new(err)))? {
                Ok(()) => {
                    dirty.store(true, Ordering::Release);
                    Ok(())
                }
                // fjall only reports conflicts for transactions that read; this one never does.
                Err(fjall::Conflict) => {
                    unreachable!("a blind remove performs no reads, so it can never conflict")
                }
            }
        })
        .await
        .expect("output-capture delete task panicked")
    }

    /// Every stored id, oldest first (fjall key order). Reads keys only.
    async fn all_ids(&self) -> Result<Vec<HistoryId>, GetOutputError> {
        let keyspace = self.keyspace.clone();
        tokio::task::spawn_blocking(move || {
            let mut ids = Vec::new();
            for guard in keyspace.inner().iter() {
                // Read the key only; the value (a KV-separated blob) stays on disk.
                let key = guard.key().map_err(|err| GetOutputError::Storage(Box::new(err)))?;
                let id = ActiveSchema::deserialize_key(key.as_ref())
                    .map_err(|err| GetOutputError::Storage(Box::new(err)))?;
                ids.push(id);
            }
            Ok(ids)
        })
        .await
        .expect("output-capture scan task panicked")
    }

    /// The oldest ids whose values total at least `reclaim_bytes` (or all of them, if the store
    /// holds less). Reads values to measure their size, oldest first, and stops early; it does not
    /// delete -- eviction goes back through the backend's `remove` so the search index stays in sync.
    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        if reclaim_bytes == 0 {
            return Ok(Vec::new());
        }

        let keyspace = self.keyspace.clone();
        tokio::task::spawn_blocking(move || {
            let mut ids = Vec::new();
            let mut freed: u64 = 0;
            for guard in keyspace.inner().iter() {
                let (key, value) =
                    guard.into_inner().map_err(|err| DeleteOutputError::Storage(Box::new(err)))?;
                let id = ActiveSchema::deserialize_key(key.as_ref())
                    .map_err(|err| DeleteOutputError::Storage(Box::new(err)))?;
                ids.push(id);
                freed = freed.saturating_add(u64::try_from(value.len()).unwrap_or(u64::MAX));
                if freed >= reclaim_bytes {
                    break;
                }
            }
            Ok(ids)
        })
        .await
        .expect("output-capture eviction-candidates task panicked")
    }
}

/// Task responsible for flushing fjall data buffered in memory onto the disk.
#[derive(Debug)]
struct Flusher {
    /// Handle to the background task.
    task: JoinHandle<()>,
}

impl Flusher {
    /// How often to try to flush.
    ///
    /// We'd expect flush itself to take anywhere between 1-10ms, so this is plenty of overhead.
    const SYNC_INTERVAL: Duration = Duration::from_secs(5);

    pub fn spawn(inner: Arc<FjallStorageInner>) -> Self {
        let task = tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Self::SYNC_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;

                // TODO(markovejnovic): @taylordotfish and I were wondering whether it is possible
                // to use relaxed here. @taylordotfish claims that's not possible and I am more and
                // more convinced by her argument.
                //
                // The concern is that one thread may perform some writes
                //
                // db-write
                // db-write
                // dirty-set
                //
                // while another thread does
                //
                // dirty-load
                // persist
                //
                // In the pathological case, the db writes can be re-ordered after the dirty-set
                // (under relaxed semantics):
                //
                // dirty-set
                // db-write
                // db-write
                //
                // while the other thread does
                //
                // dirty-load
                // persist
                //
                // Well the persist won't observe those db-writes.
                //
                // The counter-argument is that both db-write and persist acquire the same mutex,
                // so must be seq-cst-ordered?
                //
                // Unsure but would be curious to learn more.
                //
                // @taylordotfish mentioned we shouldn't rely on the internal implementation
                // details.
                if !inner.dirty.swap(false, Ordering::Acquire) {
                    continue;
                }

                let db = inner.db.clone();
                if let Err(err) =
                    tokio::task::spawn_blocking(move || db.persist(PersistMode::SyncAll))
                        .await
                        .expect("persistence task shouldn't panic")
                {
                    error!(?err, "failed to persist data on disk. will try again...");
                    inner.dirty.store(true, Ordering::Relaxed);
                }
            }
        });

        Self { task }
    }
}

impl Drop for Flusher {
    fn drop(&mut self) {
        // Stop the background loop once nothing is holding the flusher any more.
        self.task.abort();
    }
}

#[derive(Clone, derive_more::Debug)]
pub struct FjallStorage {
    #[debug(skip)]
    inner: Arc<FjallStorageInner>,
    #[debug(skip)]
    _flusher: Arc<Flusher>,
}

impl FjallStorage {
    /// Open the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> fjall::Result<Self> {
        let db = OptimisticTxDatabase::builder(path.as_ref()).open()?;
        Self::new(db)
    }

    pub fn new(db: OptimisticTxDatabase) -> fjall::Result<Self> {
        let keyspace = db.keyspace(ActiveSchema::NAME, ActiveSchema::create_options)?;
        let inner = Arc::new(FjallStorageInner {
            db,
            keyspace,
            dirty: Arc::new(AtomicBool::new(false)),
        });

        let flusher = Arc::new(Flusher::spawn(inner.clone()));

        Ok(Self {
            inner,
            _flusher: flusher,
        })
    }
}

impl Storage for FjallStorage {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError> {
        self.inner.capture(id, capture).await
    }

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        self.inner.get(id).await
    }

    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), DeleteOutputError> {
        self.inner.remove(ids).await
    }

    fn estimated_disk_space(&self) -> u64 {
        self.inner.estimated_disk_space()
    }

    async fn all_ids(&self) -> Result<Vec<HistoryId>, GetOutputError> {
        self.inner.all_ids().await
    }

    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        self.inner.eviction_candidates(reclaim_bytes).await
    }
}

#[cfg(test)]
mod tests {
    use easy_cast::Conv;
    use uuid::Uuid;

    use super::*;

    fn temp_storage() -> (FjallStorage, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FjallStorage::open(dir.path()).expect("open");
        (backend, dir)
    }

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*Uuid::from_u128(n).as_bytes())
    }

    fn cap(output: &str) -> CommandCapture {
        CommandCapture {
            output_start: output.to_string(),
            output_end: None,
            output_observed_bytes: u64::conv(output.len()),
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    #[tokio::test]
    async fn round_trips_output_by_history_id() {
        let (store, _dir) = temp_storage();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        let got = store.get(hid(1)).await.expect("get").expect("present");
        assert_eq!(got.output_start, "hello");
        assert_eq!(got.output_observed_bytes, 5);
    }

    /// A capture whose middle was discarded: the v2 schema stores the two halves separately, so
    /// the optional tail has to survive a round trip as its own field.
    fn split_cap(start: &str, end: &str, observed: u64) -> CommandCapture {
        CommandCapture {
            output_start: start.to_string(),
            output_end: Some(end.to_string()),
            output_observed_bytes: observed,
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    #[tokio::test]
    async fn round_trips_a_capture_that_lost_its_middle() {
        let (store, _dir) = temp_storage();
        let capture = split_cap("first lines", "last lines", 10_000);
        store.capture(hid(1), capture.clone()).await.expect("capture");

        let got = store.get(hid(1)).await.expect("get").expect("present");
        assert_eq!(got, capture);
        // The tail is what distinguishes a split capture from a whole one, so it must come back
        // as `Some` and not be folded into the start.
        assert_eq!(got.output_end.as_deref(), Some("last lines"));
        assert_eq!(got.output_observed_bytes, 10_000, "the observed count is not the kept count");
    }

    #[tokio::test]
    async fn an_empty_tail_is_not_the_same_as_no_tail() {
        // `Some("")` means "everything after the start was discarded"; `None` means "nothing was".
        // Collapsing the two would lose the only signal that a capture is incomplete.
        let (store, _dir) = temp_storage();
        store.capture(hid(1), split_cap("kept", "", 500)).await.expect("capture");
        store.capture(hid(2), cap("kept")).await.expect("capture");

        let split = store.get(hid(1)).await.expect("get").expect("present");
        let whole = store.get(hid(2)).await.expect("get").expect("present");
        assert_eq!(split.output_end.as_deref(), Some(""));
        assert_eq!(whole.output_end, None);
    }

    #[tokio::test]
    async fn missing_id_returns_none() {
        let (store, _dir) = temp_storage();
        assert!(store.get(hid(9)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn second_capture_for_same_id_is_rejected() {
        let (store, _dir) = temp_storage();
        store.capture(hid(1), cap("first")).await.expect("first");
        let err = store.capture(hid(1), cap("second")).await.unwrap_err();
        assert!(matches!(err, CaptureError::AlreadyExists));
        // The first write survives.
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "first");
    }

    #[tokio::test]
    async fn concurrent_writers_store_exactly_one() {
        let (store, _dir) = temp_storage();
        let store = std::sync::Arc::new(store);
        let mut handles = Vec::new();
        for n in 0..16u8 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                store.capture(hid(1), cap(&format!("w{n}"))).await
            }));
        }
        let mut ok = 0;
        for h in handles {
            if h.await.expect("join").is_ok() {
                ok += 1;
            }
        }
        assert_eq!(ok, 1, "exactly one writer wins, no TOCTOU double-store");
    }

    #[tokio::test]
    async fn remove_removes_stored_output() {
        let (store, _dir) = temp_storage();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.remove([hid(1)].into_iter()).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_of_absent_ids_is_ok() {
        let (store, _dir) = temp_storage();
        store.remove(std::iter::empty()).await.expect("remove of nothing is idempotent");
        store.remove([hid(9)].into_iter()).await.expect("remove of an absent id is idempotent");
    }

    #[tokio::test]
    async fn remove_only_removes_requested_ids() {
        let (store, _dir) = temp_storage();
        for n in 1..=3u128 {
            store.capture(hid(n), cap(&format!("out{n}"))).await.expect("capture");
        }
        store.remove([hid(1), hid(3), hid(9)].into_iter()).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
        assert_eq!(store.get(hid(2)).await.expect("get").expect("kept").output_start, "out2");
        assert!(store.get(hid(3)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn removed_id_can_be_captured_again() {
        let (store, _dir) = temp_storage();
        store.capture(hid(1), cap("first")).await.expect("first");
        store.remove([hid(1)].into_iter()).await.expect("remove");
        // The tombstone must free the id for the capture-once check, not merely hide the value.
        store.capture(hid(1), cap("second")).await.expect("recapture after remove");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "second");
    }

    #[tokio::test]
    async fn remove_after_removal_is_idempotent() {
        let (store, _dir) = temp_storage();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.remove([hid(1)].into_iter()).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
        // Re-removing an already-removed id alongside an absent one is still Ok.
        store.remove([hid(1), hid(9)].into_iter()).await.expect("remove again");
    }

    #[tokio::test]
    async fn eviction_candidates_names_oldest_until_budget_met() {
        let (store, _dir) = temp_storage();
        for n in 1..=3u128 {
            store.capture(hid(n), cap(&format!("out{n}"))).await.expect("capture");
        }

        // One byte of budget names exactly the oldest entry (keys sort by id), and selecting a
        // victim must not delete it -- the backend's `remove` does that, so both stores stay in sync.
        let victims = store.eviction_candidates(1).await.expect("candidates");
        assert_eq!(victims, vec![hid(1)]);
        assert!(store.get(hid(1)).await.expect("get").is_some(), "selection does not delete");

        // A budget past everything names all entries, oldest first.
        let all = store.eviction_candidates(u64::MAX).await.expect("candidates");
        assert_eq!(all, vec![hid(1), hid(2), hid(3)]);
    }

    #[tokio::test]
    async fn eviction_candidates_for_zero_bytes_is_empty() {
        let (store, _dir) = temp_storage();
        store.capture(hid(1), cap("keep")).await.expect("capture");
        assert!(store.eviction_candidates(0).await.expect("candidates").is_empty());
    }

    #[tokio::test]
    async fn all_ids_lists_every_stored_id_oldest_first() {
        let (store, _dir) = temp_storage();
        for n in 1..=3u128 {
            store.capture(hid(n), cap(&format!("out{n}"))).await.expect("capture");
        }
        assert_eq!(store.all_ids().await.expect("all_ids"), vec![hid(1), hid(2), hid(3)]);
    }
}
