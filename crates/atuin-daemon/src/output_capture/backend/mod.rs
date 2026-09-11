#[cfg(test)]
mod failing;
mod fjall;
mod nop;

use atuin_client::history::{CommandCapture, HistoryId};
use enum_dispatch::enum_dispatch;
#[cfg(test)]
pub use failing::FailingBackend;
pub use fjall::FjallBackend;
pub use nop::NopBackend;
use thiserror::Error;

pub type BackendError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("history id already has an associated capture")]
    AlreadyExists,
    #[error("storage error: {0}")]
    Storage(#[source] BackendError),
    #[error("failed to serialize the capture: {0}")]
    Serialize(#[source] BackendError),
}

#[derive(Debug, Error)]
pub enum GetOutputError {
    #[error("storage error: {0}")]
    Storage(#[source] BackendError),
}

#[derive(Debug, Error)]
pub enum DeleteOutputError {
    #[error("storage error: {0}")]
    Storage(#[source] BackendError),
}

#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait Backend {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError>;

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError>;

    /// Forget the captured output of every history id in `ids`. Absent ids are ignored.
    async fn remove(&self, ids: Vec<HistoryId>) -> Result<(), DeleteOutputError>;

    /// On-disk bytes the store occupies. A store that persists nothing reports 0.
    fn estimated_disk_space(&self) -> u64;

    /// Every stored id, oldest first (by key order). Reads keys only; used to reconcile the derived
    /// search index against the store.
    async fn all_ids(&self) -> Result<Vec<HistoryId>, GetOutputError>;

    /// The oldest stored ids whose values total at least `reclaim_bytes` (or all of them, if the
    /// store holds less), for the garbage collector to evict. Does not delete anything.
    async fn eviction_candidates(
        &self,
        reclaim_bytes: u64,
    ) -> Result<Vec<HistoryId>, DeleteOutputError>;
}

#[enum_dispatch(Backend)]
#[derive(Debug, strum_macros::EnumDiscriminants)]
#[strum_discriminants(name(BackendKind))]
pub enum AnyBackend {
    Fjall(FjallBackend),
    Nop(NopBackend),
    /// A broken store, built only by the [`OutputCapture::failing`](crate::OutputCapture::failing)
    /// test hook. Never selected in production.
    #[cfg(test)]
    Failing(FailingBackend),
}
