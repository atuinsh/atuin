use time::OffsetDateTime;
use uuid::Uuid;
use xxhash_rust::xxh3::xxh3_128;

/// UUIDv7 constructors beyond [`Uuid::now_v7`] and [`Uuid::new_v7`].
pub trait UuidV7Ext {
    /// A UUIDv7 stamped `timestamp` whose other bits hash `key`, so the same pair always gives
    /// the same id: for ids drawn from data that can be read again, such as an imported shell
    /// session. A `timestamp` before the Unix epoch is stamped at the epoch.
    fn new_v7_keyed(timestamp: OffsetDateTime, key: &[u8]) -> Self;
}

impl UuidV7Ext for Uuid {
    fn new_v7_keyed(timestamp: OffsetDateTime, key: &[u8]) -> Self {
        let millis = u64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000).unwrap_or(0);
        let [_, _, _, _, _, _, random @ ..] = xxh3_128(key).to_le_bytes();
        uuid::Builder::from_unix_timestamp_millis(millis, &random).into_uuid()
    }
}
