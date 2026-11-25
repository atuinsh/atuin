//! The types that callers of the store see.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atuin_common::string::StrExt as _;
use atuin_desktop_capture::{ContentHash, Node, SegmentId, Trigger, render};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::keyspaces::Started;
use super::query::MatchedRecord;
use super::schema::SegmentStart;

/// The id of a stored segment, which is the id of its first record.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    derive_more::Display,
    derive_more::From,
)]
#[serde(transparent)]
pub struct SegmentKey(pub Uuid);

/// What a deletion removes.
#[derive(Debug, Clone, derive_more::From)]
pub enum Selection {
    /// Every segment that the filter selects.
    Filter(SegmentFilter),
    /// One segment.
    Segment(SegmentKey),
}

/// Names one distinct tree of a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentKey {
    pub segment: SegmentKey,
    pub hash: ContentHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SegmentKind {
    /// A window that the user had active.
    Active,
    /// A dialog, as it appeared.
    Dialog,
    /// A window that capture read while it was in the background.
    ///
    /// A run of the helper starts a new segment for the window when the window's application,
    /// title, or URL changes, when the next new content would be a whole tree, and after an
    /// hour. A segment also ends after an hour without a write.
    Background,
    /// A notification, as it arrived.
    Notification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFilter {
    pub from: SystemTime,
    pub to: SystemTime,
    /// The application of the segments, compared without regard to case.
    pub app: Option<String>,
    pub kinds: Kinds,
}

impl SegmentFilter {
    /// Whether `app` is the filter's application, or the filter doesn't name an application.
    pub fn matches_app(&self, app: &str) -> bool {
        self.app.as_ref().is_none_or(|wanted| wanted.eq_ignoring_case(app))
    }

    /// Whether the filter selects segments of `kind`.
    pub fn matches_kind(&self, kind: SegmentKind) -> bool {
        match &self.kinds {
            Kinds::All => true,
            Kinds::Only(kinds) => kinds.contains(&kind),
        }
    }

    /// Whether the filter selects the segments of every application and every kind.
    pub fn selects_every_app_and_kind(&self) -> bool {
        self.app.is_none() && self.kinds == Kinds::All
    }

    /// Whether the filter selects the segment that `started` starts, by its kind, its
    /// application, and whether it started by the end of the filter's range.
    pub fn admits(&self, started: &Started) -> bool {
        self.matches_kind(started.start.kind)
            && self.matches_app(&started.start.app)
            && SystemTime::from(started.at) <= self.to
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kinds {
    All,
    Only(Vec<SegmentKind>),
}

/// The snapshot of a segment that a read returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    First,
    Latest,
    /// The snapshot of this record id.
    Record(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSummary {
    pub key: SegmentKey,
    pub kind: SegmentKind,
    pub app: String,
    pub title: String,
    pub url: Option<String>,
    pub started: SystemTime,
    /// `None` when the segment is open.
    pub ended: Option<SystemTime>,
    pub snapshots: u32,
}

/// One snapshot of a segment, with its tree.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRead {
    pub segment: SegmentSummary,
    /// The `Observed` record of the snapshot.
    pub record: Uuid,
    pub at: SystemTime,
    pub root: Node,
}

/// One snapshot of a segment, rendered as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotText {
    pub segment: SegmentSummary,
    /// The `Observed` record of the snapshot.
    pub record: Uuid,
    pub at: SystemTime,
    pub text: String,
    /// Whether the text leaves out part of the snapshot, to keep within the bytes that the
    /// read allowed.
    pub truncated: bool,
}

/// The text of one snapshot, from a read of several snapshots of a segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordText {
    /// The `Observed` record of the snapshot.
    pub record: Uuid,
    pub text: String,
    /// Whether the text leaves out part of the snapshot, to keep within the bytes that the
    /// read allowed.
    pub truncated: bool,
}

/// How much text a read of several snapshots returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextBudget {
    /// The longest text of one snapshot, in bytes. A longer text keeps its beginning and its
    /// end.
    pub per_text: usize,
    /// The most bytes that the texts hold together. A read always returns its first text.
    pub total: usize,
}

/// The texts of a read of several snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordTexts {
    /// The texts, in the order of the segment's snapshots.
    pub texts: Vec<RecordText>,
    /// How many of the chosen snapshots, after the last of `texts`, the read left out to keep
    /// within [`TextBudget::total`].
    pub left_out: usize,
}

