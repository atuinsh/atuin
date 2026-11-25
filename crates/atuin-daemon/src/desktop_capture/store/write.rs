//! The write path: the transactions that start, observe, and end segments.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use atuin_desktop_capture::{
    Background, Content, Dialog, Notification, Segment, SegmentId, Snapshot, SnapshotContent,
    Trigger,
};
use tracing::warn;

use super::delta;
use super::keyspaces::{Keyspaces, Reader, Writer};
use super::model::{ContentKey, IndexItem, SegmentInfo, SegmentKey, SegmentKind, StoreError};
use super::open::OpenSegments;
use super::run::{BackgroundEnd, OpenSegment, Unstarted};
use super::schema::{BodyKind, SegmentStart, StoredBody, StoredContent};
use super::tree::Expanded;

/// One snapshot of a segment's window, on its way to the store.
pub struct Observation {
    at: SystemTime,
    /// `None` for a dialog, which capture reads when it appears.
    trigger: Option<Trigger>,
    content: SnapshotContent,
}

impl From<Snapshot> for Observation {
    fn from(snapshot: Snapshot) -> Self {
        Self {
            at: snapshot.at,
            trigger: Some(snapshot.trigger),
            content: snapshot.content,
        }
    }
}

/// A record that starts a segment and makes its first observation.
pub struct FirstObservation {
    at: SystemTime,
    start: SegmentStart,
    observation: Observation,
}

impl From<Dialog> for FirstObservation {
    fn from(dialog: Dialog) -> Self {
        Self::of_one_snapshot(
            SegmentKind::Dialog,
            dialog.app,
            dialog.title,
            dialog.at,
            dialog.content,
        )
    }
}

impl From<Notification> for FirstObservation {
    fn from(notification: Notification) -> Self {
        Self::of_one_snapshot(
            SegmentKind::Notification,
            notification.app,
            notification.title,
            notification.at,
            notification.content,
        )
    }
}

impl FirstObservation {
    pub const fn at(&self) -> SystemTime {
        self.at
    }

    pub const fn start(&self) -> &SegmentStart {
        &self.start
    }

    pub fn into_observation(self) -> Observation {
        self.observation
    }

    /// The first observation of a segment of `kind`, without a URL or a trigger.
    fn of_one_snapshot(
        kind: SegmentKind,
        app: String,
        title: String,
        at: SystemTime,
        content: Content,
    ) -> Self {
        Self {
            at,
            start: SegmentStart {
                kind,
                app,
                title,
                url: None,
            },
            observation: Observation {
                at,
                trigger: None,
                content: SnapshotContent::Changed(content),
            },
        }
    }
}

impl From<Background> for FirstObservation {
    fn from(read: Background) -> Self {
        Self {
            at: read.at,
            start: SegmentStart {
                kind: SegmentKind::Background,
                app: read.app,
                title: read.title,
                url: read.url,
            },
            observation: Observation {
                at: read.at,
                trigger: Some(read.trigger),
                content: SnapshotContent::Changed(read.content),
            },
        }
    }
}

/// A record that starts an active segment.
pub struct ActiveStart {
    /// The helper's id of the segment.
    pub id: SegmentId,
    pub start: Unstarted,
}

impl From<Segment> for ActiveStart {
    fn from(segment: Segment) -> Self {
        Self {
            id: segment.id,
            start: Unstarted {
                at: segment.started,
                start: SegmentStart {
                    kind: SegmentKind::Active,
                    app: segment.app,
                    title: segment.title,
                    url: segment.url,
                },
            },
        }
    }
}

/// An observation that is ready to store.
pub struct PendingObservation {
    segment: SegmentKey,
    observation: Observation,
    /// The base of the content when the content is new and stored as a delta.
    base: Option<Arc<Expanded>>,
}

/// What a write of an observation stored.
pub enum Observed {
    /// The observation and its content, which the segment did not hold yet.
    New(NewContent),
    /// The observation of a content that the segment holds already.
    Known,
    /// Nothing: the observation found an unchanged content that the segment does not hold.
    ///
    /// An unchanged snapshot doesn't carry a tree, so the store cannot store the snapshot's
    /// content. That happens when the write of the changed snapshot before it failed. The store
    /// skips such a snapshot, because a record without its content could not be read.
    Missing,
}

/// A content that a write stored for the first time.
pub struct NewContent {
    pub key: ContentKey,
    /// The encoded tree.
    pub tree: Vec<u8>,
    /// The text that the search index makes searchable.
    pub text: String,
}

