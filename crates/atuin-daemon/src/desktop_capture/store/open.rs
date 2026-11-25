//! The segments that this process has started and not ended.
//!
//! A segment that the store holds without an end reads as open when it is in [`OpenSegments`].
//! Otherwise the segment belongs to an earlier process or to a helper run that is over, and it
//! reads as ended at its last record. That is right only when each read sees the keyspaces and
//! the open segments at the same point. Three rules make that so:
//!
//! - A write adds a segment before the write commits the segment's start.
//! - A write removes a segment after the write commits the segment's end, or after it removes
//!   the segment from the store.
//! - A read takes its snapshot of the keyspaces and copies the open segments in one hold of the
//!   lock. See [`OpenSegments::snapshot`].
//!
//! Take a segment that a read's snapshot holds without an end, and that this process started.
//! The write added the segment before the snapshot, because the write committed the start
//! before the snapshot. The write can't remove the segment between the snapshot and the copy,
//! because the read holds the lock. A removal before the snapshot follows a commit before the
//! snapshot, so the snapshot would hold the end. The removal therefore comes after the copy,
//! and the copy holds the segment.
//!
//! Writes commit outside the lock. A read holds the lock only while it opens a snapshot and
//! copies the open segments, and opening a snapshot doesn't wait for commits. A write therefore
//! waits for a read only briefly.

use std::collections::BTreeSet;

use parking_lot::Mutex;

use super::keyspaces::{Keyspaces, Reader};
use super::model::SegmentKey;

#[derive(Debug, Default)]
pub struct OpenSegments(Mutex<State>);

#[derive(Debug, Default)]
struct State {
    keys: BTreeSet<SegmentKey>,
    /// Whether the segments that an earlier process left open have been ended.
    swept: bool,
}

/// A snapshot of the keyspaces with the segments that were open when the snapshot was taken.
pub struct OpenSnapshot<'k> {
    pub reader: Reader<'k>,
    pub open: BTreeSet<SegmentKey>,
}

impl OpenSegments {
    /// Adds a segment. Add it before committing its start.
    pub fn insert(&self, key: SegmentKey) {
        self.0.lock().keys.insert(key);
    }

    /// Removes a segment. Remove it after committing its end, or after a failed commit of its
    /// start.
    pub fn remove(&self, key: SegmentKey) {
        self.0.lock().keys.remove(&key);
    }

    /// Removes segments. Remove them after committing their ends or their removal.
    pub fn remove_all(&self, keys: impl IntoIterator<Item = SegmentKey>) {
        let mut state = self.0.lock();
        for key in keys {
            state.keys.remove(&key);
        }
    }

    /// Takes a snapshot of `keyspaces`, with a copy of the open segments.
    pub fn snapshot<'k>(&self, keyspaces: &'k Keyspaces) -> OpenSnapshot<'k> {
        let state = self.0.lock();
        let reader = keyspaces.read();
        OpenSnapshot {
            reader,
            open: state.keys.clone(),
        }
    }

    /// Adds every open segment to `keys`.
    pub fn copy_into(&self, keys: &mut BTreeSet<SegmentKey>) {
        keys.extend(self.0.lock().keys.iter());
    }

    pub fn is_swept(&self) -> bool {
        self.0.lock().swept
    }

    pub fn mark_swept(&self) {
        self.0.lock().swept = true;
    }
}
