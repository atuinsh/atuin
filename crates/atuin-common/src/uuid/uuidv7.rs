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

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::OffsetDateTime;
    use time::macros::datetime;
    use uuid::Uuid;

    use super::UuidV7Ext;

    #[rstest]
    #[case::same_namespace_and_name("ns", b"name", "ns", b"name", true)]
    #[case::another_name("ns", b"name", "ns", b"other", false)]
    #[case::another_namespace("ns", b"name", "other", b"name", false)]
    #[case::the_split_moved("a", b"bc", "ab", b"c", false)]
    fn is_the_same_exactly_when_namespace_and_name_are(
        #[case] namespace: &str,
        #[case] name: &[u8],
        #[case] other_namespace: &str,
        #[case] other_name: &[u8],
        #[case] same: bool,
    ) {
        let at = datetime!(2024-01-02 03:04:05 UTC);
        assert_eq!(
            Uuid::new_v7_named(at, namespace, name)
                == Uuid::new_v7_named(at, other_namespace, other_name),
            same
        );
    }

    #[rstest]
    #[case::after_the_epoch(datetime!(2024-01-02 03:04:05.678 UTC), 1_704_164_645_678)]
    #[case::before_the_epoch(datetime!(1960-01-01 0:00 UTC), 0)]
    fn is_a_v7_stamped_with_the_timestamp(#[case] timestamp: OffsetDateTime, #[case] millis: u64) {
        let id = Uuid::new_v7_named(timestamp, "ns", b"name");

        assert_eq!(id.get_version(), Some(uuid::Version::SortRand));
        let (seconds, nanos) = id.get_timestamp().unwrap().to_unix();
        assert_eq!(seconds * 1000 + u64::from(nanos / 1_000_000), millis);
    }
}
