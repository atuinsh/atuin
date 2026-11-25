//! Typed access to the store's keyspaces, with the store's accounting of the bytes that it uses.
//!
//! Every read goes through a [`Reader`], which sees one consistent snapshot. Every write goes
//! through a [`Writer`], which tracks the bytes that it inserts and frees and settles them on
//! commit.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use atuin_domain::record::HostId;
use fjall::{
    KeyspaceCreateOptions, OptimisticTxDatabase, OptimisticTxKeyspace, OptimisticWriteTx,
    PersistMode, Readable as _, Slice, Snapshot,
};
use parking_lot::Mutex;
use tracing::warn;
use uuid::Uuid;

use super::format;
use super::model::{ContentKey, SegmentKey, StoreError};
use super::schema::{
    self, BodyToStore, RecordHead, RecordToStore, SegmentStart, StoredBody, StoredContent,
    StoredRecord, Timestamp,
};
use crate::flusher::{DirtyFlag, Persist};

/// How many times [`retry_on_conflict`] tries a write.
const ATTEMPTS: u32 = 3;

/// The store's running estimate of the bytes that it uses.
///
/// The estimate starts at the size of the database on disk, and then grows by the size of each
/// insert and shrinks by the size of each deletion. The store keeps its own estimate because
/// fjall reclaims the space of a deletion only later, so the size on disk lags behind
/// deletions.
#[derive(Debug)]
pub struct Usage(AtomicU64);

impl Usage {
    fn add(&self, bytes: u64) {
        self.0.fetch_add(bytes, Ordering::Relaxed);
    }

    fn sub(&self, bytes: u64) {
        let _ = self.0.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |usage| {
            Some(usage.saturating_sub(bytes))
        });
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// What a rewrite of the store carries from the old keyspaces to their successor.
pub struct Carried {
    host: HostId,
    last_id: u128,
    evictable: u64,
    usage: Arc<Usage>,
    /// How many of the next reopens fail, for the tests.
    #[cfg(test)]
    failing_reopens: Arc<std::sync::atomic::AtomicU32>,
}

/// The database and its keyspaces. See [`schema`] for their layout.
pub struct Keyspaces {
    db: OptimisticTxDatabase,
    records: OptimisticTxKeyspace,
    segment_records: OptimisticTxKeyspace,
    contents: OptimisticTxKeyspace,
    meta: OptimisticTxKeyspace,
    /// The host of every record that the store writes.
    host: HostId,
    /// The largest record id that the store holds or has handed out.
    last_id: Mutex<u128>,
    /// How many segments the store holds.
    segments: AtomicU64,
    /// Counts the commits that ended a segment, which are the commits that can give an
    /// eviction something new to remove.
    evictable: AtomicU64,
    dirty: DirtyFlag,
    usage: Arc<Usage>,
    /// How many stored contents the store has expanded.
    #[cfg(test)]
    expansions: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    failing_reopens: Arc<std::sync::atomic::AtomicU32>,
    #[cfg(test)]
    next_snapshot: Mutex<Option<SnapshotWrapper>>,
}

/// Takes the next snapshot with the function that it receives, for the tests.
#[cfg(test)]
type SnapshotWrapper = Box<dyn FnOnce(&dyn Fn() -> Snapshot) -> Snapshot + Send>;

impl Keyspaces {
    /// Opens the database, and fails when the database holds a format other than this
    /// daemon's.
    ///
    /// The open counts the stored segments, so it takes longer for a larger store.
    pub fn open(dir: impl AsRef<Path>, host: HostId) -> Result<Self, StoreError> {
        Self::reopen(dir, &Carried {
            host,
            last_id: 0,
            evictable: 0,
            usage: Arc::new(Usage(AtomicU64::new(0))),
            #[cfg(test)]
            failing_reopens: Arc::default(),
        })
    }

