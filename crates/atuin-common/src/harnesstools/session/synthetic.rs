//! Content-addressed ids, for lines a harness gave no id of their own.
//!
//! Capture keys a row of such a line on a hash of everything the row takes from it (see the
//! daemon's `MessageEnricher`), so re-reading the line resolves to the same row. A reader that
//! has to name such a row (the line a resumed read follows on from) computes the id the same way.

use time::OffsetDateTime;

use super::{Message, SessionId};

/// Prefix of a content-addressed id.
pub const SYNTHETIC: &str = "syn-";

/// A hash of everything a row takes from an id-less line `m` of `session`, so lines that differ
/// in any of it (two usage records written in one millisecond) never share an id.
pub fn content_hash<M: Message + ?Sized>(session: &SessionId, m: &M) -> u64 {
    let canonical = serde_json::json!([
        session.as_ref(),
        m.timestamp().map(OffsetDateTime::unix_timestamp_nanos).map(|ns| ns.to_string()),
        m.role(),
        m.content(),
        m.title(),
        m.model(),
        m.usage(),
        m.stop_reason(),
        m.cwd(),
        m.git_branch(),
        m.parent_id(),
        m.parent_session(),
        m.turn_id(),
    ]);
    xxhash_rust::xxh3::xxh3_64(canonical.to_string().as_bytes())
}

/// The id of the `ordinal`th line with content hash `hash`: the first occurrence keeps the bare
/// hash; the nth identical line after it is `-n`.
#[must_use]
pub fn synthetic_id(hash: u64, ordinal: u32) -> String {
    match ordinal {
        0 => format!("{SYNTHETIC}{hash:016x}"),
        n => format!("{SYNTHETIC}{hash:016x}-{n}"),
    }
}

/// The hash and ordinal of a [`synthetic_id`]; `None` for any other id.
#[must_use]
pub fn parse_synthetic(id: &str) -> Option<(u64, u32)> {
    let rest = id.strip_prefix(SYNTHETIC)?;
    let (hash, ordinal) = match rest.split_once('-') {
        Some((hash, n)) => (hash, n.parse().ok()?),
        None => (rest, 0),
    };
    Some((u64::from_str_radix(hash, 16).ok()?, ordinal))
}
