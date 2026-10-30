//! The request a message for the user answers, so a process serving several shows each client its
//! own.

use std::str::FromStr;

use uuid::Uuid;

use super::USER;

/// The field of [`RequestId::span`] that holds the id.
pub(super) const FIELD: &str = "request";

/// A request a process serves for a client, such as a daemon call: a message shown inside its
/// [`span`](Self::span) carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, derive_more::Display)]
pub struct RequestId(Uuid);

impl RequestId {
    /// An id no other request has.
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    /// The span to serve the request in.
    #[must_use]
    pub fn span(self) -> tracing::Span {
        tracing::info_span!(target: USER, "request", request = self.0.as_u128())
    }

    pub(super) const fn from_u128(id: u128) -> Self {
        Self(Uuid::from_u128(id))
    }
}

impl FromStr for RequestId {
    type Err = uuid::Error;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        id.parse().map(Self)
    }
}