    /// Opens the database in `dir` as the successor of the keyspaces that `carried` came from.
    pub fn reopen(dir: impl AsRef<Path>, carried: &Carried) -> Result<Self, StoreError> {
        let Carried {
            host,
            last_id,
            evictable,
            ref usage,
            #[cfg(test)]
            ref failing_reopens,
        } = *carried;
        #[cfg(test)]
        if failing_reopens
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| count.checked_sub(1))
            .is_ok()
        {
            return Err(StoreError::Corrupt("a test fails this open"));
        }
        let usage = Arc::clone(usage);
        let db = OptimisticTxDatabase::builder(dir).open()?;
        let records = db.keyspace(schema::RECORDS, schema::compressed)?;
        let segment_records = db.keyspace(schema::SEGMENT_RECORDS, schema::compressed)?;
        let contents = db.keyspace(schema::CONTENTS, schema::uncompressed)?;
        let meta = db.keyspace(schema::META, schema::uncompressed)?;
        usage.0.store(db.disk_space()?, Ordering::Relaxed);
        let last_key =
            db.read_tx().last_key_value(&records).map(|guard| guard.key()).transpose()?;
        let stored_last_id = match last_key.map(|key| id_of(&key)).transpose() {
            Ok(id) => id.map_or(0, |id| id.as_u128()),
            Err(err) => {
                warn!(%err, "the newest desktop capture record has a malformed key");
                0
            }
        };
        let keyspaces = Self {
            db,
            records,
            segment_records,
            contents,
            meta,
            host,
            last_id: Mutex::new(stored_last_id.max(last_id)),
            segments: AtomicU64::new(0),
            evictable: AtomicU64::new(evictable),
            dirty: DirtyFlag::default(),
            usage,
            #[cfg(test)]
            expansions: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            failing_reopens: Arc::clone(failing_reopens),
            #[cfg(test)]
            next_snapshot: Mutex::new(None),
        };
        format::check(&keyspaces)?;
        let mut segments = 0;
        for segment in keyspaces.read().segments() {
            match segment {
                Ok(_) => segments += 1,
                Err(err) if err.is_damage() => {
                    warn!(%err, "stopped counting desktop capture segments at a damaged key");
                    break;
                }
                Err(err) => return Err(err),
            }
        }
        keyspaces.segments.store(segments, Ordering::Relaxed);
        Ok(keyspaces)
    }

    /// The store's running estimate of the bytes that it uses, which stays readable after the
    /// keyspaces close.
    pub fn usage(&self) -> Arc<Usage> {
        Arc::clone(&self.usage)
    }

    /// Fjall's estimate of the number of records.
    pub fn approximate_record_count(&self) -> u64 {
        u64::try_from(self.records.approximate_len()).unwrap_or(u64::MAX)
    }

    /// How many segments the store holds.
    pub fn segment_count(&self) -> u64 {
        self.segments.load(Ordering::Relaxed)
    }

    /// A number that changes whenever a commit ends a segment.
    pub fn evictable_generation(&self) -> u64 {
        self.evictable.load(Ordering::Acquire)
    }

    /// Makes every later record id larger than `id`.
    pub fn follow(&self, id: Uuid) {
        let mut last = self.last_id.lock();
        *last = (*last).max(id.as_u128());
    }

    /// A new record id, which is larger than every id that the store holds or has handed out.
    ///
    /// The id is a UUIDv7 of the current time. A clock that was set back between two daemons
    /// can read earlier than the newest stored id. In that case the id follows the newest id,
    /// so that key order stays the order of writes.
    pub fn next_id(&self) -> Uuid {
        let now = Uuid::now_v7().as_u128();
        let mut last = self.last_id.lock();
        *last = now.max(last.saturating_add(1));
        Uuid::from_u128(*last)
    }

    /// The bytes that the database's files take on disk.
    pub fn disk_space(&self) -> Result<u64, StoreError> {
        Ok(self.db.disk_space()?)
    }

    /// Copies every entry into a new database in `dir`, and persists the new database.
    ///
    /// The copy goes straight into the new database's tables, so that the new database's
    /// journal doesn't hold any of the copied values. The copy leaves out the note of an owed
    /// rewrite and the count of evicted bytes, because the copy is that rewrite.
    pub fn copy_to(&self, dir: &Path) -> Result<(), StoreError> {
        let copy = OptimisticTxDatabase::builder(dir).open()?;
        let snapshot = self.db.read_tx();
        let keyspaces: [(_, _, fn() -> KeyspaceCreateOptions); 4] = [
            (&self.records, schema::RECORDS, schema::compressed),
            (&self.segment_records, schema::SEGMENT_RECORDS, schema::compressed),
            (&self.contents, schema::CONTENTS, schema::uncompressed),
            (&self.meta, schema::META, schema::uncompressed),
        ];
        for (from, name, options) in keyspaces {
            let to = copy.keyspace(name, options)?;
            let mut ingestion = to.inner().start_ingestion()?;
            for guard in snapshot.iter(from) {
                let (key, value) = guard.into_inner()?;
                if name == schema::META
                    && [schema::REWRITE_OWED_KEY, schema::EVICTED_KEY].contains(&&*key)
                {
                    continue;
                }
                ingestion.write(key, value)?;
            }
            ingestion.finish()?;
        }
        drop(snapshot);
        copy.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    /// Closes the database, and returns what its successor carries on.
    pub fn close(self) -> Carried {
        Carried {
            host: self.host,
            last_id: *self.last_id.lock(),
            evictable: self.evictable.load(Ordering::Acquire),
            usage: self.usage,
            #[cfg(test)]
            failing_reopens: self.failing_reopens,
        }
    }

    /// Makes the next `count` opens of the successors of these keyspaces fail.
    #[cfg(test)]
    pub fn fail_reopens(&self, count: u32) {
        self.failing_reopens.store(count, Ordering::Relaxed);
    }

    pub fn read(&self) -> Reader<'_> {
        let take = || self.db.read_tx();
        #[cfg(test)]
        let wrap = self.next_snapshot.lock().take();
        #[cfg(test)]
        if let Some(wrap) = wrap {
            return Reader {
                keyspaces: self,
                snapshot: wrap(&take),
            };
        }
        Reader {
            keyspaces: self,
            snapshot: take(),
        }
    }

