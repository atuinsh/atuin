//! Durable storage for captured command output.
//!
//! Output is keyed by `history_id` (16 UUID bytes) in a fjall keyspace; the
//! value is the encoded `history::CommandCapture`. Each write runs in an
//! optimistic transaction so the check-then-insert is atomic against concurrent
//! writers, and all blocking fjall I/O runs on tokio's blocking pool via
//! `spawn_blocking`.
//!
//! TODO(retention): the store grows unbounded; no eviction yet. See the design doc.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::HistoryId;
use fjall::config::CompressionPolicy;
use fjall::{OptimisticTxDatabase, OptimisticTxKeyspace, PersistMode, Readable};
use prost::Message;
use thiserror::Error;

use crate::grpc::history::pb::CommandCapture;

/// fjall keyspace name for stored output.
const KEYSPACE_NAME: &str = "command_output";

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("history id already has an associated capture")]
    AlreadyExists,
    #[error("storage error: {0}")]
    Storage(#[from] fjall::Error),
}

#[derive(Debug, Error)]
pub enum GetOutputError {
    #[error("storage error: {0}")]
    Storage(#[from] fjall::Error),
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("storage error: {0}")]
    Storage(#[from] fjall::Error),
}

/// [`OutputCapture`] is the core engine responsible for collecting command output.
///
/// It wraps an [`OptimisticTxKeyspace`], and stores data in said keyspace.
#[derive(derive_more::Debug)]
pub struct OutputCapture {
    #[debug(skip)]
    db: OptimisticTxDatabase,
    #[debug(skip)]
    keyspace: OptimisticTxKeyspace,
    /// Set whenever a capture is committed; cleared when the journal is flushed.
    /// Lets the periodic flusher skip `fdatasync` entirely on idle ticks.
    #[debug(skip)]
    dirty: Arc<AtomicBool>,
}

impl OutputCapture {
    /// Open (or create) the store at `path`.
    ///
    /// Currently, the only consumer of [`OptimisticTxDatabase`] is [`OutputCapture`] so it's safe
    /// to construct [`OutputCapture`] via this function.
    ///
    /// If, in the future, you need to share the database for other uses, you should definitely
    /// delete this function and inject the database via [`OutputCapture::new`].
    pub fn open(path: impl AsRef<std::path::Path>) -> fjall::Result<Self> {
        let db = OptimisticTxDatabase::builder(path.as_ref()).open()?;
        Self::new(db)
    }

    /// Create a new [`OutputCapture`] system.
    pub fn new(db: OptimisticTxDatabase) -> fjall::Result<Self> {
        let keyspace = db.keyspace(KEYSPACE_NAME, || {
            fjall::KeyspaceCreateOptions::default()
                .data_block_compression_policy(CompressionPolicy::all(fjall::CompressionType::Lz4))
                .with_kv_separation(Some(fjall::KvSeparationOptions::default()))
        })?;
        Ok(Self { db, keyspace, dirty: Arc::new(AtomicBool::new(false)) })
    }

    /// How often the background flusher persists the journal to disk.
    pub const SYNC_INTERVAL: Duration = Duration::from_secs(1);

    /// A cloneable handle that can flush this store's journal to disk.
    ///
    /// Kept separate from [`OutputCapture`] so the daemon can hold onto it to
    /// drive periodic and shutdown flushes after the store itself has been
    /// moved into the history journal.
    #[must_use]
    pub fn sync_handle(&self) -> OutputSyncHandle {
        OutputSyncHandle { db: self.db.clone(), dirty: self.dirty.clone() }
    }

    /// Capture a command and associate it with the given history id.
    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        let dirty = self.dirty.clone();
        let key = id.into_bytes();
        let value = capture.encode_to_vec();

        tokio::task::spawn_blocking(move || {
            let mut tx = db.write_tx()?;
            if tx.contains_key(&keyspace, key)? {
                return Err(CaptureError::AlreadyExists);
            }

            tx.insert(&keyspace, key, value);
            match tx.commit()? {
                Ok(()) => {
                    // A new capture is now in the journal buffer but not yet
                    // fsynced; the periodic flusher will pick it up.
                    dirty.store(true, Ordering::SeqCst);
                    Ok(())
                }
                // Another writer committed this key first, so it's already captured.
                Err(fjall::Conflict) => Err(CaptureError::AlreadyExists),
            }
        })
        .await
        .expect("output-capture write task panicked")
    }

    pub async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        let keyspace = self.keyspace.clone();
        let key = id.into_bytes();

        tokio::task::spawn_blocking(move || match keyspace.get(key)? {
            Some(slice) => {
                let capture = CommandCapture::decode(&*slice)
                    .expect("stored value is a valid CommandCapture");
                Ok(Some(capture))
            }
            None => Ok(None),
        })
        .await
        .expect("output-capture read task panicked")
    }
}

/// A cloneable handle for flushing the output-capture journal to disk.
///
/// Flushing affects durability only, never consistency: fjall never returns a
/// corrupt database, so a missed flush can only lose the most recent captures
/// on power loss, never damage older ones.
#[derive(Clone)]
pub struct OutputSyncHandle {
    db: OptimisticTxDatabase,
    dirty: Arc<AtomicBool>,
}

