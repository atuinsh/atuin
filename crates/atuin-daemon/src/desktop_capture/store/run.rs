//! The state of one run of the helper: the segments that the run is writing to.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atuin_desktop_capture::{SegmentId, WindowId};
use tracing::warn;

use super::model::{IndexItem, SegmentInfo, SegmentKey, StoreError};
use super::schema::SegmentStart;
use super::tree::{Expanded, WHOLE_EVERY};
use super::write::{NewContent, Observation, Observed, PendingObservation, StartedBackground};

/// How long a background segment lasts before the next read of its window starts a new one, and
/// how long a background segment stays open after the run last wrote to it.
///
/// An eviction keeps every open segment, and a background segment would otherwise stay open
/// for as long as the helper runs.
pub const MAX_BACKGROUND_AGE: Duration = Duration::from_hours(1);

/// The most bytes of trees that a run keeps as the bases of its background segments' deltas.
const MAX_BACKGROUND_TREE_BYTES: usize = 32 << 20;

/// The state of one run of the helper.
///
/// The helper's segment and window ids are only valid for one run, so start a new state for
/// every run.
#[derive(Debug, Default)]
pub struct RunState {
    segments: HashMap<SegmentId, RunSegment>,
    /// The background segment of each window that the run has read in the background.
    background: HashMap<WindowId, BackgroundSegment>,
}

/// An active segment of a run.
#[derive(Debug)]
pub enum RunSegment {
    Open(OpenSegment),
    /// The store could not write the segment's start, so the segment's next record starts the
    /// segment again.
    Unstarted(Unstarted),
}

/// The start of an active segment that the store has not written yet.
#[derive(Debug)]
pub struct Unstarted {
    pub at: SystemTime,
    pub start: SegmentStart,
}

/// What a background read of a window does to the window's segment.
pub enum BackgroundStep<'a> {
    /// The read belongs to the window's segment.
    Continue(&'a mut OpenSegment),
    /// The read starts a new segment, and ends the window's segment `old` when there is one.
    Start {
        old: Option<SegmentKey>,
    },
}

impl RunState {
    pub fn add_segment(&mut self, id: SegmentId, segment: RunSegment) {
        self.segments.insert(id, segment);
    }

    /// Whether the run stored the start of segment `id`, and has not stored its end yet.
    pub fn is_open(&self, id: SegmentId) -> bool {
        matches!(self.segments.get(&id), Some(RunSegment::Open(_)))
    }

    /// Whether the run has started segment `id` and has not ended the segment yet, including a
    /// segment whose start the store could not write.
    pub fn has_segment(&self, id: SegmentId) -> bool {
        self.segments.contains_key(&id)
    }

    /// Whether the store could not write the start of segment `id`, so that the segment's next
    /// record tries the start again.
    pub fn is_unstarted(&self, id: SegmentId) -> bool {
        matches!(self.segments.get(&id), Some(RunSegment::Unstarted(_)))
    }

    /// Takes out the start of the segment `id` when the store has not written the start yet.
    pub fn take_unstarted(&mut self, id: SegmentId) -> Option<Unstarted> {
        if !matches!(self.segments.get(&id), Some(RunSegment::Unstarted(_))) {
            return None;
        }
        match self.segments.remove(&id) {
            Some(RunSegment::Unstarted(unstarted)) => Some(unstarted),
            Some(RunSegment::Open(_)) | None => None,
        }
    }

    /// The open segment `id`.
    pub fn segment(&mut self, id: SegmentId) -> Result<&mut OpenSegment, StoreError> {
        match self.segments.get_mut(&id) {
            Some(RunSegment::Open(segment)) => Ok(segment),
            Some(RunSegment::Unstarted(_)) | None => Err(StoreError::UnknownSegment(id)),
        }
    }

    pub fn take_segment(&mut self, id: SegmentId) -> Result<RunSegment, StoreError> {
        self.segments.remove(&id).ok_or(StoreError::UnknownSegment(id))
    }