    /// Makes the next read call `wrap` with the function that takes the read's snapshot.
    #[cfg(test)]
    pub fn wrap_next_snapshot(
        &self,
        wrap: impl FnOnce(&dyn Fn() -> Snapshot) -> Snapshot + Send + 'static,
    ) {
        *self.next_snapshot.lock() = Some(Box::new(wrap));
    }

    pub fn write(&self) -> Result<Writer<'_>, StoreError> {
        Ok(Writer {
            keyspaces: self,
            tx: self.db.write_tx()?,
            inserted: 0,
            freed: 0,
            segments_added: 0,
            segments_removed: 0,
            evictable: false,
        })
    }

    /// Appends one record in a transaction of its own, and returns the record's id.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: the commit consumes the transaction"
    )]
    pub fn append(
        &self,
        segment: SegmentKey,
        at: Timestamp,
        body: BodyToStore<'_>,
    ) -> Result<Uuid, StoreError> {
        let mut writer = self.write()?;
        let id = writer.append(segment, at, body)?;
        writer.commit()?;
        Ok(id)
    }
}

impl Persist for Keyspaces {
    fn dirty(&self) -> &DirtyFlag {
        &self.dirty
    }

    fn persist(&self) -> fjall::Result<()> {
        self.db.persist(PersistMode::SyncAll)
    }
}

/// Runs `write` again when its commit conflicts with a concurrent write, up to three times in
/// all.
pub fn retry_on_conflict<T>(
    mut write: impl FnMut() -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let mut attempt = 1;
    loop {
        match write() {
            Err(StoreError::Conflict) if attempt < ATTEMPTS => attempt += 1,
            result => return result,
        }
    }
}

/// A stored record that has not been decoded.
pub struct RawRecord {
    pub id: Uuid,
    value: Slice,
}

impl RawRecord {
    /// Decodes the parts of the record that scans need.
    pub fn head(&self) -> Result<RecordHead, StoreError> {
        Ok(rmp_serde::from_slice(&self.value)?)
    }

    pub fn record(&self) -> Result<StoredRecord, StoreError> {
        Ok(rmp_serde::from_slice(&self.value)?)
    }

    /// The bytes that the record's entry in `records` takes.
    pub fn size(&self) -> u64 {
        entry_size(self.id.as_bytes(), &self.value)
    }
}

/// A record whose head decoded.
pub struct Headed {
    pub id: Uuid,
    pub head: RecordHead,
}

/// The first record of a segment.
pub struct Started {
    pub at: Timestamp,
    pub start: SegmentStart,
}

/// A stored content that has not been decoded.
pub struct ContentValue(Slice);

impl ContentValue {
    pub fn view(&self) -> Result<StoredContent<'_>, StoreError> {
        Ok(rmp_serde::from_slice(&self.0)?)
    }
}

/// One consistent snapshot of the keyspaces.
pub struct Reader<'a> {
    keyspaces: &'a Keyspaces,
    snapshot: Snapshot,
}

