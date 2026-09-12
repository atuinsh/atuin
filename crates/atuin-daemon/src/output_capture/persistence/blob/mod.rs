//! Durable storage for captured output.

#[cfg(test)]
mod failing;
mod fjall;
mod nop;

use atuin_client::history::{CommandCapture, HistoryId};
use atuin_common::futures::stream::ChunkedStream;
#[cfg(test)]
pub use failing::FailingBlobStore;
pub use fjall::FjallBlobStore;
pub use nop::NopBlobStore;
use thiserror::Error;

pub type StorageError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("history id already has an associated capture")]
    AlreadyExists,
    #[error("storage error: {0}")]
    Storage(#[source] StorageError),
    #[error("failed to serialize the capture: {0}")]
    Serialize(#[source] StorageError),
}

#[derive(Debug, Error)]
pub enum GetOutputError {
    #[error("storage error: {0}")]
    Storage(#[source] StorageError),
}

#[derive(Debug, Error)]
pub enum DeleteOutputError {
    #[error("storage error: {0}")]
    Storage(#[source] StorageError),
}

#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait BlobStore {
    /// Try to store the [`CommandCapture`] associated with the given [`HistoryId`].
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError>;

    /// Fetch the [`CommandCapture`] for the given [`HistoryId`].
    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError>;

    /// Forget the captured output of every history id in `ids`. Absent ids are ignored.
    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), DeleteOutputError>;

    /// On-disk bytes the store occupies. A store that persists nothing reports 0.
    ///
    /// Doesn't have to be exact -- just try to be within 100MB of error.
    fn estimated_disk_space(&self) -> u64;

    /// Every stored id, oldest first (by key order).
    async fn all_ids(&self) -> ChunkedStream<Result<HistoryId, GetOutputError>>;

    /// The oldest stored ids whose values total at least `reclaim_bytes`.
    ///
    /// TODO(markovejnovic): Don't return Vec. In a pathological case, this can be a lot of memory,
    ///                      but we call this relatively frequently so the candidate count should
    ///                      be relatively small.
    ///
    ///                      Check the invocation site to see it is called in a periodic (1m) task
    ///                      which cleans up old entries. I wouldn't worry about this too much.
    ///
    ///                      Famous last words.
    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError>;
}

#[derive(Debug)]
pub enum AnyBlobStore {
    Fjall(FjallBlobStore),
    Nop(NopBlobStore),
    #[cfg(test)]
    Failing(FailingBlobStore),
}

impl BlobStore for AnyBlobStore {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError> {
        match self {
            Self::Fjall(s) => s.capture(id, capture).await,
            Self::Nop(s) => s.capture(id, capture).await,
            #[cfg(test)]
            Self::Failing(s) => s.capture(id, capture).await,
        }
    }

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        match self {
            Self::Fjall(s) => s.get(id).await,
            Self::Nop(s) => s.get(id).await,
            #[cfg(test)]
            Self::Failing(s) => s.get(id).await,
        }
    }

    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), DeleteOutputError> {
        match self {
            Self::Fjall(s) => s.remove(ids).await,
            Self::Nop(s) => s.remove(ids).await,
            #[cfg(test)]
            Self::Failing(s) => s.remove(ids).await,
        }
    }

    fn estimated_disk_space(&self) -> u64 {
        match self {
            Self::Fjall(s) => s.estimated_disk_space(),
            Self::Nop(s) => s.estimated_disk_space(),
            #[cfg(test)]
            Self::Failing(s) => s.estimated_disk_space(),
        }
    }

    async fn all_ids(&self) -> ChunkedStream<Result<HistoryId, GetOutputError>> {
        match self {
            Self::Fjall(s) => s.all_ids().await,
            Self::Nop(s) => s.all_ids().await,
            #[cfg(test)]
            Self::Failing(s) => s.all_ids().await,
        }
    }

    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError> {
        match self {
            Self::Fjall(s) => s.eviction_candidates(reclaim_bytes).await,
            Self::Nop(s) => s.eviction_candidates(reclaim_bytes).await,
            #[cfg(test)]
            Self::Failing(s) => s.eviction_candidates(reclaim_bytes).await,
        }
    }
}