    /// What a read of `window` at `at`, which `start` describes, does to the window's segment.
    pub fn background_step(
        &mut self,
        window: WindowId,
        start: &SegmentStart,
        at: SystemTime,
    ) -> BackgroundStep<'_> {
        match self.background.get_mut(&window) {
            Some(background) if background.continues(start, at) => {
                BackgroundStep::Continue(background.written_at(at))
            }
            Some(background) => BackgroundStep::Start {
                old: Some(background.segment.key),
            },
            None => BackgroundStep::Start { old: None },
        }
    }

    /// Takes the segment that a [`BackgroundStep::Start`] of `window` started as the window's
    /// segment, and returns the item of the search index for the segment's first content.
    pub fn started_background(
        &mut self,
        window: WindowId,
        started: StartedBackground,
    ) -> Option<IndexItem> {
        let StartedBackground {
            segment,
            observed,
            url,
        } = started;
        let mut background = BackgroundSegment::new(segment, url);
        let item = background.segment.observed(observed);
        self.background.insert(window, background);
        item
    }

    /// Takes out the background segments that the run last wrote to at least
    /// [`MAX_BACKGROUND_AGE`] before `now`, each to end [`MAX_BACKGROUND_AGE`] after its last
    /// write.
    ///
    /// The helper reads a background window again only when the window changes, so the window
    /// may have shown the segment's last content until the end. The next read of such a
    /// segment's window starts a new segment.
    pub fn take_idle_background(&mut self, now: SystemTime) -> Vec<BackgroundEnd> {
        self.background
            .extract_if(|_, background| {
                now.duration_since(background.written).is_ok_and(|idle| idle >= MAX_BACKGROUND_AGE)
            })
            .map(|(_, background)| BackgroundEnd {
                key: background.segment.key,
                at: background.written + MAX_BACKGROUND_AGE,
            })
            .collect()
    }

    /// Takes out every background segment, each to end at `at`.
    ///
    /// The next read of a window starts a new segment.
    pub fn take_background(&mut self, at: SystemTime) -> Vec<BackgroundEnd> {
        self.background
            .drain()
            .map(|(_, background)| BackgroundEnd {
                key: background.segment.key,
                at,
            })
            .collect()
    }

    /// Drops the trees of the background segments that the run wrote to least recently, until
    /// the trees that the run keeps take at most [`MAX_BACKGROUND_TREE_BYTES`].
    ///
    /// A segment without its tree stores its next content as a whole tree.
    pub fn trim_background_trees(&mut self) {
        let mut kept: usize = self.background.values().map(BackgroundSegment::tree_bytes).sum();
        while kept > MAX_BACKGROUND_TREE_BYTES {
            let Some(oldest) = self
                .background
                .values_mut()
                .filter(|background| background.segment.last.is_some())
                .min_by_key(|background| background.written)
            else {
                return;
            };
            kept -= oldest.tree_bytes();
            oldest.segment.last = None;
        }
    }
}

/// A background segment that a run no longer writes to, and when the segment ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundEnd {
    pub key: SegmentKey,
    pub at: SystemTime,
}

/// A segment that a run is writing to.
#[derive(Debug)]
pub struct OpenSegment {
    pub key: SegmentKey,
    info: SegmentInfo,
    /// The segment's last new content, which is the base of the next delta.
    last: Option<Arc<Expanded>>,
    /// The number of distinct contents that the segment has stored.
    distinct: u32,
    /// Whether the segment was told about an unchanged content that it does not hold.
    missed: Missed,
}

/// Whether a segment was told about an unchanged content that it does not hold, which is
/// logged once per segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Missed {
    Never,
    Logged,
}

impl OpenSegment {
    pub const fn new(key: SegmentKey, info: SegmentInfo) -> Self {
        Self {
            key,
            info,
            last: None,
            distinct: 0,
            missed: Missed::Never,
        }
    }

    /// Whether the segment stores its next new content as a whole tree.
    const fn is_due_for_whole_tree(&self) -> bool {
        self.distinct.is_multiple_of(WHOLE_EVERY)
    }

    /// Prepares an observation of the segment's window for [`PendingObservation::store`].
    pub fn pending(&self, observation: Observation) -> PendingObservation {
        let base = self.last.as_ref().filter(|_| !self.is_due_for_whole_tree()).map(Arc::clone);
        PendingObservation::new(self.key, observation, base)
    }

    /// Takes what [`PendingObservation::store`] stored, and returns the item of the search
    /// index for a new content.
    pub fn observed(&mut self, observed: Observed) -> Option<IndexItem> {
        match observed {
            Observed::New(new) => Some(self.stored(new)),
            Observed::Known => None,
            Observed::Missing => {
                if self.missed == Missed::Never {
                    self.missed = Missed::Logged;
                    warn!(
                        segment = %self.key,
                        "the helper reported an unchanged desktop capture content that the \
                         segment does not hold, so the store skips the snapshot"
                    );
                }
                None
            }
        }
    }