impl Reader<'_> {
    pub fn record(&self, id: Uuid) -> Result<Option<StoredRecord>, StoreError> {
        self.raw_record(id)?.map(|raw| raw.record()).transpose()
    }

    pub fn raw_record(&self, id: Uuid) -> Result<Option<RawRecord>, StoreError> {
        Ok(self
            .snapshot
            .get(&self.keyspaces.records, id.as_bytes())?
            .map(|value| RawRecord { id, value }))
    }

    /// Every record, in the order that they were written, undecoded.
    pub fn records(
        &self,
    ) -> impl DoubleEndedIterator<Item = Result<RawRecord, StoreError>> + use<> {
        self.records_from([0; 16])
    }

    /// The records from the key `from` on, in the order that they were written, undecoded.
    pub fn records_from(
        &self,
        from: [u8; 16],
    ) -> impl DoubleEndedIterator<Item = Result<RawRecord, StoreError>> + use<> {
        self.snapshot.range(&self.keyspaces.records, from..).map(|guard| {
            let (key, value) = guard.into_inner()?;
            Ok(RawRecord {
                id: id_of(&key)?,
                value,
            })
        })
    }

    /// The ids of a segment's records, in the order that they were written.
    pub fn segment_record_ids(
        &self,
        segment: SegmentKey,
    ) -> impl DoubleEndedIterator<Item = Result<Uuid, StoreError>> + use<> {
        self.snapshot.prefix(&self.keyspaces.segment_records, segment.0.as_bytes()).map(|guard| {
            schema::record_in_segment_key(&guard.key()?)
                .ok_or(StoreError::Corrupt("a segment record key is too short"))
        })
    }

    /// The heads of a segment's records after its start, in the order that they were written.
    pub fn heads_after_start(
        &self,
        segment: SegmentKey,
    ) -> impl DoubleEndedIterator<Item = Result<Headed, StoreError>> + use<'_> {
        self.segment_record_ids(segment)
            .filter(move |id| !id.as_ref().is_ok_and(|&id| id == segment.0))
            .map(|id| {
                let id = id?;
                Ok(Headed {
                    id,
                    head: self.listed_record(id)?.head()?,
                })
            })
    }

    /// Whether the store holds the segment.
    pub fn has_segment(&self, segment: SegmentKey) -> Result<bool, StoreError> {
        let key = schema::segment_record_key(segment, segment.0);
        Ok(self.snapshot.contains_key(&self.keyspaces.segment_records, key)?)
    }

    /// A record that a segment lists.
    fn listed_record(&self, id: Uuid) -> Result<RawRecord, StoreError> {
        self.raw_record(id)?.ok_or(StoreError::Corrupt("a segment lists a missing record"))
    }

    /// The head of the last record of a segment.
    pub fn last_head(&self, segment: SegmentKey) -> Result<Headed, StoreError> {
        let id = self
            .segment_record_ids(segment)
            .next_back()
            .ok_or(StoreError::Corrupt("a segment has no records"))??;
        Ok(Headed {
            id,
            head: self.listed_record(id)?.head()?,
        })
    }

    /// The head of the last record of a segment that is present and decodes. `None` when no
    /// record of the segment is intact.
    ///
    /// Each damaged record on the way is logged and skipped, so that maintenance can carry on
    /// past damage.
    pub fn last_intact_head(&self, segment: SegmentKey) -> Result<Option<Headed>, StoreError> {
        for id in self.segment_record_ids(segment).rev() {
            let head = id.and_then(|id| {
                let head = self.listed_record(id)?.head()?;
                Ok(Headed { id, head })
            });
            match head {
                Ok(head) => return Ok(Some(head)),
                Err(err) if err.is_damage() => {
                    warn!(%err, %segment, "skipped a damaged desktop capture record");
                }
                Err(err) => return Err(err),
            }
        }
        Ok(None)
    }

    pub fn started(&self, segment: SegmentKey) -> Result<Option<Started>, StoreError> {
        Ok(self.record(segment.0)?.and_then(|record| match record.body {
            StoredBody::SegmentStarted(start) => Some(Started {
                at: record.at,
                start,
            }),
            _ => None,
        }))
    }

    /// Every segment, oldest first, with one seek per segment.
    pub fn segments(&self) -> impl Iterator<Item = Result<SegmentKey, StoreError>> + use<'_> {
        self.segments_from([0; 16])
    }

    /// The segments from the key `from` on, oldest first, with one seek per segment.
    ///
    /// A malformed key yields an error, and the walk carries on after it.
    pub fn segments_from(
        &self,
        from: [u8; 16],
    ) -> impl Iterator<Item = Result<SegmentKey, StoreError>> + use<'_> {
        self.segment_entries_from(from).map(|entry| match entry? {
            SegmentEntry::Segment(segment) => Ok(segment),
            SegmentEntry::Malformed(_) => Err(StoreError::Corrupt(MALFORMED_SEGMENT_KEY)),
        })
    }

    /// The segments from the key `from` on, oldest first, with one seek per segment, and each
    /// malformed key on the way.
    pub fn segment_entries_from(
        &self,
        from: [u8; 16],
    ) -> impl Iterator<Item = Result<SegmentEntry, StoreError>> + use<'_> {
        let mut from = Some(Bound::Included(Slice::from(from)));
        std::iter::from_fn(move || {
            let range = (from.take()?, Bound::Unbounded);
            let guard = self.snapshot.range(&self.keyspaces.segment_records, range).next()?;
            let key = match guard.key() {
                Ok(key) => key,
                Err(err) => return Some(Err(err.into())),
            };
            let Some(segment) = schema::segment_in_segment_key(&key) else {
                from = Some(Bound::Excluded(key.clone()));
                return Some(Ok(SegmentEntry::Malformed(key)));
            };
            from = schema::key_after(segment.0).map(|next| Bound::Included(Slice::from(next)));
            Some(Ok(SegmentEntry::Segment(segment)))
        })
    }

    /// Every segment, newest first, with one seek per segment.
    pub fn segments_newest_first(
        &self,
    ) -> impl Iterator<Item = Result<SegmentKey, StoreError>> + use<'_> {
        let mut before = Some(std::ops::Bound::Unbounded);
        std::iter::from_fn(move || {
            // The last key below `before` is the last record of the segment before it.
            let range = (std::ops::Bound::Unbounded, before?);
            let guard = self.snapshot.range(&self.keyspaces.segment_records, range).next_back()?;
            let segment = guard.key().map_err(StoreError::from).and_then(|key| segment_in(&key));
            before = segment
                .as_ref()
                .ok()
                .map(|segment| std::ops::Bound::Excluded(*segment.0.as_bytes()));
            Some(segment)
        })
    }

    /// The segment of each record in `records` that a segment lists.
    ///
    /// The lookup reads the whole index of segment records, so it suits only a few records,
    /// such as the damaged records that a deletion meets.
    pub fn segments_listing(
        &self,
        records: &BTreeSet<Uuid>,
    ) -> Result<BTreeMap<Uuid, SegmentKey>, StoreError> {
        let mut segments = BTreeMap::new();
        for guard in self.snapshot.iter(&self.keyspaces.segment_records) {
            let key = guard.key()?;
            if let (Some(segment), Some(record)) =
                (schema::segment_in_segment_key(&key), schema::record_in_segment_key(&key))
                && records.contains(&record)
            {
                segments.insert(record, segment);
            }
        }
        Ok(segments)
    }

    /// The format of the stored data. `None` for a new database, which doesn't note a format
    /// yet.
    pub fn format(&self) -> Result<Option<u32>, StoreError> {
        let Some(value) = self.snapshot.get(&self.keyspaces.meta, schema::FORMAT_KEY)? else {
            return Ok(None);
        };
        let bytes = <[u8; 4]>::try_from(&*value)
            .map_err(|_| StoreError::Corrupt("the format of the stored data is malformed"))?;
        Ok(Some(u32::from_be_bytes(bytes)))
    }

    /// Whether a removal waits for a rewrite of the store.
    pub fn owes_rewrite(&self) -> Result<bool, StoreError> {
        Ok(self.snapshot.contains_key(&self.keyspaces.meta, schema::REWRITE_OWED_KEY)?)
    }

    /// Whether the search index can still hold a segment that the store removed.
    pub fn owes_index_removal(&self) -> Result<bool, StoreError> {
        Ok(self.snapshot.contains_key(&self.keyspaces.meta, schema::INDEX_REMOVAL_OWED_KEY)?)
    }

    /// The bytes that evictions freed since the last rewrite.
    pub fn evicted(&self) -> Result<u64, StoreError> {
        let value = self.snapshot.get(&self.keyspaces.meta, schema::EVICTED_KEY)?;
        Ok(value.map_or(0, |value| decode_evicted(&value)))
    }

    pub fn content(&self, key: ContentKey) -> Result<ContentValue, StoreError> {
        self.snapshot
            .get(&self.keyspaces.contents, schema::content_key(key))?
            .map(ContentValue)
            .ok_or(StoreError::Corrupt("a content is missing"))
    }

    /// Counts an expansion of a stored content, for the tests.
    #[cfg_attr(not(test), expect(clippy::unused_self, reason = "only the tests count expansions"))]
    pub fn count_expansion(&self) {
        #[cfg(test)]
        self.keyspaces.expansions.fetch_add(1, Ordering::Relaxed);
    }
}