impl OutputSyncHandle {
    /// Flush the journal with `fdatasync`, but only if something has been
    /// captured since the last successful flush. Returns `true` if a flush ran.
    ///
    /// Skipping the syscall on clean ticks means an idle daemon does no I/O.
    pub async fn sync_if_dirty(&self) -> Result<bool, SyncError> {
        // Claim the pending writes; if there were none, there is nothing to do.
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return Ok(false);
        }

        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || db.persist(PersistMode::SyncData))
            .await
            .expect("output-capture sync task panicked");

        if let Err(e) = result {
            // A persist failure is fatal: fjall poisons the database, so every
            // subsequent persist (and write) short-circuits to `Err`. There is
            // no recoverable retry here; re-arm only so `dirty` keeps telling
            // the truth that these writes are still unsynced.
            self.dirty.store(true, Ordering::SeqCst);
            return Err(SyncError::from(e));
        }

        Ok(true)
    }

    /// Flush the journal with `fsync` (data + metadata) unconditionally.
    ///
    /// Used on graceful shutdown to guarantee everything captured is durable.
    pub async fn sync_now(&self) -> Result<(), SyncError> {
        // Claim any pending writes; a capture racing in after this re-sets the
        // flag and is caught by the next periodic tick.
        self.dirty.store(false, Ordering::SeqCst);
        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || db.persist(PersistMode::SyncAll))
            .await
            .expect("output-capture sync task panicked");
        if let Err(e) = result {
            // A persist failure is fatal: fjall poisons the database, so every
            // subsequent persist (and write) short-circuits to `Err`. There is
            // no recoverable retry here; re-arm only so `dirty` keeps telling
            // the truth that these writes are still unsynced.
            self.dirty.store(true, Ordering::SeqCst);
            return Err(SyncError::from(e));
        }
        Ok(())
    }
}

/// Periodically flush the output-capture journal until the task is aborted.
///
/// Flushes at most once per `period`, and issues no `fdatasync` on ticks where
/// nothing was captured. Intended to be `tokio::spawn`ed for the life of the
/// daemon.
pub async fn run_periodic_sync(handle: OutputSyncHandle, period: Duration) {
    let mut ticker = tokio::time::interval(period);
    // If a flush (or the whole runtime) stalls, don't fire a burst of catch-up
    // ticks afterwards — just resume the cadence.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        if let Err(e) = handle.sync_if_dirty().await {
            // A persist failure poisons the fjall database: every subsequent
            // persist (and write) will also fail, so retrying on the next
            // tick is futile. Log once and stop the loop instead of spamming
            // a warning every tick forever.
            tracing::error!(
                "output-capture flush failed, database is now unrecoverable, stopping periodic flush: {e}"
            );
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use easy_cast::Conv;
    use uuid::Uuid;

    use super::*;
    use crate::grpc::history::pb::CommandCaptureMeta;

    fn temp_capture() -> (OutputCapture, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = OutputCapture::open(dir.path()).expect("open");
        (store, dir)
    }

    fn hid(n: u128) -> HistoryId {
        HistoryId::from_bytes(*Uuid::from_u128(n).as_bytes())
    }

    fn cap(output: &str) -> CommandCapture {
        CommandCapture {
            output: output.to_string(),
            meta: Some(CommandCaptureMeta {
                output_truncated: false,
                output_observed_bytes: u64::conv(output.len()),
            }),
        }
    }

    #[tokio::test]
    async fn round_trips_output_by_history_id() {
        let (store, _dir) = temp_capture();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        let got = store.get(hid(1)).await.expect("get").expect("present");
        assert_eq!(got.output, "hello");
        assert_eq!(got.meta.expect("meta").output_observed_bytes, 5);
    }

    #[tokio::test]
    async fn missing_id_returns_none() {
        let (store, _dir) = temp_capture();
        assert!(store.get(hid(9)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn second_capture_for_same_id_is_rejected() {
        let (store, _dir) = temp_capture();
        store.capture(hid(1), cap("first")).await.expect("first");
        let err = store.capture(hid(1), cap("second")).await.unwrap_err();
        assert!(matches!(err, CaptureError::AlreadyExists));
        // The first write survives.
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output, "first");
    }

    #[tokio::test]
    async fn concurrent_writers_store_exactly_one() {
        let (store, _dir) = temp_capture();
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
    async fn sync_flushes_only_when_dirty() {
        let (store, _dir) = temp_capture();
        let handle = store.sync_handle();

        // Nothing captured yet: no flush should run.
        assert!(!handle.sync_if_dirty().await.expect("sync"));

        store.capture(hid(1), cap("hello")).await.expect("capture");

        // A capture marked the store dirty: the next call flushes.
        assert!(handle.sync_if_dirty().await.expect("sync"));
        // Nothing new since: the following call is a no-op (no fsync issued).
        assert!(!handle.sync_if_dirty().await.expect("sync"));
    }

    #[tokio::test]
    async fn sync_now_is_ok_on_empty_store() {
        let (store, _dir) = temp_capture();
        // Shutdown flush must be safe even if nothing was ever captured.
        store.sync_handle().sync_now().await.expect("sync_now");
    }

    #[tokio::test]
    async fn sync_now_flushes_and_data_is_readable() {
        // NOTE: this verifies the flush API does not error and does not lose data
        // in-process. It does NOT (and cannot, from a unit test) prove bytes
        // reached the physical disk.
        let (store, _dir) = temp_capture();
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.sync_handle().sync_now().await.expect("sync_now");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output, "hello");
    }

    #[tokio::test]
    async fn periodic_sync_flushes_captured_output() {
        let (store, _dir) = temp_capture();
        let handle = store.sync_handle();
        store.capture(hid(1), cap("hello")).await.expect("capture"); // marks dirty

        // Run the loop with a tiny period so it flushes quickly.
        let task = tokio::spawn(run_periodic_sync(handle.clone(), Duration::from_millis(5)));
        tokio::time::sleep(Duration::from_millis(50)).await; // >> period: several ticks
        task.abort();

        // The loop consumed the dirty flag, so a manual flush is now a no-op.
        assert!(!handle.sync_if_dirty().await.expect("sync"), "loop should have flushed");
    }
}
