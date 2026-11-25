//! The read side: segments, their snapshots, and search hits.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::SystemTime;

use atuin_desktop_capture::encoding::decode;
use atuin_desktop_capture::{Completeness, ContentHash, Node, Trigger, render};
use tracing::warn;
use uuid::Uuid;

use super::keyspaces::{Headed, Keyspaces, Reader, Started};
use super::model::{
    AppTally, AppTotals, ContentKey, DialogText, IndexItem, Kinds, Matched, MatchedContent,
    RecentActivity, RecordText, RecordTexts, SegmentFilter, SegmentInfo, SegmentKey, SegmentKind,
    SegmentSnapshots, SegmentSummary, SnapshotSummary, SnapshotText, Span, StoreError, Tally,
    TextBudget, Which,
};
use super::open::{OpenSegments, OpenSnapshot};
use super::schema::{self, BodyKind, SegmentStart};
use super::tree::{self, Expanded};

/// A snapshot of the keyspaces together with the segments that were open when the snapshot was
/// taken.
///
/// A segment that ends after the snapshot was taken reads as open. See [`super::open`] for why
/// the snapshot and the open segments agree.
pub struct View<'a> {
    reader: Reader<'a>,
    open: BTreeSet<SegmentKey>,
}

/// `result`, or `Ok(None)` in place of an error from damage, which the read logs.
fn skip_damage<T>(
    result: Result<Option<T>, StoreError>,
    segment: SegmentKey,
) -> Result<Option<T>, StoreError> {
    match result {
        Err(err) if err.is_damage() => {
            warn!(%err, %segment, "skipped a damaged desktop capture segment");
            Ok(None)
        }
        result => result,
    }
}

/// The last record that observed a search hit's content, in a segment that the search admits.
///
/// [`View::matched_content`] reads the content of the hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedRecord {
    segment: SegmentInfo,
    /// The last `Observed` record of the content.
    record: Uuid,
    at: SystemTime,
}

/// An `Observed` record of a segment.
struct Seen {
    id: Uuid,
    at: SystemTime,
    hash: ContentHash,
    trigger: Option<Trigger>,
    nodes: u64,
}

impl Seen {
    /// The observation that a record makes, when it is an `Observed` record.
    fn of(headed: &Headed) -> Option<Self> {
        match headed.head.body {
            BodyKind::Observed(observed) => Some(Self {
                id: headed.id,
                at: headed.head.at.into(),
                hash: observed.hash,
                trigger: observed.trigger,
                nodes: observed.nodes,
            }),
            BodyKind::SegmentStarted(_) | BodyKind::SegmentEnded => None,
        }
    }
}

/// A snapshot that a read chose, with its segment.
struct Chosen {
    segment: SegmentSummary,
    seen: Seen,
}

/// A segment that a filter admits and that overlaps the filter's time range.
struct Overlapping {
    key: SegmentKey,
    started: Started,
}

/// The segments that can overlap a time range, and the damaged records from the range's start
/// on.
#[derive(Default)]
struct Candidates {
    segments: BTreeSet<SegmentKey>,
    /// The records whose heads don't decode.
    damaged: BTreeSet<Uuid>,
}

/// What a deletion by a filter removes, from [`View::selected_by`].
#[derive(Debug, Default)]
pub struct Selected {
    /// The segments that the filter selects, oldest first.
    pub segments: Vec<SegmentKey>,
    /// The damaged records in the filter's time range that no segment lists.
    pub orphans: Vec<Uuid>,
    /// The damaged segments and records that the filter can select, but whose damage hides
    /// whether the filter selects them. A segment appears as its key.
    pub undecided: Vec<Uuid>,
    /// How many of `segments` have a damaged or missing start.
    pub damaged_starts: usize,
}

/// What the start of a segment is, from [`View::start_of`].
enum Start {
    Intact(Started),
    /// The view doesn't hold any record of the segment, because the segment started after the
    /// view's snapshot was taken.
    Later,
    /// The start is missing or damaged, though the view holds other records of the segment.
    Damaged,
}