/// The bytes of a value of [`schema::EVICTED_KEY`]. A malformed value counts as nothing
/// evicted, which delays a rewrite until later evictions add up again.
fn decode_evicted(value: &[u8]) -> u64 {
    <[u8; 8]>::try_from(value).map_or_else(
        |_| {
            warn!("the desktop capture store's count of evicted bytes is malformed");
            0
        },
        u64::from_be_bytes,
    )
}

/// The record id of a `records` key.
fn id_of(key: &[u8]) -> Result<Uuid, StoreError> {
    Uuid::from_slice(key).map_err(|_| StoreError::Corrupt("a record key is malformed"))
}

/// The damage of a `segment_records` key that doesn't hold a segment.
const MALFORMED_SEGMENT_KEY: &str = "a segment record key is too short";

/// The segment in a `segment_records` key.
fn segment_in(key: &[u8]) -> Result<SegmentKey, StoreError> {
    schema::segment_in_segment_key(key).ok_or(StoreError::Corrupt(MALFORMED_SEGMENT_KEY))
}

/// One entry of a walk over the segments.
pub enum SegmentEntry {
    Segment(SegmentKey),
    /// A `segment_records` key that is too short to hold a segment.
    Malformed(Slice),
}

/// One write transaction.
pub struct Writer<'a> {
    keyspaces: &'a Keyspaces,
    tx: OptimisticWriteTx,
    /// The bytes that the transaction's inserts take.
    inserted: u64,
    /// The bytes that the transaction's removals free.
    freed: u64,
    segments_added: u64,
    segments_removed: u64,
    /// Whether the transaction ends a segment.
    evictable: bool,
}