    /// Takes a new content of the segment as the base of the next delta, and returns the item
    /// of the search index for the content.
    fn stored(&mut self, new: NewContent) -> IndexItem {
        let NewContent { key, tree, text } = new;
        self.last = Some(Arc::new(Expanded {
            hash: key.hash,
            tree,
        }));
        self.distinct += 1;
        self.info.item(key, text)
    }
}

/// The segment of a window that a run reads in the background.
#[derive(Debug)]
struct BackgroundSegment {
    segment: OpenSegment,
    url: Option<String>,
    /// When the run last wrote to the segment.
    written: SystemTime,
}

impl BackgroundSegment {
    fn new(segment: OpenSegment, url: Option<String>) -> Self {
        let written = segment.info.started;
        Self {
            segment,
            url,
            written,
        }
    }

    /// Whether a read of the window that `start` describes, at `at`, belongs to the segment.
    ///
    /// The read starts a new segment when the window's application, title, or URL changed.
    /// The read also starts a new segment when the segment's next new content would be a whole
    /// tree, or when the segment has lasted [`MAX_BACKGROUND_AGE`], so that an eviction can
    /// remove the older part of a long run.
    fn continues(&self, start: &SegmentStart, at: SystemTime) -> bool {
        let info = &self.segment.info;
        let rolls = (self.segment.distinct > 0 && self.segment.is_due_for_whole_tree())
            || at.duration_since(info.started).is_ok_and(|age| age >= MAX_BACKGROUND_AGE);
        *info.app == *start.app && *info.title == *start.title && self.url == start.url && !rolls
    }

    /// Notes a write to the segment at `at`, and returns the segment.
    const fn written_at(&mut self, at: SystemTime) -> &mut OpenSegment {
        self.written = at;
        &mut self.segment
    }

    fn tree_bytes(&self) -> usize {
        self.segment.last.as_ref().map_or(0, |last| last.tree.len())
    }
}

#[cfg(test)]
impl RunState {
    /// Adds segment `id` as an active segment of gedit whose start the store could not write.
    pub fn add_unstarted(&mut self, id: SegmentId, at: SystemTime) {
        let start = SegmentStart {
            kind: super::model::SegmentKind::Active,
            app: "gedit".to_owned(),
            title: "unstarted".to_owned(),
            url: None,
        };
        self.add_segment(id, RunSegment::Unstarted(Unstarted { at, start }));
    }
}

#[cfg(test)]
mod tests {
    use atuin_desktop_capture::ContentHash;
    use rstest::rstest;
    use uuid::Uuid;

    use super::*;
    use crate::desktop_capture::store::SegmentKind;

    const MIB: usize = 1 << 20;

    /// A background segment whose last tree takes `bytes`, and which the run last wrote to
    /// `written` seconds after the epoch.
    fn background(n: u128, bytes: usize, written: u64) -> BackgroundSegment {
        let info = SegmentInfo {
            kind: SegmentKind::Background,
            app: Arc::from("kate"),
            title: Arc::from("notes"),
            started: SystemTime::UNIX_EPOCH,
        };
        let mut segment = OpenSegment::new(SegmentKey(Uuid::from_u128(n)), info);
        segment.last = Some(Arc::new(Expanded {
            hash: ContentHash([0; 32]),
            tree: vec![0; bytes],
        }));
        BackgroundSegment {
            segment,
            url: None,
            written: SystemTime::UNIX_EPOCH + Duration::from_secs(written),
        }
    }

    #[rstest]
    #[case::under_the_cap(&[(1, 10), (2, 10)], &[1, 2])]
    #[case::the_least_recent_goes(&[(1, 12), (2, 12), (3, 12)], &[1, 3])]
    #[case::as_many_as_needed(&[(1, 20), (2, 20), (3, 20)], &[1])]
    fn trimming_drops_the_trees_that_the_run_wrote_least_recently(
        #[case] segments: &[(u64, usize)],
        #[case] kept: &[u64],
    ) {
        // The run wrote to window 2 first, then to window 3, and then to window 1.
        let written = |window: u64| match window {
            1 => 30,
            2 => 10,
            _ => 20,
        };
        let mut run = RunState::default();
        for &(window, mib) in segments {
            let segment = background(window.into(), mib * MIB, written(window));
            run.background.insert(WindowId(window), segment);
        }

        run.trim_background_trees();

        let mut with_trees: Vec<u64> = run
            .background
            .iter()
            .filter(|(_, background)| background.segment.last.is_some())
            .map(|(window, _)| window.0)
            .collect();
        with_trees.sort_unstable();
        assert_eq!(with_trees, kept);
    }
}