/// The totals of the segments that overlap a time range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    /// One tally per application, by the application's name.
    pub apps: Vec<AppTally>,
    /// From the earliest start to the latest end of the tallied segments. `None` when the range
    /// doesn't hold any segment.
    pub span: Option<Span>,
    /// How many damaged segments and records a deletion by the tally's filter removes. The
    /// totals and the span leave them out.
    pub damaged: u64,
}

/// The totals of one application's segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppTally {
    pub app: String,
    pub totals: AppTotals,
}

/// The counts of one application's segments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AppTotals {
    /// The segments other than notifications.
    pub windows: u64,
    pub notifications: u64,
    /// The snapshots of the segments other than notifications.
    pub snapshots: u64,
    /// How long the segments other than notifications lasted, together.
    pub duration: Duration,
}

/// A stretch of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub from: SystemTime,
    pub to: SystemTime,
}

/// One snapshot of a segment, without its tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummary {
    /// The `Observed` record of the snapshot.
    pub record: Uuid,
    pub at: SystemTime,
    /// What led capture to read the window. `None` for a dialog.
    pub trigger: Option<Trigger>,
    /// How many elements the read stored.
    pub nodes: u64,
    /// Whether the snapshot read the same content as the snapshot before it.
    pub same_as_previous: bool,
}

/// A segment with every one of its snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSnapshots {
    pub segment: SegmentSummary,
    /// Oldest first.
    pub snapshots: Vec<SnapshotSummary>,
}

/// What a segment's start says about every content of the segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInfo {
    pub kind: SegmentKind,
    pub app: Arc<str>,
    pub title: Arc<str>,
    pub started: SystemTime,
}

impl SegmentInfo {
    /// What the start of a segment at `at` says about the segment's contents.
    pub fn new(at: SystemTime, start: &SegmentStart) -> Self {
        Self {
            kind: start.kind,
            app: Arc::from(start.app.as_str()),
            title: Arc::from(start.title.as_str()),
            started: at,
        }
    }

    /// The item of the search index for the content `key` of the segment, whose text is `text`.
    pub fn item(&self, key: ContentKey, text: String) -> IndexItem {
        self.clone().into_item(key, text)
    }

    /// Like [`Self::item`], for a caller that no longer needs the segment's info.
    pub fn into_item(self, key: ContentKey, text: String) -> IndexItem {
        IndexItem {
            key,
            kind: self.kind,
            app: self.app,
            title: self.title,
            started: self.started,
            text,
        }
    }
}

impl From<Started> for SegmentInfo {
    fn from(started: Started) -> Self {
        Self {
            kind: started.start.kind,
            app: Arc::from(started.start.app),
            title: Arc::from(started.start.title),
            started: started.at.into(),
        }
    }
}

/// A content that a search matched, with the last record that observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedContent {
    pub item: IndexItem,
    /// The last `Observed` record of the content.
    pub record: Uuid,
    pub at: SystemTime,
}

/// The store's verdict on one search hit, from
/// [`DesktopStore::admit_all`](super::DesktopStore::admit_all).
#[derive(Debug)]
pub struct Admission {
    pub key: ContentKey,
    /// What the store holds for the hit, or the error of a hit that the store can't read.
    pub matched: Result<Matched, StoreError>,
}

/// What the store holds for a search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matched {
    Found(MatchedRecord),
    /// The store no longer holds the hit's segment, so the index's rows for it are stale.
    NoSegment,
    /// None of the segment's records observed the hit's content, so the index's row for the
    /// content is stale.
    NoObservation,
    /// The hit's segment lies outside the search's filter.
    Excluded,
}

/// A distinct content, with what the search index holds for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexItem {
    pub key: ContentKey,
    pub kind: SegmentKind,
    pub app: Arc<str>,
    pub title: Arc<str>,
    /// When the content's segment started.
    pub started: SystemTime,
    pub text: String,
}

impl IndexItem {
    /// The text of a tree that the search index makes searchable.
    pub fn text_of(root: &Node) -> String {
        render(root, usize::MAX).text
    }
}

/// A dialog with the text of its snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogText {
    pub segment: SegmentSummary,
    /// The text of the dialog, cut to the bytes that the read allowed. Empty when the dialog
    /// doesn't have a snapshot, or when the store can't read the dialog's tree.
    pub text: String,
}