impl Writer<'_> {
    /// Adds a record of the store's host under a new id, and returns the id.
    pub fn append(
        &mut self,
        segment: SegmentKey,
        at: Timestamp,
        body: BodyToStore<'_>,
    ) -> Result<Uuid, StoreError> {
        let id = self.keyspaces.next_id();
        self.insert_record(id, segment, at, body)?;
        Ok(id)
    }

    /// Adds the start of a new segment, and returns the segment's key.
    pub fn start_segment(
        &mut self,
        at: Timestamp,
        start: &SegmentStart,
    ) -> Result<SegmentKey, StoreError> {
        let id = self.keyspaces.next_id();
        // A segment's key is the id of its start record.
        let segment = SegmentKey(id);
        self.insert_record(id, segment, at, StoredBody::SegmentStarted(start))?;
        Ok(segment)
    }

    /// Notes the format of the stored data.
    pub fn set_format(&mut self, format: u32) {
        self.tx.insert(&self.keyspaces.meta, schema::FORMAT_KEY, format.to_be_bytes());
    }

    /// Notes that the store owes a rewrite, which stays noted until a rewrite succeeds.
    pub fn owe_rewrite(&mut self) {
        self.tx.insert(&self.keyspaces.meta, schema::REWRITE_OWED_KEY, []);
    }

    /// Adds `bytes` to the bytes that evictions freed since the last rewrite.
    pub fn add_evicted(&mut self, bytes: u64) -> Result<(), StoreError> {
        let meta = &self.keyspaces.meta;
        let evicted = self.tx.get(meta, schema::EVICTED_KEY)?;
        let evicted = evicted.map_or(0, |value| decode_evicted(&value)).saturating_add(bytes);
        self.tx.insert(meta, schema::EVICTED_KEY, evicted.to_be_bytes());
        Ok(())
    }

    /// Notes that the search index holds only the segments that the store holds.
    pub fn settle_index_removal(&mut self) {
        self.tx.remove(&self.keyspaces.meta, schema::INDEX_REMOVAL_OWED_KEY);
    }

    fn insert_record(
        &mut self,
        id: Uuid,
        segment: SegmentKey,
        at: Timestamp,
        body: BodyToStore<'_>,
    ) -> Result<(), StoreError> {
        match body {
            StoredBody::SegmentStarted(_) => self.segments_added += 1,
            StoredBody::SegmentEnded => self.evictable = true,
            StoredBody::Observed { .. } => {}
        }
        let record = RecordToStore {
            host: self.keyspaces.host,
            segment,
            at,
            body,
        };
        let value = rmp_serde::to_vec_named(&record)?;
        self.inserted += entry_size(id.as_bytes(), &value);
        self.tx.insert(&self.keyspaces.records, id.into_bytes(), value);
        let key = schema::segment_record_key(segment, id);
        self.inserted += entry_size(&key, &[]);
        self.tx.insert(&self.keyspaces.segment_records, key, []);
        Ok(())
    }

    pub fn has_content(&self, key: ContentKey) -> Result<bool, StoreError> {
        Ok(self.tx.contains_key(&self.keyspaces.contents, schema::content_key(key))?)
    }

    pub fn insert_content(
        &mut self,
        key: ContentKey,
        content: &StoredContent<'_>,
    ) -> Result<(), StoreError> {
        let key = schema::content_key(key);
        let value = rmp_serde::to_vec_named(content)?;
        self.inserted += entry_size(&key, &value);
        self.tx.insert(&self.keyspaces.contents, key, value);
        Ok(())
    }

    /// Whether the record `id` exists, as this transaction sees it.
    ///
    /// The read makes the commit fail with [`StoreError::Conflict`] when another transaction
    /// has removed the record since this transaction began.
    pub fn has_record(&self, id: Uuid) -> Result<bool, StoreError> {
        Ok(self.tx.contains_key(&self.keyspaces.records, id.as_bytes())?)
    }

    /// Whether the segment exists, as this transaction sees it.
    ///
    /// The read makes the commit fail with [`StoreError::Conflict`] when another transaction
    /// has removed the segment since this transaction began.
    pub fn has_segment(&self, segment: SegmentKey) -> Result<bool, StoreError> {
        // A segment's key is the id of its start record, so a segment lists its own key for as
        // long as it exists, even when the start record itself is damaged.
        let key = schema::segment_record_key(segment, segment.0);
        Ok(self.tx.contains_key(&self.keyspaces.segment_records, key)?)
    }

    /// Removes a `segment_records` key that is too short to hold a segment.
    pub fn remove_malformed_segment_key(&mut self, key: Slice) {
        self.freed += entry_size(&key, &[]);
        self.tx.remove(&self.keyspaces.segment_records, key);
    }

    /// Removes one record and leaves the rest of its segment.
    pub fn remove_record(&mut self, record: &RawRecord) {
        self.freed += record.size();
        self.tx.remove(&self.keyspaces.records, record.id.into_bytes());
    }

    /// Removes every entry of a segment, as `reader` sees the segment, unless another
    /// transaction has removed the segment since `reader` was taken.
    ///
    /// The removal doesn't decode any record, so the removal also removes a damaged segment.
    pub fn remove_segment_if_present(
        &mut self,
        reader: &Reader<'_>,
        segment: SegmentKey,
    ) -> Result<Removal, StoreError> {
        // The reader's snapshot can still hold a segment that another transaction has removed
        // since, so the check reads the segment in this transaction.
        if !self.has_segment(segment)? {
            return Ok(Removal::AlreadyGone);
        }
        let before = self.freed;
        self.remove_segment(reader, segment)?;
        Ok(Removal::Removed {
            freed: self.freed - before,
        })
    }

    /// Removes every entry of a segment, as `reader` sees the segment.
    fn remove_segment(
        &mut self,
        reader: &Reader<'_>,
        segment: SegmentKey,
    ) -> Result<(), StoreError> {
        for guard in reader.snapshot.prefix(&self.keyspaces.segment_records, segment.0.as_bytes()) {
            let key = guard.key()?;
            self.freed += entry_size(&key, &[]);
            if let Some(id) = schema::record_in_segment_key(&key)
                && let Some(record) = reader.raw_record(id)?
            {
                self.freed += record.size();
                self.tx.remove(&self.keyspaces.records, id.into_bytes());
            }
            self.tx.remove(&self.keyspaces.segment_records, key);
        }
        for guard in reader.snapshot.prefix(&self.keyspaces.contents, segment.0.as_bytes()) {
            let (key, value) = guard.into_inner()?;
            self.freed += entry_size(&key, &value);
            self.tx.remove(&self.keyspaces.contents, key);
        }
        self.segments_removed += 1;
        Ok(())
    }

    /// The bytes that the transaction's removals free.
    pub const fn freed(&self) -> u64 {
        self.freed
    }

    /// Commits the transaction. A transaction that removes a segment also notes that the
    /// search index owes the segment's removal.
    pub fn commit(mut self) -> Result<(), StoreError> {
        let keyspaces = self.keyspaces;
        if self.segments_removed > 0 {
            self.tx.insert(&keyspaces.meta, schema::INDEX_REMOVAL_OWED_KEY, []);
        }
        self.tx.commit()?.map_err(|fjall::Conflict| StoreError::Conflict)?;
        keyspaces.dirty.mark();
        keyspaces.usage.add(self.inserted);
        keyspaces.usage.sub(self.freed);
        keyspaces.segments.fetch_add(self.segments_added, Ordering::Relaxed);
        let _ = keyspaces.segments.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            Some(count.saturating_sub(self.segments_removed))
        });
        if self.evictable {
            keyspaces.evictable.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }
}

