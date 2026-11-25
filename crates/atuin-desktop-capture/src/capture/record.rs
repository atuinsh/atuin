//! The records that capture hands out.

use std::time::{Duration, SystemTime};

use crate::backend::{Cause, Node, ReadStats};

/// What made capture take a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub enum Trigger {
    Activated,
    TitleChanged,
    Loaded,
    Edited,
    Restructured,
    /// No event arrived for a while, and the window was read anyway.
    Polled,
    /// Capture read every window when it started.
    Swept,
    /// The last read of the window ran out of time, and the window was read again.
    Retried,
}

impl Trigger {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Activated => "activated",
            Self::TitleChanged => "title-changed",
            Self::Loaded => "loaded",
            Self::Edited => "edited",
            Self::Restructured => "restructured",
            Self::Polled => "polled",
            Self::Swept => "swept",
            Self::Retried => "retried",
        }
    }
}

impl From<Cause> for Trigger {
    fn from(cause: Cause) -> Self {
        match cause {
            Cause::Loaded => Self::Loaded,
            Cause::Edited => Self::Edited,
            Cause::Restructured => Self::Restructured,
        }
    }
}

/// Identifies a segment within one [`Capture`](super::Capture).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct SegmentId(pub u64);

/// Identifies a window within one [`Capture`](super::Capture), across the segments of the
/// window.
///
/// A store can use the id to find the last snapshot of a window. For example, the store can
/// keep a new snapshot as a delta from that snapshot. Treat the id as a hint: a toolkit can
/// hand the handle of a closed window to a new one, and the id then goes along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct WindowId(pub u64);

/// A stretch of time during which one window was active under one title and URL.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct Segment {
    pub id: SegmentId,
    /// The window that the segment is about.
    pub window: WindowId,
    pub app: String,
    /// The window title, with URL parameters removed.
    pub title: String,
    /// The URL of the window's document, reduced to its scheme, host, and path.
    pub url: Option<String>,
    pub started: SystemTime,
}

/// The BLAKE3 hash of a snapshot's tree in its [encoded](crate::encoding) form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// The hash of `root` in its encoded form, without the encoded bytes in memory.
    #[must_use]
    pub fn of(root: &Node) -> Self {
        Self(*crate::encoding::hash(root).as_bytes())
    }

    /// The hash of a tree that is encoded already.
    #[must_use]
    pub fn of_encoded(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }
}

/// An element tree in its [encoded](crate::encoding) form, with the hash of those bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct EncodedTree {
    #[cfg_attr(feature = "link", serde(with = "atuin_common::rmp::byte_buf"))]
    bytes: Vec<u8>,
    hash: ContentHash,
}

/// The hash that came with an [`EncodedTree`] does not match the tree's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the hash of an encoded tree does not match its bytes")]
pub struct HashMismatch;

impl EncodedTree {
    /// Encodes the tree under `root`, and hashes the bytes.
    #[must_use]
    pub fn encode(root: &Node) -> Self {
        let bytes = crate::encoding::encode(root);
        let hash = ContentHash::of_encoded(&bytes);
        Self { bytes, hash }
    }

    /// Encodes the tree under `root`, whose encoded form takes at most `len` bytes, and hashes
    /// the bytes.
    #[must_use]
    pub fn encode_with_capacity(root: &Node, len: usize) -> Self {
        let bytes = crate::encoding::encode_with_capacity(root, len);
        let hash = ContentHash::of_encoded(&bytes);
        Self { bytes, hash }
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The hash of the bytes. A tree that crossed the link carries the hash that its sender
    /// computed, which [`verify`](Self::verify) checks.
    #[must_use]
    pub const fn hash(&self) -> ContentHash {
        self.hash
    }

    /// Decodes the tree.
    ///
    /// # Errors
    ///
    /// Fails when the bytes are not an encoded tree.
    pub fn decode(&self) -> Result<Node, crate::encoding::DecodeError> {
        crate::encoding::decode(&self.bytes)
    }

    /// Checks that the hash matches the bytes.
    ///
    /// # Errors
    ///
    /// Fails when the hash is not the hash of the bytes.
    pub fn verify(&self) -> Result<(), HashMismatch> {
        if ContentHash::of_encoded(&self.bytes) == self.hash {
            Ok(())
        } else {
            Err(HashMismatch)
        }
    }
}

/// What one read of a window found: the element tree, and what the read cost.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct Content {
    /// The element tree, without credentials and URL parameters.
    /// [`render`](crate::render()) turns its decoded form into text.
    pub tree: EncodedTree,
    pub read_time: Duration,
    pub read: ReadStats,
}

