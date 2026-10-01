use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A `date-time` from the API, on the wire as RFC 3339, e.g. `2026-08-01T00:00:00Z`.
///
/// A newtype because the workspace's `time` serializes [`OffsetDateTime`] in its own
/// human-readable format, which rejects RFC 3339.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DateTime(#[serde(with = "time::serde::rfc3339")] pub OffsetDateTime);