/// The totals of a tally, which [`View::tally`] adds segments to.
#[derive(Default)]
struct Totals {
    apps: BTreeMap<String, AppTotals>,
    span: Option<Span>,
}

impl Totals {
    /// Adds a segment that ends at `ended`, or at `to` when the segment is open.
    fn add(&mut self, summary: SegmentSummary, to: SystemTime) {
        let ended = summary.ended.unwrap_or(to);
        self.span = Some(match self.span {
            None => Span {
                from: summary.started,
                to: ended,
            },
            Some(span) => Span {
                from: span.from.min(summary.started),
                to: span.to.max(ended),
            },
        });
        let totals = self.apps.entry(summary.app).or_default();
        if summary.kind == SegmentKind::Notification {
            totals.notifications += 1;
        } else {
            totals.windows += 1;
            totals.snapshots += u64::from(summary.snapshots);
            totals.duration += ended.duration_since(summary.started).unwrap_or_default();
        }
    }

    fn into_tally(self, damaged: usize) -> Tally {
        Tally {
            apps: self.apps.into_iter().map(|(app, totals)| AppTally { app, totals }).collect(),
            span: self.span,
            damaged: u64::try_from(damaged).unwrap_or(u64::MAX),
        }
    }
}

/// What a deletion by a filter does with one segment.
enum Verdict {
    Remove,
    /// The segment's start is damaged or missing, and the filter selects the segment anyway.
    RemoveDamaged,
    Keep,
    /// The segment's damage hides whether the filter selects the segment.
    Undecided,
}

impl<'a> View<'a> {
    pub fn new(keyspaces: &'a Keyspaces, open: &OpenSegments) -> Self {
        let OpenSnapshot { reader, open } = open.snapshot(keyspaces);
        Self { reader, open }
    }

    /// The segment that the record `id` belongs to.
    pub fn lookup(&self, id: Uuid) -> Result<Option<SegmentSummary>, StoreError> {
        let Some(key) = self.segment_of(id)? else {
            return Ok(None);
        };
        let Some(started) = self.reader.started(key)? else {
            return Ok(None);
        };
        Ok(Some(self.summarize(key, started, |_| {})?))
    }

    /// The key of the segment that the record `id` belongs to. `None` when the store doesn't
    /// hold the record or its segment.
    pub fn segment_of(&self, id: Uuid) -> Result<Option<SegmentKey>, StoreError> {
        let Some(raw) = self.reader.raw_record(id)? else {
            return Ok(None);
        };
        let segment = raw.head()?.segment;
        Ok(self.reader.has_segment(segment)?.then_some(segment))
    }

    /// The segments that overlap the filter's time range, oldest first.
    ///
    /// A damaged segment is logged and left out.
    pub fn segments(&self, filter: &SegmentFilter) -> Result<Vec<SegmentSummary>, StoreError> {
        self.overlapping(filter)?
            .map(|segment| {
                let Overlapping { key, started } = segment?;
                self.summarize(key, started, |_| {})
            })
            .collect()
    }

    /// What a deletion by `filter` removes: every segment that [`Self::segments`] lists for
    /// `filter`, and the damaged segments and records that `filter` can select.
    pub fn selected_by(&self, filter: &SegmentFilter) -> Result<Selected, StoreError> {
        let Candidates {
            mut segments,
            damaged,
        } = self.segment_keys_from(filter.from)?;
        let mut selected = Selected::default();
        for id in self.unlisted(damaged, &mut segments)? {
            if filter.selects_every_app_and_kind() {
                selected.orphans.push(id);
            } else {
                selected.undecided.push(id);
            }
        }
        for key in segments {
            match self.verdict(key, filter)? {
                Verdict::Remove => selected.segments.push(key),
                Verdict::RemoveDamaged => {
                    selected.segments.push(key);
                    selected.damaged_starts += 1;
                }
                Verdict::Keep => {}
                Verdict::Undecided => selected.undecided.push(key.0),
            }
        }
        Ok(selected)
    }