impl Content {
    /// The hash of the tree.
    #[must_use]
    pub const fn hash(&self) -> ContentHash {
        self.tree.hash()
    }
}

/// Whether a snapshot differs from the previous snapshot of the same segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Changed,
    Unchanged,
}

/// The content of a segment's window at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct Snapshot {
    pub segment: SegmentId,
    pub at: SystemTime,
    pub trigger: Trigger,
    pub content: SnapshotContent,
}

/// What a snapshot found in its segment's window.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub enum SnapshotContent {
    /// The content differs from the previous snapshot of the segment.
    Changed(Content),
    /// The content is the same as in the previous snapshot of the segment, so the snapshot
    /// carries only the content's hash and what the read cost.
    Unchanged {
        hash: ContentHash,
        read_time: Duration,
        read: ReadStats,
    },
}

impl SnapshotContent {
    /// The hash of the content that the snapshot found.
    #[must_use]
    pub const fn hash(&self) -> ContentHash {
        match self {
            Self::Changed(content) => content.hash(),
            Self::Unchanged { hash, .. } => *hash,
        }
    }

    #[must_use]
    pub const fn change(&self) -> Change {
        match self {
            Self::Changed(_) => Change::Changed,
            Self::Unchanged { .. } => Change::Unchanged,
        }
    }

    /// How long the read took.
    #[must_use]
    pub const fn read_time(&self) -> Duration {
        match self {
            Self::Changed(content) => content.read_time,
            Self::Unchanged { read_time, .. } => *read_time,
        }
    }

    #[must_use]
    pub const fn read(&self) -> ReadStats {
        match self {
            Self::Changed(content) => content.read,
            Self::Unchanged { read, .. } => *read,
        }
    }
}

/// The content of a dialog or an alert at the moment that it appeared.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct Dialog {
    /// The dialog's own window. A segment of the same window follows when the dialog becomes
    /// active.
    pub window: WindowId,
    pub app: String,
    /// The window title, with URL parameters removed.
    pub title: String,
    pub at: SystemTime,
    pub content: Content,
}

/// The content of a window that capture read while the window was in the background.
///
/// Capture hands out a background window when it sweeps every window at its start, and again
/// whenever the content of the window differs from what capture last saw of it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct Background {
    /// The window. A segment of the same window follows when the window becomes active.
    pub window: WindowId,
    pub app: String,
    /// The window title, with URL parameters removed.
    pub title: String,
    /// The URL of the window's document, reduced to its scheme, host, and path.
    pub url: Option<String>,
    pub at: SystemTime,
    /// [`Trigger::Swept`] for the sweep, and otherwise the kind of change that led to the read.
    pub trigger: Trigger,
    pub content: Content,
}

/// A notification at the moment that an application sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub struct Notification {
    /// The application that sent the notification. Empty when the platform doesn't name a sender.
    pub app: String,
    pub title: String,
    pub at: SystemTime,
    /// The content of the notification.
    ///
    /// For a notification that the platform reports as a window, this is the window's tree.
    /// Otherwise, it is a window with one paragraph that holds the body.
    pub content: Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "link", derive(serde::Serialize, serde::Deserialize))]
pub enum Record {
    /// A window stayed active long enough to be snapshotted. Its first snapshot follows.
    SegmentStarted(Segment),
    Snapshot(Snapshot),
    SegmentEnded {
        segment: SegmentId,
        at: SystemTime,
    },
    /// A dialog appeared, whether or not it became active. It doesn't belong to a segment yet; the
    /// daemon will assign one.
    Dialog(Dialog),
    /// An application sent a notification. It doesn't belong to a segment yet; the daemon will
    /// assign one.
    Notification(Notification),
    /// A window was read while it was in the background. It doesn't belong to a segment yet; the
    /// daemon will assign one.
    Background(Background),
}
