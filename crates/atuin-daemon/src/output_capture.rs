//! Durable storage for captured command output.
//!
//! Output is keyed by `history_id` (16 UUID bytes) in a fjall keyspace; the
//! value is the encoded `history::CommandCapture`. Each write runs in an
//! optimistic transaction so the check-then-insert is atomic against concurrent
//! writers, and all blocking fjall I/O runs on tokio's blocking pool via
//! `spawn_blocking`.
//!
//! TODO(retention): the store grows unbounded; no eviction yet. See the design doc.

use atuin_client::history::HistoryId;
use fjall::config::CompressionPolicy;
use fjall::{OptimisticTxDatabase, OptimisticTxKeyspace, Readable};
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

/// [`OutputCapture`] is the core engine responsible for collecting command output.
///
/// It wraps an [`OptimisticTxKeyspace`], and stores data in said keyspace.
#[derive(derive_more::Debug)]
pub struct OutputCapture {
    #[debug(skip)]
    db: OptimisticTxDatabase,
    #[debug(skip)]
    keyspace: OptimisticTxKeyspace,
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
        Ok(Self { db, keyspace })
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

        tokio::task::spawn_blocking(move || {
            let mut tx = db.write_tx()?;
            if tx.contains_key(&keyspace, key)? {
                return Err(CaptureError::AlreadyExists);
            }

            tx.insert(&keyspace, key, value);
            match tx.commit()? {
                Ok(()) => Ok(()),
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

#[cfg(test)]
mod tests {
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
}