    /// Whether a deletion by `filter` removes the segment `key`.
    fn verdict(&self, key: SegmentKey, filter: &SegmentFilter) -> Result<Verdict, StoreError> {
        let started = match self.start_of(key)? {
            Start::Intact(started) => started,
            Start::Later => return Ok(Verdict::Keep),
            Start::Damaged => return Ok(Self::damaged_start_verdict(filter)),
        };
        if filter.admits(&started) && self.may_last_until(key, filter.from)? {
            Ok(Verdict::Remove)
        } else {
            Ok(Verdict::Keep)
        }
    }

    /// The start of the segment `key`. A damaged start is logged.
    fn start_of(&self, key: SegmentKey) -> Result<Start, StoreError> {
        match self.reader.started(key) {
            Ok(Some(started)) => Ok(Start::Intact(started)),
            Ok(None) if self.reader.segment_record_ids(key).next().is_none() => Ok(Start::Later),
            Ok(None) => {
                warn!(segment = %key, "a desktop capture segment doesn't have a start");
                Ok(Start::Damaged)
            }
            Err(err) if err.is_damage() => {
                warn!(%err, segment = %key, "a desktop capture segment has a damaged start");
                Ok(Start::Damaged)
            }
            Err(err) => Err(err),
        }
    }

    /// Adds to `segments` each segment that lists a record of `damaged`, and returns the records
    /// of `damaged` that no segment lists.
    ///
    /// The time of a damaged record is unknown, so every damaged record from a range's start on
    /// can be in the range.
    fn unlisted(
        &self,
        damaged: BTreeSet<Uuid>,
        segments: &mut BTreeSet<SegmentKey>,
    ) -> Result<impl Iterator<Item = Uuid>, StoreError> {
        let listing = if damaged.is_empty() {
            BTreeMap::new()
        } else {
            self.reader.segments_listing(&damaged)?
        };
        segments.extend(listing.values().copied());
        Ok(damaged.into_iter().filter(move |id| !listing.contains_key(id)))
    }

    /// Whether a deletion by `filter` removes a segment whose start is missing or damaged.
    ///
    /// Only the start names the segment's application, kind, and start time. The segment can
    /// overlap the filter's time range, because the segment is open or has a record in the range
    /// or after it.
    fn damaged_start_verdict(filter: &SegmentFilter) -> Verdict {
        if filter.selects_every_app_and_kind() {
            Verdict::RemoveDamaged
        } else {
            Verdict::Undecided
        }
    }

