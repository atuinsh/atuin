//! History module for the daemon gRPC history service.
//!
//! This module contains the proto-generated types for the history gRPC service.

// Include the generated proto code. `common` holds the shared primitive types (e.g. `Uuid`) that
// `history` imports; the generated `history` code refers to them via `super::common`, so `common`
// must sit alongside `proto` here.
pub mod common {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]

    tonic::include_proto!("common");
}
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

#[cfg(test)]
mod tests {
    use atuin_client::history::AuthorKind as ClientAuthorKind;
    use rstest::rstest;

    use super::*;

    /// The two hand-written matches compose into a lossless round trip in both directions:
    /// a flipped arm would silently reclassify agents as users (or lose the "not stated" case).
    #[rstest]
    #[case(None, AuthorKind::Unspecified)]
    #[case(Some(ClientAuthorKind::User), AuthorKind::User)]
    #[case(Some(ClientAuthorKind::Agent), AuthorKind::Agent)]
    fn author_kind_round_trips(
        #[case] client: Option<ClientAuthorKind>,
        #[case] proto: AuthorKind,
    ) {
        assert_eq!(AuthorKind::from(client), proto);
        assert_eq!(Option::<ClientAuthorKind>::from(proto), client);
    }
}