/// What happened on the desktop in a time range, from
/// [`DesktopStore::recent_activity`](super::DesktopStore::recent_activity).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentActivity {
    /// The active and notification segments that overlap the range, oldest first.
    pub segments: Vec<SegmentSummary>,
    /// The dialogs that overlap the range, oldest first.
    pub dialogs: Vec<DialogText>,
}

/// The size of the store, from [`DesktopStore::counts`](super::DesktopStore::counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub segments: u64,
    pub records: u64,
}

/// What a deletion removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deleted {
    pub segments: Vec<SegmentKey>,
    /// The damaged records that no segment listed and that the deletion removed.
    pub orphans: Vec<Uuid>,
    /// The ids of the damaged segments and records that a deletion by filter left in the store,
    /// because their damage hides whether the filter selects them. A segment appears as its
    /// key.
    pub undecided: Vec<Uuid>,
}

impl Deleted {
    /// Whether the deletion didn't remove anything.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.segments.is_empty() && self.orphans.is_empty()
    }
}

/// A deletion that failed after it had removed some records.
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub struct DeleteFailed {
    /// What the deletion removed before the failure.
    pub deleted: Box<Deleted>,
    #[source]
    pub source: StoreError,
}

/// What an eviction removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evicted {
    /// The segments that the eviction removed.
    pub segments: Vec<SegmentKey>,
    /// The bytes that the eviction freed, by the store's estimate.
    pub freed: u64,
}

/// What a write stored.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct Written {
    /// The new distinct content, for the search index. `None` when the write didn't store a
    /// new content.
    pub item: Option<IndexItem>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the desktop capture store's directory could not be made private: {0}")]
    Directory(#[source] std::io::Error),
    #[error("the files of the desktop capture store could not be updated: {0}")]
    Files(#[source] std::io::Error),
    #[error("another process has the desktop capture store open")]
    Locked,
    #[error("the desktop capture store failed: {0}")]
    Storage(#[from] fjall::Error),
    #[error("a desktop capture record does not encode: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("a stored desktop capture value does not decode: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error("a stored tree does not decode: {0}")]
    Tree(#[from] atuin_desktop_capture::encoding::DecodeError),
    #[error("the desktop capture store is damaged: {0}")]
    Corrupt(&'static str),
    #[error(
        "the desktop capture store holds data of format {found}, and this build of atuin reads \
         only format {}",
        super::format::FORMAT
    )]
    UnknownFormat {
        found: u32,
    },
    #[error("the helper sent a tree that does not decode: {0}")]
    SentTree(atuin_desktop_capture::encoding::DecodeError),
    #[error("the helper sent a tree whose hash does not match its bytes")]
    SentHash,
    #[error("the helper wrote to segment {0:?}, which is not open in this run")]
    UnknownSegment(SegmentId),
    #[error("another write to the desktop capture store conflicted with this one")]
    Conflict,
    #[error("the desktop capture store is closed")]
    Closed,
    #[error(
        "a rewrite of the desktop capture store needs {needed} free on the disk, which has \
         {available} free"
    )]
    NoSpace {
        needed: atuin_common::units::ByteSize,
        available: atuin_common::units::ByteSize,
    },
    /// The store replaced its database with a rewritten copy, and the copy failed to open. The
    /// store tries to open the copy again later.
    #[error("the rewritten desktop capture store could not be opened: {0}")]
    Reopen(#[source] std::sync::Arc<Self>),
}

impl StoreError {
    /// Whether the error comes from a damaged stored value. The store's maintenance skips such
    /// values.
    pub const fn is_damage(&self) -> bool {
        matches!(self, Self::Decode(_) | Self::Tree(_) | Self::Corrupt(_))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::any_app(None, "kate", true)]
    #[case::same(Some("kate"), "kate", true)]
    #[case::ascii_case(Some("KATE"), "kate", true)]
    #[case::non_ascii_case(Some("éCRIRE"), "Écrire", true)]
    #[case::other_app(Some("kate"), "kwrite", false)]
    fn matches_app_ignores_case(
        #[case] wanted: Option<&str>,
        #[case] app: &str,
        #[case] expected: bool,
    ) {
        let filter = SegmentFilter {
            from: SystemTime::UNIX_EPOCH,
            to: SystemTime::UNIX_EPOCH,
            app: wanted.map(str::to_owned),
            kinds: Kinds::All,
        };

        assert_eq!(filter.matches_app(app), expected);
    }
}
