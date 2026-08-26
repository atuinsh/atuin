//! History module for the daemon gRPC history service.
//!
//! This module contains the proto-generated types for the history gRPC service.

// Include the generated proto code
mod proto {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]

    tonic::include_proto!("history");
}
pub use proto::*;

impl From<Option<atuin_client::history::AuthorKind>> for AuthorKind {
    fn from(kind: Option<atuin_client::history::AuthorKind>) -> Self {
        match kind {
            None => Self::Unspecified,
            Some(atuin_client::history::AuthorKind::User) => Self::User,
            Some(atuin_client::history::AuthorKind::Agent) => Self::Agent,
        }
    }
}

impl From<AuthorKind> for Option<atuin_client::history::AuthorKind> {
    fn from(kind: AuthorKind) -> Self {
        match kind {
            AuthorKind::Unspecified => None,
            AuthorKind::User => Some(atuin_client::history::AuthorKind::User),
            AuthorKind::Agent => Some(atuin_client::history::AuthorKind::Agent),
        }
    }
}

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
