//! Atuin's GRPC daemon transport layer.
//!
//! The client talks to the daemon over GRPC. This module does all of that.

/// Trait for reply types that include the daemon version and protocol version.
pub trait VersionedReply {
    fn version(&self) -> &str;
    fn protocol(&self) -> u32;
}

/// Mark a reply as versioned.
///
/// Versioned replies are protobuf messages that have a `version` field and a `protocol` field. This
/// will implement the [`VersionedReply`] trait for the given messages.
macro_rules! versioned_messages {
    ($($name:ident),* $(,)?) => {
        $(
            impl crate::grpc::VersionedReply for $name {
                fn version(&self) -> &str {
                    &self.version
                }

                fn protocol(&self) -> u32 {
                    self.protocol
                }
            }
        )*
    };
}

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

pub mod common;
pub mod history;

pub use history::Service as HistoryService;
