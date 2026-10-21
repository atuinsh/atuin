//! Atuin's GRPC daemon transport layer.
//!
//! The client talks to the daemon over GRPC. This module does all of that.

/// Mark a [`thiserror::Error`] type as an invalid argument error.
///
/// Invalid argument errors receive an [`Into`] implementation into [`tonic::Status`] which returns
/// the error as [`tonic::Status::invalid_argument`].
macro_rules! invalid_argument_errors {
    ($($err:ty),* $(,)?) => {
        $(
            impl From<$err> for tonic::Status {
                fn from(value: $err) -> Self {
                    Self::invalid_argument(value.to_string())
                }
            }
        )*
    };
}

/// Mark a [`thiserror::Error`] type as an internal error.
///
/// Invalid argument errors receive an [`Into`] implementation into [`tonic::Status`] which returns
/// the error as [`tonic::Status::internal`].
macro_rules! internal_errors {
    ($($err:ty),* $(,)?) => {
        $(
            impl From<$err> for tonic::Status {
                fn from(value: $err) -> Self {
                    Self::internal(value.to_string())
                }
            }
        )*
    };
}

pub mod ai;
pub mod common;
pub mod history;
pub mod versioning;

pub use ai::session::Service as AiSessionService;
pub use history::Service as HistoryService;
