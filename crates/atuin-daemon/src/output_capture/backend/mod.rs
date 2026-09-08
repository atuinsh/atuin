mod fjall;
mod nop;

use std::future::Future;

use atuin_client::history::{CommandCapture, HistoryId};
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

pub trait Backend {
    fn capture(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> impl Future<Output = Result<(), CaptureError>> + Send;

    fn get(
        &self,
        id: HistoryId,
    ) -> impl Future<Output = Result<Option<CommandCapture>, GetOutputError>> + Send;
}

#[derive(Debug, strum_macros::EnumDiscriminants)]
#[strum_discriminants(name(BackendKind))]
pub enum AnyBackend {
    Fjall(FjallBackend),
    Nop(NopBackend),
}

impl Backend for AnyBackend {
    async fn capture(&self, id: HistoryId, capture: CommandCapture) -> Result<(), CaptureError> {
        match self {
            Self::Fjall(backend) => backend.capture(id, capture).await,
            Self::Nop(backend) => backend.capture(id, capture).await,
        }
    }

    async fn get(&self, id: HistoryId) -> Result<Option<CommandCapture>, GetOutputError> {
        match self {
            Self::Fjall(backend) => backend.get(id).await,
            Self::Nop(backend) => backend.get(id).await,
        }
    }
}
