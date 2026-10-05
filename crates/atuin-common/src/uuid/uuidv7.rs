use time::OffsetDateTime;
use uuid::Uuid;
use xxhash_rust::xxh3::{xxh3_64, xxh3_128_with_seed};

/// UUIDv7 constructors beyond [`Uuid::now_v7`] and [`Uuid::new_v7`].
pub trait UuidV7Ext {
    /// A name-based UUIDv7, after the name-based v3 and v5: stamped `timestamp`, with its other
    /// bits a hash of `name` within `namespace`, so the same three always give the same id. For
    /// ids drawn from data that can be read again, such as a session imported from another tool,
    /// where `namespace` keeps one tool's names apart from another's. A `timestamp` before the
    /// Unix epoch is stamped at the epoch.
    fn new_v7_named(timestamp: OffsetDateTime, namespace: &str, name: &[u8]) -> Self;
}

impl UuidV7Ext for Uuid {
    fn new_v7_named(timestamp: OffsetDateTime, namespace: &str, name: &[u8]) -> Self {
        let millis = u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).unwrap_or(0);
        let hash = xxh3_128_with_seed(name, xxh3_64(namespace.as_bytes()));
        let [_, _, _, _, _, _, random @ ..] = hash.to_le_bytes();
        uuid::Builder::from_unix_timestamp_millis(millis, &random).into_uuid()
    }
}