/// What [`Writer::remove_segment_if_present`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// The transaction removes the segment, which frees `freed` bytes.
    Removed {
        freed: u64,
    },
    /// Another transaction, such as a deletion or an eviction, removed the segment first.
    AlreadyGone,
}

/// The bytes that one entry takes before compression.
fn entry_size(key: &[u8], value: &[u8]) -> u64 {
    u64::try_from(key.len() + value.len()).unwrap_or(u64::MAX)
}

/// A stored content, for the tests to look at how the store keeps a tree.
#[cfg(test)]
pub struct ContentEntry {
    pub key: ContentKey,
    pub base: Option<atuin_desktop_capture::ContentHash>,
    /// The bytes of the stored value.
    pub size: usize,
}

#[cfg(test)]
impl Keyspaces {
    pub fn stored_contents(&self) -> Vec<ContentEntry> {
        self.db
            .read_tx()
            .iter(&self.contents)
            .map(|guard| {
                let (key, value) = guard.into_inner().expect("a content");
                let content: StoredContent<'_> =
                    rmp_serde::from_slice(&value).expect("a stored content");
                ContentEntry {
                    key: schema::parse_content_key(&key).expect("a content key"),
                    base: content.base,
                    size: value.len(),
                }
            })
            .collect()
    }

    /// The bytes of the tables of the contents keyspace, which leave out its memtable.
    pub fn contents_table_space(&self) -> u64 {
        self.contents.inner().disk_space()
    }

