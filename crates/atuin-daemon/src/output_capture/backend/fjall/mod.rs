//! Durable storage for captured command output.
//!
//! Output is keyed by `history_id` (16 UUID bytes) in a fjall keyspace; the
//! value is the encoded `history::CommandCapture`. Each write runs in an
//! optimistic transaction so the check-then-insert is atomic against concurrent
//! writers, and all blocking fjall I/O runs on tokio's blocking pool via
//! `spawn_blocking`.
//!
//! TODO(retention): the store grows unbounded; no eviction yet. See the design doc.
mod schema;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use atuin_client::history::{CommandCapture, HistoryId};
use easy_cast::Conv;
use fjall::{OptimisticTxDatabase, OptimisticTxKeyspace, PersistMode, Readable};
use schema::{Schema as _, SchemaV2};
use tokio::task::JoinHandle;
use tracing::error;

use super::{Backend, CaptureError, DeleteOutputError, GetOutputError};

/// The schema currently in use for stored output.
type ActiveSchema = SchemaV2;

/// Task responsible for flushing fjall data buffered in memory onto the disk.
#[derive(Debug)]
struct Flusher {
    /// Whether new data was inserted since the last flush.
    dirty: Arc<AtomicBool>,
    /// Handle to the background task.
    task: JoinHandle<()>,
}

impl Flusher {
    /// How often to try to flush.
    ///
    /// We'd expect flush itself to take anywhere between 1-10ms, so this is plenty of overhead.
    const SYNC_INTERVAL: Duration = Duration::from_secs(5);

    pub fn spawn(db: OptimisticTxDatabase) -> Self {
        let dirty_outer = Arc::new(AtomicBool::new(false));

        let dirty = dirty_outer.clone();
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
                if !dirty.swap(false, Ordering::Acquire) {
                    continue;
                }

                let db = db.clone();
                if let Err(err) =
                    tokio::task::spawn_blocking(move || db.persist(PersistMode::SyncAll))
                        .await
                        .expect("persistence task shouldn't panic")
                {
                    error!(?err, "failed to persist data on disk. will try again...");
                    dirty.store(true, Ordering::Relaxed);
                }
            }
        });

        Self {
            dirty: dirty_outer,
            task,
        }
    }

    /// Mark the flusher as necessary.
    ///
    /// Generally, this should be called on every mutation.
    fn kick(&self) {
        // Relaxed _should_ be OK here since fjall is handling actual memory ordering and concurrency.
        self.dirty.store(true, Ordering::Release);
    }
}

impl Drop for Flusher {
    fn drop(&mut self) {
        // Stop the background loop once nothing is holding the flusher any more.
        self.task.abort();
    }
}

#[derive(derive_more::Debug)]
pub struct FjallBackend {
    #[debug(skip)]
    db: OptimisticTxDatabase,
    #[debug(skip)]
    keyspace: OptimisticTxKeyspace,
    flusher: Arc<Flusher>,
}

impl FjallBackend {
    pub fn open(path: impl AsRef<std::path::Path>) -> fjall::Result<Self> {
        let db = OptimisticTxDatabase::builder(path.as_ref()).open()?;
        Self::new(db)
    }

    pub fn new(db: OptimisticTxDatabase) -> fjall::Result<Self> {
        Ok(Self {
            db: db.clone(),
            keyspace: db.keyspace(ActiveSchema::NAME, ActiveSchema::create_options)?,
            flusher: Arc::new(Flusher::spawn(db)),
        })
    }
}

impl Backend for FjallBackend {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError> {
        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        let key = ActiveSchema::serialize_key(id).expect("history id serialization is infallible");
        let value = ActiveSchema::serialize_value(capture)
            .map_err(|err| CaptureError::Serialize(Box::new(err)))?;

        let flusher = self.flusher.clone();
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
                    flusher.kick();
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

