//! Deletes the oldest segments to bring the store under its size limit.

use parking_lot::Mutex;
use tracing::warn;

use super::keyspaces::{Keyspaces, Reader, Removal, SegmentEntry, Writer};
use super::model::{Evicted, SegmentKey, StoreError};
use super::schema::{self, BodyKind};

/// The most segments that one transaction of an eviction removes.
const SEGMENTS_PER_COMMIT: usize = 64;

/// Evicts the oldest segments, and skips the scan when nothing new can be evicted.
#[derive(Debug, Default)]
pub struct Evictor {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// The keyspaces' evictable generation when an eviction last scanned the whole store and
    /// found nothing to remove.
    fruitless_at: Option<u64>,
    /// The key at which the next eviction starts its walk of the segments.
    ///
    /// Every segment before the key is gone, so the walk doesn't step over the removed entries
    /// that the store keeps until a compaction. New segments get later keys, so no segment
    /// appears before the key.
    resume_from: [u8; 16],
}

/// What one scan of the store removed.
struct Scan {
    evicted: Evicted,
    /// Whether the scan reached the end of the store.
    exhausted: bool,
    /// The key at which the next scan starts.
    resume_from: [u8; 16],
}

impl Evictor {
    /// Deletes the oldest ended segments until they add up to at least `bytes` or until none is
    /// left.
    ///
    /// A segment without an end is kept, because a helper run or the sweep of open segments
    /// may still write to it. A removal that conflicts with a concurrent deletion ends the
    /// eviction early, and the next eviction carries on. A segment whose records are damaged
    /// is removed whole. When no ended segment is left, the eviction also removes each record
    /// that doesn't decode.
    pub fn evict(&self, keyspaces: &Keyspaces, bytes: u64) -> Result<Evicted, StoreError> {
        // Only a commit that ends a segment gives an eviction something new to remove, so an
        // eviction that found nothing stays fruitless until then. The generation is read before
        // the scan, so that a commit during the scan makes the next eviction scan again.
        let generation = keyspaces.evictable_generation();
        let resume_from = {
            let state = self.state.lock();
            if state.fruitless_at == Some(generation) {
                return Ok(Evicted::default());
            }
            state.resume_from
        };
        let mut scan = scan(keyspaces, resume_from, bytes)?;
        if scan.exhausted && scan.evicted.freed < bytes {
            scan.evicted.freed += remove_damaged_records(keyspaces)?;
        }
        {
            let mut state = self.state.lock();
            state.resume_from = scan.resume_from;
            if scan.exhausted && scan.evicted.freed == 0 {
                state.fruitless_at = Some(generation);
            }
        }
        Ok(scan.evicted)
    }
}

/// The removals of one transaction of an eviction.
struct Batch<'a> {
    writer: Writer<'a>,
    segments: Vec<SegmentKey>,
}

impl<'a> Batch<'a> {
    fn new(keyspaces: &'a Keyspaces) -> Result<Self, StoreError> {
        Ok(Self {
            writer: keyspaces.write()?,
            segments: Vec::new(),
        })
    }

    /// Commits the batch, and adds what it removed to `evicted`.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: the commit consumes the transaction"
    )]
    fn commit(self, evicted: &mut Evicted) -> Result<(), StoreError> {
        let mut writer = self.writer;
        let freed = writer.freed();
        if freed > 0 {
            writer.add_evicted(freed)?;
        }
        writer.commit()?;
        evicted.freed += freed;
        evicted.segments.extend(self.segments);
        Ok(())
    }
}

/// Walks the segments from the key `from` on, oldest first, and removes the ended ones until
/// they free `bytes`.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: the iterator borrows the reader until the scan ends"
)]
fn scan(keyspaces: &Keyspaces, from: [u8; 16], bytes: u64) -> Result<Scan, StoreError> {
    let reader = keyspaces.read();
    let mut evicted = Evicted::default();
    let mut batch = Batch::new(keyspaces)?;
    let mut first_kept = None;
    let mut next = from;
    let mut exhausted = true;
    for entry in reader.segment_entries_from(from) {
        if evicted.freed + batch.writer.freed() >= bytes {
            exhausted = false;
            break;
        }
        let segment = match entry? {
            SegmentEntry::Segment(segment) => segment,
            SegmentEntry::Malformed(key) => {
                warn!(?key, "evicting a malformed desktop capture segment key");
                batch.writer.remove_malformed_segment_key(key);
                continue;
            }
        };
        next = schema::key_after(segment.0).unwrap_or([0xff; 16]);
        if !is_ended(&reader, segment)? {
            first_kept.get_or_insert(*segment.0.as_bytes());
            continue;
        }
        if let Removal::Removed { .. } = batch.writer.remove_segment_if_present(&reader, segment)? {
            batch.segments.push(segment);
        }
        if batch.segments.len() >= SEGMENTS_PER_COMMIT {
            let full = std::mem::replace(&mut batch, Batch::new(keyspaces)?);
            match full.commit(&mut evicted) {
                Ok(()) => {}
                Err(StoreError::Conflict) => return Ok(ended_early(evicted, from)),
                Err(err) => return Err(err),
            }
        }
    }
    match batch.commit(&mut evicted) {
        Ok(()) => {}
        Err(StoreError::Conflict) => return Ok(ended_early(evicted, from)),
        Err(err) => return Err(err),
    }
    Ok(Scan {
        evicted,
        exhausted,
        resume_from: first_kept.unwrap_or(next),
    })
}

/// A scan that a conflicting deletion ended. The next scan starts where this one did.
fn ended_early(evicted: Evicted, from: [u8; 16]) -> Scan {
    Scan {
        evicted,
        exhausted: false,
        resume_from: from,
    }
}

/// Removes the records that don't decode, and returns the bytes that the removals freed.
///
/// A conflict with a concurrent deletion leaves the records for the next eviction.
fn remove_damaged_records(keyspaces: &Keyspaces) -> Result<u64, StoreError> {
    let reader = keyspaces.read();
    let mut writer = keyspaces.write()?;
    for raw in reader.records() {
        let raw = raw?;
        let Err(err) = raw.head() else {
            continue;
        };
        if !err.is_damage() {
            return Err(err);
        }
        // The reader's snapshot can still hold a record that a deletion has removed since.
        if writer.has_record(raw.id)? {
            warn!(%err, record = %raw.id, "evicting a damaged desktop capture record");
            writer.remove_record(&raw);
        }
    }
    drop(reader);
    let freed = writer.freed();
    if freed > 0 {
        writer.add_evicted(freed)?;
    }
    match writer.commit() {
        Ok(()) => Ok(freed),
        Err(StoreError::Conflict) => Ok(0),
        Err(err) => Err(err),
    }
}

/// Whether a segment has ended. A segment whose records are all damaged counts as ended, so
/// that an eviction removes it.
fn is_ended(reader: &Reader<'_>, segment: SegmentKey) -> Result<bool, StoreError> {
    let last = reader.last_intact_head(segment)?;
    Ok(last.is_none_or(|last| last.head.body == BodyKind::SegmentEnded))
}
