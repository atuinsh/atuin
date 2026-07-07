//! History module for the daemon gRPC history service.
//!
//! This module contains the proto-generated types for the history gRPC service.

// Include the generated proto code
tonic::include_proto!("history");

/// Trait for reply types that include the daemon version and protocol version.
pub trait VersionedReply {
    fn version(&self) -> &str;
    fn protocol(&self) -> u32;
}

macro_rules! impl_versioned_reply {
    ($($name:ident),* $(,)?) => {
        $(
            impl VersionedReply for $name {
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

impl_versioned_reply!(StartHistoryReply, EndHistoryReply, CancelHistoryReply);