impl PendingObservation {
    /// An observation of `segment`, whose content is stored as a delta from `base` when the
    /// content is new and `base` is given.
    pub const fn new(
        segment: SegmentKey,
        observation: Observation,
        base: Option<Arc<Expanded>>,
    ) -> Self {
        Self {
            segment,
            observation,
            base,
        }
    }

    /// Stores an `Observed` record, and its content when the segment does not have it yet.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: the commit consumes the transaction"
    )]
    pub fn store(self, keyspaces: &Keyspaces) -> Result<Observed, StoreError> {
        let mut writer = keyspaces.write()?;
        let observed = observe(&mut writer, self.segment, self.observation, self.base.as_deref())?;
        writer.commit()?;
        Ok(observed)
    }
}

/// Adds an `Observed` record to `writer`, and the record's content when the segment does not
/// have the content yet.
fn observe(
    writer: &mut Writer<'_>,
    segment: SegmentKey,
    observation: Observation,
    base: Option<&Expanded>,
) -> Result<Observed, StoreError> {
    let Observation {
        at,
        trigger,
        content,
    } = observation;
    if let SnapshotContent::Changed(content) = &content {
        content.tree.verify().map_err(|_| StoreError::SentHash)?;
    }
    let hash = content.hash();
    let content_read_time = content.read_time();
    let content_read = content.read();
    let key = ContentKey { segment, hash };
    let known = writer.has_content(key)?;
    let tree = match content {
        SnapshotContent::Unchanged { .. } if !known => return Ok(Observed::Missing),
        SnapshotContent::Changed(content) if !known => Some(content.tree),
        SnapshotContent::Changed(_) | SnapshotContent::Unchanged { .. } => None,
    };
    let body = StoredBody::Observed {
        hash,
        trigger,
        read_us: u64::try_from(content_read_time.as_micros()).unwrap_or(u64::MAX),
        nodes: u64::try_from(content_read.nodes).unwrap_or(u64::MAX),
    };
    writer.append(segment, at.into(), body)?;
    let Some(tree) = tree else {
        return Ok(Observed::Known);
    };
    let text = IndexItem::text_of(&tree.decode().map_err(StoreError::SentTree)?);
    let tree = tree.into_bytes();
    let bytes = match base {
        Some(base) => delta::delta(&tree, &base.tree),
        None => delta::whole(&tree),
    };
    let content = StoredContent {
        base: base.map(|base| base.hash),
        bytes: &bytes,
    };
    writer.insert_content(key, &content)?;
    Ok(Observed::New(NewContent { key, tree, text }))
}

/// Starts a segment.
pub fn start(
    keyspaces: &Keyspaces,
    open: &OpenSegments,
    at: SystemTime,
    start: &SegmentStart,
) -> Result<OpenSegment, StoreError> {
    let mut writer = keyspaces.write()?;
    let segment = start_in(&mut writer, open, at, start)?;
    commit_started(writer, open, segment)
}

/// A background segment that [`start_background`] started, with what its first observation
/// stored.
pub struct StartedBackground {
    pub segment: OpenSegment,
    pub observed: Observed,
    /// The URL of the segment's start.
    pub url: Option<String>,
}

/// Starts a background segment with its first observation, in one transaction. The same
/// transaction ends the segment `old` at the start, when `old` is given.
///
/// A failure leaves the store unchanged, so that every stored background segment has an
/// observation.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: the commit consumes the transaction"
)]
pub fn start_background(
    keyspaces: &Keyspaces,
    open: &OpenSegments,
    old: Option<SegmentKey>,
    first: FirstObservation,
) -> Result<StartedBackground, StoreError> {
    let FirstObservation {
        at,
        start,
        observation,
    } = first;
    let mut writer = keyspaces.write()?;
    if let Some(old) = old {
        writer.append(old, at.into(), StoredBody::SegmentEnded)?;
    }
    let segment = start_in(&mut writer, open, at, &start)?;
    let observed = observe(&mut writer, segment.key, observation, None)
        .inspect_err(|_| open.remove(segment.key))?;
    let segment = commit_started(writer, open, segment)?;
    if let Some(old) = old {
        open.remove(old);
    }
    Ok(StartedBackground {
        segment,
        observed,
        url: start.url,
    })
}

/// Adds the start of a segment to `writer`, and adds the segment to `open`.
fn start_in(
    writer: &mut Writer<'_>,
    open: &OpenSegments,
    at: SystemTime,
    start: &SegmentStart,
) -> Result<OpenSegment, StoreError> {
    let info = SegmentInfo::new(at, start);
    let key = writer.start_segment(at.into(), start)?;
    open.insert(key);
    Ok(OpenSegment::new(key, info))
}

