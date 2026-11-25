//! The stored form of desktop capture records, and the layout of their keys.
//!
//! - `records`: the 16 bytes of a record's UUIDv7, to a [`StoredRecord`].
//! - `segment_records`: a segment's 16 bytes followed by a record's 16 bytes, to nothing.
//! - `contents`: a segment's 16 bytes followed by the 32 bytes of a content hash, to a
//!   [`StoredContent`].
//! - `meta`: [`FORMAT_KEY`] to the format of the stored data, as a big-endian `u32`,
//!   [`REWRITE_OWED_KEY`] to nothing when a removal waits for a rewrite,
//!   [`INDEX_REMOVAL_OWED_KEY`] to nothing when the search index can still hold a removed
//!   segment, and [`EVICTED_KEY`] to the bytes that evictions freed since the last rewrite, as a
//!   big-endian `u64`.
//!
//! Values in `records` and `contents` are MessagePack with named fields. The store never rewrites
//! a stored record or a content.

use std::time::{Duration, SystemTime};

use atuin_desktop_capture::{ContentHash, Trigger};
use atuin_domain::record::HostId;
use fjall::config::CompressionPolicy;
use fjall::{CompressionType, KeyspaceCreateOptions};
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs, serde_as};
use uuid::Uuid;

use super::model::{ContentKey, SegmentKey, SegmentKind};

pub const RECORDS: &str = "records";
pub const SEGMENT_RECORDS: &str = "segment_records";
pub const CONTENTS: &str = "contents";
pub const META: &str = "meta";

/// The key of the format in `meta`.
pub const FORMAT_KEY: &[u8] = b"format";

/// The key in `meta` that a removal sets when the store owes a rewrite, so that the store's
/// files drop the removed records. A rewrite leaves the key out of its copy.
pub const REWRITE_OWED_KEY: &[u8] = b"rewrite_owed";

/// The key in `meta` that each removal of a segment sets, so that the search index removes the
/// segment's rows even when the daemon stops before the index follows the removal. The
/// daemon removes the key once the index holds only the segments that the store holds.
pub const INDEX_REMOVAL_OWED_KEY: &[u8] = b"index_removal_owed";

/// The key in `meta` of the bytes that evictions freed since the last rewrite. A rewrite leaves
/// the key out of its copy.
pub const EVICTED_KEY: &[u8] = b"evicted";

/// The options of `records` and `segment_records`, which compress with LZ4 as output capture
/// does.
pub fn compressed() -> KeyspaceCreateOptions {
    KeyspaceCreateOptions::default()
        .data_block_compression_policy(CompressionPolicy::all(CompressionType::Lz4))
}

/// The options of `contents`, whose values are zstd frames already.
pub fn uncompressed() -> KeyspaceCreateOptions {
    KeyspaceCreateOptions::default()
        .data_block_compression_policy(CompressionPolicy::all(CompressionType::None))
}

/// A point in time, as nanoseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(u64);

impl From<SystemTime> for Timestamp {
    fn from(time: SystemTime) -> Self {
        let nanos = time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        Self(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

impl From<Timestamp> for SystemTime {
    fn from(time: Timestamp) -> Self {
        Self::UNIX_EPOCH + Duration::from_nanos(time.0)
    }
}

/// One record as it is stored.
///
/// A read decodes the start of a segment into a [`SegmentStart`] of its own. A write encodes
/// the start by reference, as a [`RecordToStore`].
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRecord<S = SegmentStart> {
    pub host: HostId,
    /// The segment that the record belongs to.
    pub segment: SegmentKey,
    pub at: Timestamp,
    pub body: StoredBody<S>,
}

/// A record on its way to the store, which borrows the start of its segment.
pub type RecordToStore<'a> = StoredRecord<&'a SegmentStart>;

/// The body of a [`RecordToStore`].
pub type BodyToStore<'a> = StoredBody<&'a SegmentStart>;

/// The parts of a [`StoredRecord`] that scans need, which decode without copying the start of
/// a segment.
#[derive(Debug, Deserialize)]
pub struct RecordHead {
    pub segment: SegmentKey,
    pub at: Timestamp,
    pub body: BodyKind,
}

/// The variant of a [`StoredBody`], with the fields of an observation that scans need.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub enum BodyKind {
    SegmentStarted(IgnoredAny),
    Observed(ObservedHead),
    SegmentEnded,
}

/// The fields of an `Observed` body that scans need.
#[serde_as]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct ObservedHead {
    #[serde_as(as = "HashBytes")]
    pub hash: ContentHash,
    pub trigger: Option<Trigger>,
    pub nodes: u64,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredBody<S = SegmentStart> {
    SegmentStarted(S),
    /// A snapshot of the segment's window, whose tree is the content under `hash`.
    Observed {
        #[serde_as(as = "HashBytes")]
        hash: ContentHash,
        /// What led capture to read the window. `None` for a dialog, which capture reads when
        /// it appears.
        trigger: Option<Trigger>,
        read_us: u64,
        nodes: u64,
    },
    SegmentEnded,
}

/// What a segment is about, as its first record says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentStart {
    pub kind: SegmentKind,
    pub app: String,
    pub title: String,
    pub url: Option<String>,
}

/// One distinct tree of a segment, compressed with zstd.
///
/// A content that is read borrows its bytes from the stored value.
#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredContent<'a> {
    /// The content that `bytes` is a delta from, in the same segment. `None` for a whole tree.
    #[serde_as(as = "Option<HashBytes>")]
    pub base: Option<ContentHash>,
    #[serde(borrow)]
    #[serde_as(as = "serde_with::Bytes")]
    pub bytes: &'a [u8],
}