    async fn remove(&self, ids: Vec<HistoryId>) -> Result<(), DeleteOutputError> {
        let keys: Vec<_> = ids
            .into_iter()
            .map(|id| {
                ActiveSchema::serialize_key(id).expect("history id serialization is infallible")
            })
            .collect();
        if keys.is_empty() {
            return Ok(());
        }

        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        let flusher = self.flusher.clone();
        tokio::task::spawn_blocking(move || {
            let mut tx = db.write_tx().map_err(|err| DeleteOutputError::Storage(Box::new(err)))?;
            for key in keys {
                tx.remove(&keyspace, key);
            }
            match tx.commit().map_err(|err| DeleteOutputError::Storage(Box::new(err)))? {
                Ok(()) => {
                    flusher.kick();
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

    async fn stats(&self) -> Result<Option<super::OutputCaptureStats>, GetOutputError> {
        let keyspace = self.keyspace.clone();
        tokio::task::spawn_blocking(move || {
            // Exact live-entry count. O(n) over the keyspace index — acceptable for a diagnostic,
            // and honest: `approximate_len()` over-counts after deletes (tombstones).
            let stored_captures = u64::conv(
                keyspace.inner().len().map_err(|err| GetOutputError::Storage(Box::new(err)))?,
            );

            // On-disk footprint of this keyspace, post-LZ4 (differs from any uncompressed sum).
            let disk_bytes = keyspace.inner().disk_space();

            // History ids are UUIDv7; their bytes sort in creation order, so the first and last
            // keys are the oldest and newest captures.
            let oldest_capture_unix_ms = keyspace
                .first_key_value()
                .map(fjall::Guard::key)
                .transpose()
                .map_err(|err| GetOutputError::Storage(Box::new(err)))?
                .and_then(|key| key_to_unix_ms(key.as_ref()));
            let newest_capture_unix_ms = keyspace
                .last_key_value()
                .map(fjall::Guard::key)
                .transpose()
                .map_err(|err| GetOutputError::Storage(Box::new(err)))?
                .and_then(|key| key_to_unix_ms(key.as_ref()));

            Ok(Some(super::OutputCaptureStats {
                stored_captures,
                disk_bytes,
                oldest_capture_unix_ms,
                newest_capture_unix_ms,
                store_path: keyspace.path(),
                schema: ActiveSchema::NAME,
            }))
        })
        .await
        .expect("output-capture stats task panicked")
    }
}

/// Decode the creation time (unix ms) embedded in a UUIDv7 history-id key.
///
/// Returns `None` for keys that are not a 16-byte UUIDv7 (e.g. a test id built from a plain
/// `u128`), so a non-v7 id degrades to "unknown" rather than a bogus timestamp.
fn key_to_unix_ms(key: &[u8]) -> Option<u64> {
    let bytes: [u8; 16] = key.try_into().ok()?;
    let ts = uuid::Uuid::from_bytes(bytes).get_timestamp()?;
    let (secs, subsec_nanos) = ts.to_unix();
    Some(secs.saturating_mul(1000).saturating_add(u64::from(subsec_nanos) / 1_000_000))
}

#[cfg(test)]
mod tests {
    use easy_cast::Conv;
    use uuid::Uuid;

    use super::*;

    fn temp_backend() -> (FjallBackend, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FjallBackend::open(dir.path()).expect("open");
        (backend, dir)
    }

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*Uuid::from_u128(n).as_bytes())
    }

    /// A history id backed by a real UUIDv7, so its embedded creation time is decodable. The
    /// module's `hid(n)` builds a plain `Uuid::from_u128`, which is not v7 and has no timestamp.
    fn hid_v7() -> HistoryId {
        HistoryId::from_bytes(*atuin_common::utils::uuid_v7().as_bytes())
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
        let (store, _dir) = temp_backend();
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
        let (store, _dir) = temp_backend();
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
        let (store, _dir) = temp_backend();
        store.capture(hid(1), split_cap("kept", "", 500)).await.expect("capture");
        store.capture(hid(2), cap("kept")).await.expect("capture");

        let split = store.get(hid(1)).await.expect("get").expect("present");
        let whole = store.get(hid(2)).await.expect("get").expect("present");
        assert_eq!(split.output_end.as_deref(), Some(""));
        assert_eq!(whole.output_end, None);
    }

    #[tokio::test]
    async fn missing_id_returns_none() {
        let (store, _dir) = temp_backend();
        assert!(store.get(hid(9)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn second_capture_for_same_id_is_rejected() {
        let (store, _dir) = temp_backend();
        store.capture(hid(1), cap("first")).await.expect("first");
        let err = store.capture(hid(1), cap("second")).await.unwrap_err();
        assert!(matches!(err, CaptureError::AlreadyExists));
        // The first write survives.
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "first");
    }

    #[tokio::test]
    async fn concurrent_writers_store_exactly_one() {
        let (store, _dir) = temp_backend();
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
        let (store, _dir) = temp_backend();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.remove(vec![hid(1)]).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_of_absent_ids_is_ok() {
        let (store, _dir) = temp_backend();
        store.remove(vec![]).await.expect("remove of nothing is idempotent");
        store.remove(vec![hid(9)]).await.expect("remove of an absent id is idempotent");
    }

    #[tokio::test]
    async fn remove_only_removes_requested_ids() {
        let (store, _dir) = temp_backend();
        for n in 1..=3u128 {
            store.capture(hid(n), cap(&format!("out{n}"))).await.expect("capture");
        }
        store.remove(vec![hid(1), hid(3), hid(9)]).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
        assert_eq!(store.get(hid(2)).await.expect("get").expect("kept").output_start, "out2");
        assert!(store.get(hid(3)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn removed_id_can_be_captured_again() {
        let (store, _dir) = temp_backend();
        store.capture(hid(1), cap("first")).await.expect("first");
        store.remove(vec![hid(1)]).await.expect("remove");
        // The tombstone must free the id for the capture-once check, not merely hide the value.
        store.capture(hid(1), cap("second")).await.expect("recapture after remove");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output_start, "second");
    }

    #[tokio::test]
    async fn remove_after_removal_is_idempotent() {
        let (store, _dir) = temp_backend();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.remove(vec![hid(1)]).await.expect("remove");
        assert!(store.get(hid(1)).await.expect("get").is_none());
        // Re-removing an already-removed id alongside an absent one is still Ok.
        store.remove(vec![hid(1), hid(9)]).await.expect("remove again");
    }

    #[tokio::test]
    async fn stats_counts_sizes_and_time_range() {
        let (store, _dir) = temp_backend();

        // An empty store reports itself as an active fjall store with nothing in it.
        let stats = store.stats().await.expect("stats").expect("fjall backend reports stats");
        assert_eq!(stats.stored_captures, 0);
        assert_eq!(stats.oldest_capture_unix_ms, None);
        assert_eq!(stats.newest_capture_unix_ms, None);
        assert_eq!(stats.schema, "output_capture_v2");

        store.capture(hid_v7(), cap("first")).await.expect("capture");
        store.capture(hid_v7(), cap("second")).await.expect("capture");

        // `disk_space()` only counts flushed segments, and two tiny captures never cross the
        // (64 MiB default) memtable threshold on their own; force the flush fjall's own tests use.
        store.keyspace.inner().rotate_memtable_and_wait().expect("flush memtable to disk");
        let stats = store.stats().await.expect("stats").expect("present");
        assert_eq!(stats.stored_captures, 2);
        assert!(stats.disk_bytes > 0, "a non-empty keyspace occupies disk");
        // Keys sort in creation order, so the first key's time is never after the last key's.
        let oldest = stats.oldest_capture_unix_ms.expect("v7 ids decode to a time");
        let newest = stats.newest_capture_unix_ms.expect("v7 ids decode to a time");
        assert!(oldest <= newest);
    }
}
