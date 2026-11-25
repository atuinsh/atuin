//! Deletes the segments that match a filter, or one segment.

use tracing::warn;
use uuid::Uuid;

use super::keyspaces::{Keyspaces, Removal, retry_on_conflict};
use super::model::{DeleteFailed, Deleted, SegmentFilter, SegmentKey, Selection, StoreError};
use super::open::OpenSegments;
use super::query::{Selected, View};

/// Deletes what `selection` names.
///
/// A filter selects every segment that [`View::segments`] lists for it, whole. The filter also
/// selects each damaged segment or record that it can select, unless the damage hides whether
/// the filter selects it. [`Deleted::undecided`] names what stays for that reason. A segment
/// key selects that segment.
///
/// Each transaction that removes something also notes that the store owes a rewrite, so that a
/// rewrite that doesn't happen at once still happens later.
///
/// A failure carries what the deletion removed before it. The removed segments leave `open`
/// whether or not the deletion fails.
pub fn delete(
    keyspaces: &Keyspaces,
    open: &OpenSegments,
    selection: &Selection,
) -> Result<Deleted, DeleteFailed> {
    let mut deleted = Deleted::default();
    let result = match selection {
        Selection::Filter(filter) => remove_matching(keyspaces, open, filter, &mut deleted),
        Selection::Segment(key) => remove_one(keyspaces, *key, &mut deleted),
    };
    open.remove_all(deleted.segments.iter().copied());
    match result {
        Ok(()) => Ok(deleted),
        Err(source) => Err(DeleteFailed {
            deleted: Box::new(deleted),
            source,
        }),
    }
}

fn remove_matching(
    keyspaces: &Keyspaces,
    open: &OpenSegments,
    filter: &SegmentFilter,
    deleted: &mut Deleted,
) -> Result<(), StoreError> {
    let Selected {
        segments,
        orphans,
        undecided,
        damaged_starts: _,
    } = View::new(keyspaces, open).selected_by(filter)?;
    for key in segments {
        remove_one(keyspaces, key, deleted)?;
    }
    for id in orphans {
        if let Removal::Removed { .. } = remove_record(keyspaces, id)? {
            deleted.orphans.push(id);
        }
    }
    if !undecided.is_empty() {
        warn!(
            count = undecided.len(),
            "a deletion left damaged desktop capture segments or records in the store, because \
             their damage hides whether the deletion's filter selects them"
        );
    }
    deleted.undecided = undecided;
    Ok(())
}

fn remove_one(
    keyspaces: &Keyspaces,
    key: SegmentKey,
    deleted: &mut Deleted,
) -> Result<(), StoreError> {
    if let Removal::Removed { .. } = remove_segment(keyspaces, key)? {
        deleted.segments.push(key);
    }
    Ok(())
}

/// Removes one segment, trying again when a concurrent write conflicts with the removal.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: the commit consumes the transaction"
)]
fn remove_segment(keyspaces: &Keyspaces, segment: SegmentKey) -> Result<Removal, StoreError> {
    retry_on_conflict(|| {
        let reader = keyspaces.read();
        let mut writer = keyspaces.write()?;
        let removal = writer.remove_segment_if_present(&reader, segment)?;
        drop(reader);
        if let Removal::Removed { .. } = removal {
            writer.owe_rewrite();
            writer.commit()?;
        }
        Ok(removal)
    })
}

/// Removes one record that no segment lists, trying again when a concurrent write conflicts
/// with the removal.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: the commit consumes the transaction"
)]
fn remove_record(keyspaces: &Keyspaces, id: Uuid) -> Result<Removal, StoreError> {
    retry_on_conflict(|| {
        let Some(record) = keyspaces.read().raw_record(id)? else {
            return Ok(Removal::AlreadyGone);
        };
        let mut writer = keyspaces.write()?;
        // The check makes the commit conflict with another transaction that removes the record.
        if !writer.has_record(id)? {
            return Ok(Removal::AlreadyGone);
        }
        writer.remove_record(&record);
        writer.owe_rewrite();
        let freed = writer.freed();
        writer.commit()?;
        Ok(Removal::Removed { freed })
    })
}