/// Stores a [`ContentHash`] as 32 bytes of MessagePack binary.
struct HashBytes;

impl SerializeAs<ContentHash> for HashBytes {
    fn serialize_as<S: Serializer>(hash: &ContentHash, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&hash.0)
    }
}

impl<'de> DeserializeAs<'de, ContentHash> for HashBytes {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<ContentHash, D::Error> {
        serde_with::Bytes::deserialize_as(deserializer).map(ContentHash)
    }
}

/// The smallest record key whose time is `time` or later.
pub fn first_key_at(time: SystemTime) -> [u8; 16] {
    let millis = time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |since| since.as_millis());
    // A UUIDv7 starts with its time in milliseconds, in 48 bits.
    let millis = millis.min((1 << 48) - 1);
    Uuid::from_u128(millis << 80).into_bytes()
}

/// The smallest record key after `id`. `None` when no key follows it.
pub fn key_after(id: Uuid) -> Option<[u8; 16]> {
    id.as_u128().checked_add(1).map(u128::to_be_bytes)
}

pub fn segment_record_key(segment: SegmentKey, record: Uuid) -> [u8; 32] {
    let mut key = [0; 32];
    key[..16].copy_from_slice(segment.0.as_bytes());
    key[16..].copy_from_slice(record.as_bytes());
    key
}

/// The segment in a `segment_records` key.
pub fn segment_in_segment_key(key: &[u8]) -> Option<SegmentKey> {
    key.get(..16).and_then(|bytes| Uuid::from_slice(bytes).ok()).map(SegmentKey)
}

/// The record id in a `segment_records` key.
pub fn record_in_segment_key(key: &[u8]) -> Option<Uuid> {
    key.get(16..).and_then(|bytes| Uuid::from_slice(bytes).ok())
}

pub fn content_key(key: ContentKey) -> [u8; 48] {
    let mut bytes = [0; 48];
    bytes[..16].copy_from_slice(key.segment.0.as_bytes());
    bytes[16..].copy_from_slice(&key.hash.0);
    bytes
}