    /// How many stored contents the store has expanded since it opened.
    pub fn expansions(&self) -> usize {
        self.expansions.load(Ordering::Relaxed)
    }

    /// Whether every keyspace is empty.
    pub fn is_empty(&self) -> bool {
        let snapshot = self.db.read_tx();
        [&self.records, &self.segment_records, &self.contents]
            .into_iter()
            .all(|keyspace| snapshot.iter(keyspace).next().is_none())
    }

    pub fn record_ids(&self, segment: SegmentKey) -> Vec<Uuid> {
        self.read().segment_record_ids(segment).map(|id| id.expect("a segment record")).collect()
    }

    /// Writes `value` under the record key `id`, in place of a stored record.
    pub fn put_raw_record(&self, id: Uuid, value: &[u8]) {
        let mut tx = self.db.write_tx().expect("a transaction");
        tx.insert(&self.records, id.into_bytes(), value);
        tx.commit().expect("a commit").expect("no conflict");
    }

    /// Writes `value` as the store's format.
    pub fn put_raw_format(&self, value: &[u8]) {
        let mut tx = self.db.write_tx().expect("a transaction");
        tx.insert(&self.meta, schema::FORMAT_KEY, value);
        tx.commit().expect("a commit").expect("no conflict");
    }

    pub fn put_raw_segment_record_key(&self, key: &[u8]) {
        let mut tx = self.db.write_tx().expect("a transaction");
        tx.insert(&self.segment_records, key, []);
        tx.commit().expect("a commit").expect("no conflict");
    }

    /// Writes `value` under the content key `key`.
    pub fn put_raw_content(&self, key: ContentKey, value: &[u8]) {
        let mut tx = self.db.write_tx().expect("a transaction");
        tx.insert(&self.contents, schema::content_key(key), value);
        tx.commit().expect("a commit").expect("no conflict");
    }

    /// Writes a record under the id `id`, outside the store's own order of ids.
    pub fn put_record(&self, id: Uuid, record: &StoredRecord) {
        let mut tx = self.db.write_tx().expect("a transaction");
        let value = rmp_serde::to_vec_named(record).expect("a record encodes");
        tx.insert(&self.records, id.into_bytes(), value);
        tx.insert(&self.segment_records, schema::segment_record_key(record.segment, id), []);
        tx.commit().expect("a commit").expect("no conflict");
    }

    pub fn host(&self) -> HostId {
        self.host
    }
}
