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

/// A point-in-time snapshot of the durable capture store's size and contents.
#[derive(Debug, Clone)]
pub struct OutputCaptureStats {
    /// Exact number of captures currently stored.
    pub stored_captures: u64,
    /// On-disk size of the store's keyspace, in bytes (post-compression).
    pub disk_bytes: u64,
    /// Creation time (unix ms) of the oldest / newest stored capture, decoded from the UUIDv7
    /// history-id keys. `None` when the store is empty (or an id is not a UUIDv7).
    pub oldest_capture_unix_ms: Option<u64>,
    pub newest_capture_unix_ms: Option<u64>,
    /// Absolute path of the store directory.
    pub store_path: std::path::PathBuf,
    /// Keyspace/schema name, e.g. `"output_capture_v2"`.
    pub schema: &'static str,
}

#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code and we don't need it to be Send")]
pub trait Backend {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError>;

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError>;

    /// Forget the captured output of every history id in `ids`. Absent ids are ignored.
    async fn remove(&self, ids: Vec<HistoryId>) -> Result<(), DeleteOutputError>;

    /// Store-wide statistics, or `None` for a backend that does not persist (the nop backend).
    async fn stats(&self) -> Result<Option<OutputCaptureStats>, GetOutputError>;
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