#[cfg(test)]
pub fn parse_content_key(key: &[u8]) -> Option<ContentKey> {
    let segment = Uuid::from_slice(key.get(..16)?).ok()?;
    let hash = key.get(16..)?.try_into().ok()?;
    Some(ContentKey {
        segment: SegmentKey(segment),
        hash: ContentHash(hash),
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn uuid() -> impl Strategy<Value = Uuid> {
        any::<u128>().prop_map(Uuid::from_u128)
    }

    fn hash() -> impl Strategy<Value = ContentHash> {
        any::<[u8; 32]>().prop_map(ContentHash)
    }

    fn trigger() -> impl Strategy<Value = Option<Trigger>> {
        proptest::option::of(proptest::sample::select(vec![
            Trigger::Activated,
            Trigger::TitleChanged,
            Trigger::Loaded,
            Trigger::Edited,
            Trigger::Restructured,
            Trigger::Polled,
            Trigger::Swept,
            Trigger::Retried,
        ]))
    }

    fn kind() -> impl Strategy<Value = SegmentKind> {
        proptest::sample::select(vec![
            SegmentKind::Active,
            SegmentKind::Dialog,
            SegmentKind::Background,
            SegmentKind::Notification,
        ])
    }

    fn body() -> impl Strategy<Value = StoredBody> {
        prop_oneof![
            (kind(), ".*", ".*", proptest::option::of(".*")).prop_map(|(kind, app, title, url)| {
                StoredBody::SegmentStarted(SegmentStart {
                    kind,
                    app,
                    title,
                    url,
                })
            }),
            (hash(), trigger(), any::<u64>(), any::<u64>()).prop_map(
                |(hash, trigger, read_us, nodes)| StoredBody::Observed {
                    hash,
                    trigger,
                    read_us,
                    nodes,
                }
            ),
            Just(StoredBody::SegmentEnded),
        ]
    }

    fn record() -> impl Strategy<Value = StoredRecord> {
        (uuid(), uuid(), any::<u64>(), body()).prop_map(|(host, segment, at, body)| StoredRecord {
            host: HostId(host),
            segment: SegmentKey(segment),
            at: Timestamp(at),
            body,
        })
    }

    /// The variant of `body`, as [`BodyKind`] names it.
    const fn kind_of(body: &StoredBody) -> BodyKind {
        match *body {
            StoredBody::SegmentStarted(_) => BodyKind::SegmentStarted(IgnoredAny),
            StoredBody::Observed {
                hash,
                trigger,
                nodes,
                ..
            } => BodyKind::Observed(ObservedHead {
                hash,
                trigger,
                nodes,
            }),
            StoredBody::SegmentEnded => BodyKind::SegmentEnded,
        }
    }

    /// The form of `record` that a write encodes.
    const fn to_store(record: &StoredRecord) -> RecordToStore<'_> {
        let body = match record.body {
            StoredBody::SegmentStarted(ref start) => StoredBody::SegmentStarted(start),
            StoredBody::Observed {
                hash,
                trigger,
                read_us,
                nodes,
            } => StoredBody::Observed {
                hash,
                trigger,
                read_us,
                nodes,
            },
            StoredBody::SegmentEnded => StoredBody::SegmentEnded,
        };
        StoredRecord {
            host: record.host,
            segment: record.segment,
            at: record.at,
            body,
        }
    }

    proptest! {
        /// A record that a write encodes by reference decodes as the record.
        #[rstest]
        fn a_record_round_trips(record in record()) {
            let bytes = rmp_serde::to_vec_named(&to_store(&record)).unwrap();
            let head: RecordHead = rmp_serde::from_slice(&bytes).unwrap();
            prop_assert_eq!(head.segment, record.segment);
            prop_assert_eq!(head.at, record.at);
            prop_assert_eq!(head.body, kind_of(&record.body));
            prop_assert_eq!(rmp_serde::from_slice::<StoredRecord>(&bytes).unwrap(), record);
        }

        #[rstest]
        fn a_content_round_trips(base in proptest::option::of(hash()), bytes in any::<Vec<u8>>()) {
            let content = StoredContent { base, bytes: &bytes };
            let stored = rmp_serde::to_vec_named(&content).unwrap();
            let read: StoredContent<'_> = rmp_serde::from_slice(&stored).unwrap();
            prop_assert_eq!(read.base, base);
            prop_assert_eq!(read.bytes, &bytes[..]);
        }

        #[rstest]
        fn a_content_key_round_trips(segment in uuid(), hash in hash()) {
            let key = ContentKey { segment: SegmentKey(segment), hash };
            prop_assert_eq!(parse_content_key(&content_key(key)), Some(key));
        }

        #[rstest]
        fn a_segment_record_key_round_trips(segment in uuid(), record in uuid()) {
            let key = segment_record_key(SegmentKey(segment), record);
            prop_assert_eq!(segment_in_segment_key(&key), Some(SegmentKey(segment)));
            prop_assert_eq!(record_in_segment_key(&key), Some(record));
        }

        /// Segment record keys sort by their segment first, and then by their record.
        #[rstest]
        fn segment_record_keys_sort_by_segment_then_record(
            a in (uuid(), uuid()),
            b in (uuid(), uuid()),
        ) {
            let key = |(segment, record): (Uuid, Uuid)| segment_record_key(SegmentKey(segment), record);
            prop_assert_eq!(key(a).cmp(&key(b)), a.cmp(&b));
        }

        #[rstest]
        fn a_timestamp_round_trips(nanos in any::<u64>()) {
            let time = SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos);
            prop_assert_eq!(SystemTime::from(Timestamp::from(time)), time);
            prop_assert_eq!(Timestamp::from(time), Timestamp(nanos));
        }

        #[rstest]
        fn timestamps_keep_the_order_of_times(a in any::<u64>(), b in any::<u64>()) {
            let time = |nanos| SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos);
            prop_assert_eq!(Timestamp::from(time(a)).cmp(&Timestamp::from(time(b))), a.cmp(&b));
        }

        /// A UUIDv7 is at or after `first_key_at(time)` exactly when its millisecond is at or
        /// after the millisecond of `time`.
        #[rstest]
        fn first_key_at_splits_uuidv7_ids_by_time(
            millis in 0..(1_u64 << 48),
            random in any::<[u8; 10]>(),
            nanos in any::<u64>(),
        ) {
            let id = uuid::Builder::from_unix_timestamp_millis(millis, &random).into_uuid();
            let time = SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos);
            let time_millis = u64::try_from(Duration::from_nanos(nanos).as_millis()).unwrap();
            prop_assert_eq!(id.into_bytes() >= first_key_at(time), millis >= time_millis);
        }

        #[rstest]
        fn key_after_is_the_next_key(id in uuid()) {
            match key_after(id) {
                Some(next) => prop_assert_eq!(u128::from_be_bytes(next), id.as_u128() + 1),
                None => prop_assert_eq!(id.as_u128(), u128::MAX),
            }
        }
    }
}
