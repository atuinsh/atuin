//! Durable storage for captured command output.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use atuin_client::history::HistoryId;
use fjall::config::CompressionPolicy;
use fjall::{OptimisticTxDatabase, OptimisticTxKeyspace, PersistMode, Readable};
use prost::Message;
use thiserror::Error;
use tokio::task::JoinHandle;
use tracing::error;

use crate::grpc::history::pb::CommandCapture;

/// fjall keyspace name for stored output.
const KEYSPACE_NAME: &str = "command_output";

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("history id already has an associated capture")]
    AlreadyExists,
    #[error("storage error")]
    Storage(
        #[source]
        #[from]
        fjall::Error,
    ),
}

#[derive(Debug, Error)]
pub enum GetOutputError {
    #[error("storage error")]
    Storage(
        #[source]
        #[from]
        fjall::Error,
    ),
}

#[derive(Debug, Error)]
pub enum DeleteOutputError {
    #[error("storage error")]
    Storage(
        #[source]
        #[from]
        fjall::Error,
    ),
}

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

/// [`OutputCapture`] is the core engine responsible for collecting command output.
///
/// It wraps an [`OptimisticTxKeyspace`], and stores data in said keyspace.
#[derive(derive_more::Debug)]
pub struct OutputCapture {
    #[debug(skip)]
    db: OptimisticTxDatabase,
    #[debug(skip)]
    keyspace: OptimisticTxKeyspace,
    flusher: Arc<Flusher>,
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

        Ok(Self {
            db: db.clone(),
            keyspace,
            flusher: Arc::new(Flusher::spawn(db)),
        })
    }

    /// Capture a command and associate it with the given history id.
    pub async fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        let key = id.into_bytes();
        let value = capture.encode_to_vec();

        let flusher = self.flusher.clone();
        tokio::task::spawn_blocking(move || {
            let mut tx = db.write_tx()?;
            if tx.contains_key(&keyspace, key)? {
                return Err(CaptureError::AlreadyExists);
            }

            tx.insert(&keyspace, key, value);
            match tx.commit()? {
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

    /// Forget the captured output of every history id in `ids`.
    pub async fn delete(
        &self,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<(), DeleteOutputError> {
        let keys: Vec<[u8; 16]> = ids.into_iter().map(HistoryId::into_bytes).collect();
        if keys.is_empty() {
            return Ok(());
        }

        let db = self.db.clone();
        let keyspace = self.keyspace.clone();
        tokio::task::spawn_blocking(move || {
            // Fjall deletes by leaving tombstones.
            //
            // If a crash were to happen between this transaction finishing and the flusher fsyncing
            // it, the tombstone would never commit, which means that a subsequent reboot would
            // resurrect the entry.
            //
            // If the user wants to delete something, chances are they want to delete it **NOW**.
            // They don't delete super often anyways, so let's just fsync immediately.
            let mut tx = db.write_tx()?.durability(Some(PersistMode::SyncAll));
            for key in keys {
                tx.remove(&keyspace, key);
            }
            match tx.commit()? {
                Ok(()) => Ok(()),
                // fjall only reports conflicts for transactions that read; this one never does.
                Err(fjall::Conflict) => {
                    unreachable!("a blind remove performs no reads, so it can never conflict")
                }
            }
        })
        .await
        .expect("output-capture delete task panicked")
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use easy_cast::Conv;
    use rstest::{fixture, rstest};
    use uuid::Uuid;

    use super::*;
    use crate::grpc::history::pb::CommandCaptureMeta;

    /// An [`OutputCapture`] over a fresh temp dir. Dropping the guard removes the dir.
    struct TempStore {
        store: OutputCapture,
        _dir: tempfile::TempDir,
    }

    impl Deref for TempStore {
        type Target = OutputCapture;

        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }

    #[fixture]
    fn store() -> TempStore {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = OutputCapture::open(dir.path()).expect("open");
        TempStore { store, _dir: dir }
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
                terminal_width: 80,
                terminal_height: 24,
            }),
        }
    }

    #[rstest]
    #[tokio::test]
    async fn round_trips_output_by_history_id(store: TempStore) {
        store.capture(hid(1), cap("hello")).await.expect("capture");
        let got = store.get(hid(1)).await.expect("get").expect("present");
        assert_eq!(got.output, "hello");
        assert_eq!(got.meta.expect("meta").output_observed_bytes, 5);
    }

    #[rstest]
    #[tokio::test]
    async fn missing_id_returns_none(store: TempStore) {
        assert!(store.get(hid(9)).await.expect("get").is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn second_capture_for_same_id_is_rejected(store: TempStore) {
        store.capture(hid(1), cap("first")).await.expect("first");
        let err = store.capture(hid(1), cap("second")).await.unwrap_err();
        assert!(matches!(err, CaptureError::AlreadyExists));
        // The first write survives.
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output, "first");
    }

    #[rstest]
    #[tokio::test]
    async fn concurrent_writers_store_exactly_one(store: TempStore) {
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

    #[rstest]
    #[tokio::test]
    async fn delete_removes_stored_output(store: TempStore) {
        store.capture(hid(1), cap("hello")).await.expect("capture");
        store.delete([hid(1)]).await.expect("delete");
        assert!(store.get(hid(1)).await.expect("get").is_none());
    }

    #[rstest]
    #[case::no_ids(vec![])]
    #[case::unknown_id(vec![hid(9)])]
    #[tokio::test]
    async fn delete_of_absent_ids_is_ok(store: TempStore, #[case] ids: Vec<HistoryId>) {
        store.delete(ids).await.expect("delete is idempotent");
    }

    #[rstest]
    #[tokio::test]
    async fn delete_only_removes_requested_ids(store: TempStore) {
        for n in 1..=3 {
            store.capture(hid(n), cap(&format!("out{n}"))).await.expect("capture");
        }
        // Present and absent ids in the same batch: the absent one is simply skipped.
        store.delete([hid(1), hid(3), hid(9)]).await.expect("delete");
        assert!(store.get(hid(1)).await.expect("get").is_none());
        assert_eq!(store.get(hid(2)).await.expect("get").expect("kept").output, "out2");
        assert!(store.get(hid(3)).await.expect("get").is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn deleted_id_can_be_captured_again(store: TempStore) {
        store.capture(hid(1), cap("first")).await.expect("first");
        store.delete([hid(1)]).await.expect("delete");
        // The tombstone must free the id for the capture-once check, not merely hide the value.
        store.capture(hid(1), cap("second")).await.expect("recapture after delete");
        assert_eq!(store.get(hid(1)).await.expect("get").expect("present").output, "second");
    }
}
