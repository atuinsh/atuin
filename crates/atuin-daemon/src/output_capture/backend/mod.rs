#![allow(dead_code, unused_imports)]

mod fjall;
mod nop;

use std::future::Future;

use atuin_client::history::{CommandCapture, HistoryId};
use thiserror::Error;

pub use fjall::FjallBackend;
pub use nop::NopBackend;

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