/// Commits the start of `segment`, and takes the segment out of `open` when the commit fails.
fn commit_started(
    writer: Writer<'_>,
    open: &OpenSegments,
    segment: OpenSegment,
) -> Result<OpenSegment, StoreError> {
    let key = segment.key;
    writer.commit().inspect_err(|_| open.remove(key)).map(|()| segment)
}

pub fn end(
    keyspaces: &Keyspaces,
    open: &OpenSegments,
    key: SegmentKey,
    at: SystemTime,
) -> Result<(), StoreError> {
    keyspaces.append(key, at.into(), StoredBody::SegmentEnded)?;
    open.remove(key);
    Ok(())
}

/// Ends each of `ends` at its time, in one transaction.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: the commit consumes the transaction"
)]
pub fn end_background(
    keyspaces: &Keyspaces,
    open: &OpenSegments,
    ends: &[BackgroundEnd],
) -> Result<(), StoreError> {
    let mut writer = keyspaces.write()?;
    for end in ends {
        writer.append(end.key, end.at.into(), StoredBody::SegmentEnded)?;
    }
    writer.commit()?;
    open.remove_all(ends.iter().map(|end| end.key));
    Ok(())
}

/// Stores the segment of a dialog or a notification, with its start, its one observation, and
/// its end in one transaction. Returns the new content for the search index.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: the commit consumes the transaction"
)]
pub fn one_shot(
    keyspaces: &Keyspaces,
    first: FirstObservation,
) -> Result<Option<IndexItem>, StoreError> {
    let mut writer = keyspaces.write()?;
    // The segment starts and ends in one commit, so a reader never sees the segment open, and
    // `open` doesn't need to hold the segment.
    let key = writer.start_segment(first.at.into(), &first.start)?;
    let item = observe_once(&mut writer, key, first)?;
    writer.commit()?;
    Ok(item)
}

/// Adds the one observation and the end of a segment that `writer` started as `key` with
/// `first.start`. Returns the new content for the search index.
pub fn observe_once(
    writer: &mut Writer<'_>,
    key: SegmentKey,
    first: FirstObservation,
) -> Result<Option<IndexItem>, StoreError> {
    let FirstObservation {
        at,
        start,
        observation,
    } = first;
    let observed = observe(writer, key, observation, None)?;
    writer.append(key, at.into(), StoredBody::SegmentEnded)?;
    Ok(match observed {
        Observed::New(new) => Some(SegmentInfo::new(at, &start).into_item(new.key, new.text)),
        Observed::Known | Observed::Missing => None,
    })
}

/// Ends every open segment at its last record, including those that an earlier process left
/// open.
#[expect(
    clippy::significant_drop_tightening,
    reason = "false positive: the commit consumes the transaction"
)]
pub fn close_open_segments(keyspaces: &Keyspaces, open: &OpenSegments) -> Result<(), StoreError> {
    let reader = keyspaces.read();
    let mut keys = if open.is_swept() {
        BTreeSet::new()
    } else {
        dangling_segments(&reader)?
    };
    open.copy_into(&mut keys);
    if !keys.is_empty() {
        let mut writer = keyspaces.write()?;
        for &segment in &keys {
            end_at_last_record(&reader, &mut writer, segment)?;
        }
        drop(reader);
        writer.commit()?;
        open.remove_all(keys);
    }
    open.mark_swept();
    Ok(())
}

/// The segments whose last record is not their end.
///
/// A segment whose records are all damaged is logged and left out.
fn dangling_segments(reader: &Reader<'_>) -> Result<BTreeSet<SegmentKey>, StoreError> {
    let mut dangling = BTreeSet::new();
    for segment in reader.segments() {
        let segment = segment?;
        match reader.last_intact_head(segment)? {
            Some(last) if last.head.body != BodyKind::SegmentEnded => {
                dangling.insert(segment);
            }
            Some(_) => {}
            None => warn!(%segment, "a desktop capture segment doesn't have an intact record"),
        }
    }
    Ok(dangling)
}

/// Adds an end to `segment` at the time of the segment's last intact record. That record is the
/// segment's start when the segment doesn't have any other intact record.
fn end_at_last_record(
    reader: &Reader<'_>,
    writer: &mut Writer<'_>,
    segment: SegmentKey,
) -> Result<(), StoreError> {
    let Some(last) = reader.last_intact_head(segment)? else {
        warn!(%segment, "could not end a desktop capture segment, because none of its records is intact");
        return Ok(());
    };
    writer.append(segment, last.head.at, StoredBody::SegmentEnded).map(|_| ())
}