    /// The segments that the filter admits and that overlap the filter's time range, oldest
    /// first. A damaged segment is logged and left out.
    fn overlapping<'s>(
        &'s self,
        filter: &'s SegmentFilter,
    ) -> Result<impl Iterator<Item = Result<Overlapping, StoreError>> + 's, StoreError> {
        let keys = self.segment_keys_from(filter.from)?.segments;
        Ok(keys.into_iter().filter_map(move |key| {
            let started = skip_damage(self.admitted(key, filter), key);
            started.map(|started| started.map(|started| Overlapping { key, started })).transpose()
        }))
    }

    /// Every segment that can overlap a time range that starts at `from`, and the damaged
    /// records from `from` on, which the scan logs.
    fn segment_keys_from(&self, from: SystemTime) -> Result<Candidates, StoreError> {
        // Every segment that overlaps the range is open, or has a record in the range or after
        // it: its start, a snapshot, or its end.
        let mut candidates = Candidates::default();
        for raw in self.reader.records_from(schema::first_key_at(from)) {
            let raw = match raw {
                Ok(raw) => raw,
                Err(err) if err.is_damage() => {
                    warn!(%err, "skipped a desktop capture record with a damaged key");
                    continue;
                }
                Err(err) => return Err(err),
            };
            match raw.head() {
                Ok(head) => {
                    candidates.segments.insert(head.segment);
                }
                Err(err) if err.is_damage() => {
                    warn!(%err, record = %raw.id, "skipped a damaged desktop capture record");
                    candidates.damaged.insert(raw.id);
                }
                Err(err) => return Err(err),
            }
        }
        candidates.segments.extend(&self.open);
        Ok(candidates)
    }

    /// The newest `limit` segments, by their start, that overlap the filter's time range,
    /// oldest first. A damaged segment is logged and left out.
    pub fn newest_segments(
        &self,
        filter: &SegmentFilter,
        limit: usize,
    ) -> Result<Vec<SegmentSummary>, StoreError> {
        let mut segments = Vec::new();
        // A segment whose key is at `filter.from` or later started in the range or after it.
        // The older segments that overlap the range are open or have a record in the range,
        // and the store finds them as `segments` does.
        let boundary = SegmentKey(Uuid::from_bytes(schema::first_key_at(filter.from)));
        let mut newest = self.reader.segments_newest_first();
        for key in newest.by_ref() {
            let key = match key {
                Ok(key) => key,
                Err(err) if err.is_damage() => {
                    warn!(%err, "stopped listing desktop capture segments at a damaged key");
                    break;
                }
                Err(err) => return Err(err),
            };
            if segments.len() >= limit || key < boundary {
                break;
            }
            if let Some(started) = skip_damage(self.admitted(key, filter), key)? {
                segments.push(self.summarize(key, started, |_| {})?);
            }
        }
        drop(newest);
        if segments.len() < limit {
            let older = self.segment_keys_from(filter.from)?.segments;
            for key in older.into_iter().rev().filter(|&key| key < boundary) {
                if segments.len() >= limit {
                    break;
                }
                if let Some(started) = skip_damage(self.admitted(key, filter), key)? {
                    segments.push(self.summarize(key, started, |_| {})?);
                }
            }
        }
        segments.reverse();
        Ok(segments)
    }

    /// The active segments, dialogs, and notifications between `since` and `now`, with the text
    /// of each dialog cut to `dialog_text_bytes`.
    pub fn recent_activity(
        &self,
        since: SystemTime,
        now: SystemTime,
        dialog_text_bytes: usize,
    ) -> Result<RecentActivity, StoreError> {
        let filter = SegmentFilter {
            from: since,
            to: now,
            app: None,
            kinds: Kinds::Only(vec![
                SegmentKind::Active,
                SegmentKind::Dialog,
                SegmentKind::Notification,
            ]),
        };
        let mut activity = RecentActivity::default();
        for segment in self.overlapping(&filter)? {
            let Overlapping { key, started } = segment?;
            if started.start.kind == SegmentKind::Dialog {
                let mut latest = None;
                let segment = self.summarize(key, started, |seen| latest = Some(seen))?;
                let text = self.dialog_text(key, latest, dialog_text_bytes);
                activity.dialogs.push(DialogText { segment, text });
            } else {
                activity.segments.push(self.summarize(key, started, |_| {})?);
            }
        }
        Ok(activity)
    }

    /// The text of a dialog's latest snapshot, cut to `bytes`. Empty when the dialog doesn't
    /// have a snapshot, or when the store can't read the dialog's tree.
    fn dialog_text(&self, dialog: SegmentKey, latest: Option<Seen>, bytes: usize) -> String {
        let Some(latest) = latest else {
            return String::new();
        };
        match self.root(dialog, &latest) {
            Ok(root) => render(&root, bytes).text,
            Err(err) => {
                warn!(?err, segment = %dialog, "could not read a dialog's text");
                String::new()
            }
        }
    }

    /// Reads one snapshot of a segment. `None` when the segment or the snapshot is unknown.
    #[cfg(test)]
    pub fn read(
        &self,
        segment: SegmentKey,
        which: Which,
    ) -> Result<Option<super::model::SnapshotRead>, StoreError> {
        let Some(Chosen {
            segment: summary,
            seen,
        }) = self.chosen(segment, which)?
        else {
            return Ok(None);
        };
        Ok(Some(super::model::SnapshotRead {
            root: self.root(segment, &seen)?,
            segment: summary,
            record: seen.id,
            at: seen.at,
        }))
    }

    /// Reads one snapshot as text of at most `max_bytes`. `id` names a segment or one of its
    /// snapshots. Without `which`, the read takes the snapshot that `id` names, or the latest
    /// snapshot when `id` names a segment. `None` when the segment or the snapshot is unknown.
    pub fn read_text_by_id(
        &self,
        id: Uuid,
        which: Option<Which>,
        max_bytes: usize,
    ) -> Result<Option<SnapshotText>, StoreError> {
        let (segment, which) = match self.snapshot_segment(id)? {
            Some(segment) => (segment, which.unwrap_or(Which::Record(id))),
            None => (SegmentKey(id), which.unwrap_or(Which::Latest)),
        };
        self.read_text(segment, which, max_bytes)
    }

    /// Reads one snapshot of a segment as text of at most `max_bytes`. `None` when the segment
    /// or the snapshot is unknown.
    pub fn read_text(
        &self,
        segment: SegmentKey,
        which: Which,
        max_bytes: usize,
    ) -> Result<Option<SnapshotText>, StoreError> {
        let Some(Chosen {
            segment: summary,
            seen,
        }) = self.chosen(segment, which)?
        else {
            return Ok(None);
        };
        let rendered = render(&self.root(segment, &seen)?, max_bytes);
        Ok(Some(SnapshotText {
            segment: summary,
            record: seen.id,
            at: seen.at,
            text: rendered.text,
            truncated: rendered.completeness == Completeness::Truncated,
        }))
    }

    /// The snapshot of a segment that `which` names, with the segment's summary.
    fn chosen(&self, segment: SegmentKey, which: Which) -> Result<Option<Chosen>, StoreError> {
        let Some(started) = self.reader.started(segment)? else {
            return Ok(None);
        };
        let mut chosen = None;
        let summary = self.summarize(segment, started, |seen| match which {
            Which::First => {
                chosen.get_or_insert(seen);
            }
            Which::Latest => chosen = Some(seen),
            Which::Record(id) => {
                if seen.id == id {
                    chosen = Some(seen);
                }
            }
        })?;
        Ok(chosen.map(|seen| Chosen {
            segment: summary,
            seen,
        }))
    }

    /// The tree that a snapshot of `segment` observed.
    fn root(&self, segment: SegmentKey, seen: &Seen) -> Result<Node, StoreError> {
        let key = ContentKey {
            segment,
            hash: seen.hash,
        };
        Ok(decode(&tree::tree(&self.reader, key)?)?)
    }

    /// The segment of the snapshot `record`. `None` when `record` isn't a snapshot.
    pub fn snapshot_segment(&self, record: Uuid) -> Result<Option<SegmentKey>, StoreError> {
        let Some(raw) = self.reader.raw_record(record)? else {
            return Ok(None);
        };
        let head = raw.head()?;
        Ok(matches!(head.body, BodyKind::Observed(_)).then_some(head.segment))
    }

    /// The segment that `id` names: the segment of the snapshot `id`, or else the segment whose
    /// key is `id`, which may not exist.
    fn named_segment(&self, id: Uuid) -> Result<SegmentKey, StoreError> {
        Ok(self.snapshot_segment(id)?.unwrap_or(SegmentKey(id)))
    }

    /// Lists the snapshots of the segment that `id` names: the segment, or one of its snapshots.
    /// `None` when the segment is unknown.
    pub fn snapshots(&self, id: Uuid) -> Result<Option<SegmentSnapshots>, StoreError> {
        let segment = self.named_segment(id)?;
        let Some(started) = self.reader.started(segment)? else {
            return Ok(None);
        };
        let mut snapshots = Vec::new();
        let mut previous = None;
        let summary = self.summarize(segment, started, |seen| {
            snapshots.push(SnapshotSummary {
                record: seen.id,
                at: seen.at,
                trigger: seen.trigger,
                nodes: seen.nodes,
                same_as_previous: previous == Some(seen.hash),
            });
            previous = Some(seen.hash);
        })?;
        Ok(Some(SegmentSnapshots {
            segment: summary,
            snapshots,
        }))
    }

    /// Reads the snapshots `records` of the segment that `id` names, as texts within `budget`,
    /// in the order of the segment's snapshots. `id` names the segment or one of its snapshots.
    /// A record that isn't a snapshot of the segment is left out. `None` when the segment is
    /// unknown.
    pub fn read_snapshots(
        &self,
        id: Uuid,
        records: &HashSet<Uuid>,
        budget: TextBudget,
    ) -> Result<Option<RecordTexts>, StoreError> {
        let segment = self.named_segment(id)?;
        let Some(started) = self.reader.started(segment)? else {
            return Ok(None);
        };
        let mut chosen = Vec::new();
        self.summarize(segment, started, |seen| {
            if records.contains(&seen.id) {
                chosen.push(seen);
            }
        })?;
        let found = chosen.len();
        // A content is usually a delta of the content before it, so each expansion starts from
        // the tree that the read expanded last.
        let mut previous: Option<Expanded> = None;
        let mut texts = Vec::with_capacity(found);
        let mut total = 0_usize;
        for seen in chosen {
            let tree = match previous.take() {
                Some(known) if known.hash == seen.hash => known.tree,
                known => {
                    let key = ContentKey {
                        segment,
                        hash: seen.hash,
                    };
                    tree::tree_from(&self.reader, key, known.as_ref())?
                }
            };
            let rendered = render(&decode(&tree)?, budget.per_text);
            total = total.saturating_add(rendered.text.len());
            if total > budget.total && !texts.is_empty() {
                break;
            }
            texts.push(RecordText {
                record: seen.id,
                text: rendered.text,
                truncated: rendered.completeness == Completeness::Truncated,
            });
            previous = Some(Expanded {
                hash: seen.hash,
                tree,
            });
        }
        Ok(Some(RecordTexts {
            left_out: found - texts.len(),
            texts,
        }))
    }

    /// The totals of the segments that the filter admits and that overlap the filter's time
    /// range, per application. A segment that is still open lasts until the end of the range.
    ///
    /// A damaged segment is logged and left out of the totals. [`Tally::damaged`] counts the
    /// damaged segments and records that a deletion by the filter removes.
    pub fn tally(&self, filter: &SegmentFilter) -> Result<Tally, StoreError> {
        // Only a filter that selects every application and kind removes damaged segments and
        // records, as `selected_by` decides.
        let counts_damage = filter.selects_every_app_and_kind();
        let Candidates {
            mut segments,
            damaged,
        } = self.segment_keys_from(filter.from)?;
        let mut damage = if counts_damage {
            self.unlisted(damaged, &mut segments)?.count()
        } else {
            0
        };
        let mut totals = Totals::default();
        for key in segments {
            match self.start_of(key)? {
                Start::Intact(started) => {
                    if filter.admits(&started) && self.lasts_until(key, filter.from)? {
                        totals.add(self.summarize(key, started, |_| {})?, filter.to);
                    }
                }
                Start::Damaged => damage += usize::from(counts_damage),
                Start::Later => {}
            }
        }
        Ok(totals.into_tally(damage))
    }

    /// The last record that observed a search hit's content, when the hit's segment passes
    /// `filter` and overlaps the filter's time range.
    pub fn admit(&self, key: ContentKey, filter: &SegmentFilter) -> Result<Matched, StoreError> {
        let Some(started) = self.reader.started(key.segment)? else {
            return Ok(Matched::NoSegment);
        };
        if !filter.admits(&started) || !self.lasts_until(key.segment, filter.from)? {
            return Ok(Matched::Excluded);
        }
        let Some(seen) = self.last_observation(key)? else {
            return Ok(Matched::NoObservation);
        };
        Ok(Matched::Found(MatchedRecord {
            segment: started.into(),
            record: seen.id,
            at: seen.at,
        }))
    }

    /// The content of a search hit that [`Self::admit`] found.
    pub fn matched_content(
        &self,
        key: ContentKey,
        found: MatchedRecord,
    ) -> Result<MatchedContent, StoreError> {
        let root = decode(&tree::tree(&self.reader, key)?)?;
        Ok(MatchedContent {
            item: found.segment.into_item(key, IndexItem::text_of(&root)),
            record: found.record,
            at: found.at,
        })
    }

    /// The start of a segment, when the filter admits the segment and the segment overlaps the
    /// filter's time range.
    fn admitted(
        &self,
        key: SegmentKey,
        filter: &SegmentFilter,
    ) -> Result<Option<Started>, StoreError> {
        let Some(started) = self.reader.started(key)? else {
            // The snapshot doesn't hold the segment's start. When the snapshot doesn't hold any
            // other record of the segment either, the segment started after the snapshot was
            // taken.
            if self.reader.segment_record_ids(key).next().is_none() {
                return Ok(None);
            }
            return Err(StoreError::Corrupt("a segment doesn't have a start"));
        };
        // The segment's last record alone shows whether the segment ended before the range, so
        // the check doesn't read the segment's other records.
        if !filter.admits(&started) || !self.lasts_until(key, filter.from)? {
            return Ok(None);
        }
        Ok(Some(started))
    }

    /// Summarizes a segment, and hands each of its snapshots to `seen` in order.
    ///
    /// A segment that is not open and doesn't have an end record belongs to a helper run that is
    /// over, so the summary ends the segment at its last record. A damaged record is logged and
    /// left out.
    fn summarize(
        &self,
        key: SegmentKey,
        started: Started,
        mut seen: impl FnMut(Seen),
    ) -> Result<SegmentSummary, StoreError> {
        let mut ended = None;
        let mut last = started.at;
        let mut snapshots: u32 = 0;
        for headed in self.reader.heads_after_start(key) {
            let headed = match headed {
                Ok(headed) => headed,
                Err(err) if err.is_damage() => {
                    warn!(%err, segment = %key, "skipped a damaged desktop capture record");
                    continue;
                }
                Err(err) => return Err(err),
            };
            last = headed.head.at;
            if headed.head.body == BodyKind::SegmentEnded {
                ended = Some(headed.head.at.into());
            } else if let Some(observation) = Seen::of(&headed) {
                snapshots = snapshots.saturating_add(1);
                seen(observation);
            }
        }
        if ended.is_none() && !self.open.contains(&key) {
            ended = Some(last.into());
        }
        let SegmentStart {
            kind,
            app,
            title,
            url,
        } = started.start;
        Ok(SegmentSummary {
            key,
            kind,
            app,
            title,
            url,
            started: started.at.into(),
            ended,
            snapshots,
        })
    }

    /// Whether `segment` is open or ended at `from` or later.
    ///
    /// The last record of an ended segment is the segment's end. A finished helper run can leave
    /// a segment without an end, and [`Self::summarize`] ends that segment at its last intact
    /// record.
    fn lasts_until(&self, segment: SegmentKey, from: SystemTime) -> Result<bool, StoreError> {
        if self.open.contains(&segment) {
            return Ok(true);
        }
        let last = self.reader.last_intact_head(segment)?;
        Ok(last.is_some_and(|last| SystemTime::from(last.head.at) >= from))
    }

    /// Whether `segment` can be open or can have ended at `from` or later. A segment whose last
    /// record is damaged can have ended at any time.
    fn may_last_until(&self, segment: SegmentKey, from: SystemTime) -> Result<bool, StoreError> {
        if self.open.contains(&segment) {
            return Ok(true);
        }
        match self.reader.last_head(segment) {
            Ok(last) => Ok(SystemTime::from(last.head.at) >= from),
            Err(err) if err.is_damage() => {
                warn!(%err, %segment, "a desktop capture segment has a damaged last record");
                Ok(true)
            }
            Err(err) => Err(err),
        }
    }

    /// The last `Observed` record of a content.
    fn last_observation(&self, key: ContentKey) -> Result<Option<Seen>, StoreError> {
        for headed in self.reader.heads_after_start(key.segment).rev() {
            if let Some(seen) = Seen::of(&headed?)
                && seen.hash == key.hash
            {
                return Ok(Some(seen));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::super::write;
    use super::*;
    use crate::desktop_capture::testing::records::everything;
    use crate::desktop_capture::testing::{HOST, dir};

    /// How long a paused read waits for a write that runs during the pause.
    const WRITE_WAIT: Duration = Duration::from_millis(200);

    struct Store {
        keyspaces: Keyspaces,
        open: OpenSegments,
        _dir: TempDir,
    }

    #[fixture]
    fn store(dir: TempDir) -> Store {
        Store {
            keyspaces: Keyspaces::open(dir.path(), HOST).expect("open"),
            open: OpenSegments::default(),
            _dir: dir,
        }
    }

    fn start() -> SegmentStart {
        SegmentStart {
            kind: SegmentKind::Active,
            app: "gedit".to_owned(),
            title: "brief".to_owned(),
            url: None,
        }
    }

    /// Where a read pauses.
    #[derive(Clone, Copy)]
    enum Pause {
        BeforeSnapshot,
        AfterSnapshot,
    }

    /// Lists every segment in a view whose creation pauses at `pause` while `write` runs.
    ///
    /// The view resumes when `write` finishes, or after [`WRITE_WAIT`] when `write` waits for the
    /// view.
    fn list_during(
        store: &Store,
        pause: Pause,
        write: impl FnOnce() + Send,
    ) -> Vec<SegmentSummary> {
        let (paused_tx, paused) = mpsc::channel();
        let (resume, resumed) = mpsc::channel::<()>();
        store.keyspaces.wrap_next_snapshot(move |take| {
            let wait = move || {
                paused_tx.send(()).expect("send the pause");
                let _ = resumed.recv();
            };
            match pause {
                Pause::BeforeSnapshot => {
                    wait();
                    take()
                }
                Pause::AfterSnapshot => {
                    let snapshot = take();
                    wait();
                    snapshot
                }
            }
        });
        thread::scope(|scope| {
            let read =
                scope.spawn(|| View::new(&store.keyspaces, &store.open).segments(&everything()));
            paused.recv().expect("the view pauses");
            let write = scope.spawn(write);
            let deadline = Instant::now() + WRITE_WAIT;
            while !write.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            resume.send(()).expect("resume the view");
            write.join().expect("the write");
            read.join().expect("the read").expect("segments")
        })
    }

    /// A view reads a segment as open when the segment ends after the view's snapshot, even
    /// when the end leaves the open segments while the view is being created.
    #[rstest]
    fn a_segment_that_ends_during_a_view_reads_as_open(store: Store) {
        let segment = write::start(&store.keyspaces, &store.open, SystemTime::now(), &start())
            .expect("start");

        let listed = list_during(&store, Pause::AfterSnapshot, || {
            write::end(&store.keyspaces, &store.open, segment.key, SystemTime::now()).expect("end");
        });

        let listed: Vec<_> = listed.iter().map(|summary| (summary.key, summary.ended)).collect();
        assert_eq!(listed, [(segment.key, None)]);
    }

    /// A segment that starts while a view is being created is left out of the view or reads as
    /// open.
    #[rstest]
    fn a_segment_that_starts_during_a_view_does_not_read_as_ended(store: Store) {
        let listed = list_during(&store, Pause::BeforeSnapshot, || {
            write::start(&store.keyspaces, &store.open, SystemTime::now(), &start())
                .expect("start");
        });

        assert!(listed.iter().all(|summary| summary.ended.is_none()), "{listed:?}");
    }

    /// A segment can start after a listing takes its snapshot and end before the listing reads
    /// the segment. The listing leaves out such a segment.
    #[rstest]
    fn a_segment_that_starts_and_ends_during_a_listing_is_left_out(store: Store) {
        let Store {
            keyspaces, open, ..
        } = &store;
        let view = View::new(keyspaces, open);
        let segment = write::start(keyspaces, open, SystemTime::now(), &start()).expect("start");
        write::end(keyspaces, open, segment.key, SystemTime::now()).expect("end");

        let admitted = view.admitted(segment.key, &everything()).expect("admitted");
        drop(view);
        assert!(admitted.is_none());
    }
}
