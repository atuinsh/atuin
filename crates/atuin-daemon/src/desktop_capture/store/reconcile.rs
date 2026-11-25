//! Passes over the stored contents, for the reconciler to rebuild the search index from.
//!
//! A batch of a pass first lists the contents that it meets for the first time. The reconciler
//! asks the search index which of them the index lacks, and the pass then expands only those.

use std::collections::{HashMap, HashSet, hash_map};

use atuin_desktop_capture::ContentHash;
use atuin_desktop_capture::encoding::decode;
use tracing::warn;
use uuid::Uuid;

use super::keyspaces::Reader;
use super::model::{ContentKey, IndexItem, SegmentInfo, SegmentKey, StoreError};
use super::schema::{self, BodyKind};
use super::tree::{self, Expanded};

/// One pass of the reconciler over the records after a watermark.
///
/// A pass keeps the last tree that it expanded in each open segment, so that it decodes each
/// delta chain about once.
#[derive(Debug, Default)]
pub struct Pass {
    /// The last record that the pass has read.
    through: Option<Uuid>,
    /// The segments that the pass met and has not seen end yet.
    segments: HashMap<SegmentKey, Met>,
    /// The segments whose end the last batch read, which the pass forgets when the next batch
    /// starts.
    ended: Vec<SegmentKey>,
}

/// A segment that a pass met.
#[derive(Debug)]
enum Met {
    Readable(SegmentPass),
    /// The segment's start is missing or damaged, so the pass skips the segment's contents.
    Skipped,
}

/// What a pass knows of one segment.
#[derive(Debug)]
struct SegmentPass {
    info: SegmentInfo,
    /// The contents of the segment that the pass has handed out.
    done: HashSet<ContentHash>,
    last: Option<Expanded>,
}

/// One batch of a reconciliation pass, from
/// [`DesktopStore::reconcile_batch`](super::DesktopStore::reconcile_batch).
#[derive(Debug)]
pub struct Batch {
    /// The contents that the batch met for the first time in its pass, in the order of their
    /// first observation.
    pub candidates: Vec<ContentKey>,
    /// The last record that the batch read. `None` when the batch didn't find any record.
    pub through: Option<Uuid>,
    /// The pass, which continues after the batch.
    pub pass: Pass,
}

/// The contents that a pass expanded, from
/// [`DesktopStore::expand_candidates`](super::DesktopStore::expand_candidates).
#[derive(Debug)]
pub struct Expansion {
    /// The items of the search index for the contents, in order.
    pub items: Vec<IndexItem>,
    /// The pass, which continues after its last batch.
    pub pass: Pass,
}

impl Pass {
    /// A pass over the records after the record `through`, or over every record.
    #[must_use]
    pub fn after(through: Option<Uuid>) -> Self {
        Self {
            through,
            ..Self::default()
        }
    }

    /// The next batch of the pass: the contents of the `Observed` records among the next
    /// `limit` records after the watermark, each content once per pass.
    ///
    /// The batch leaves every content unexpanded. A record that is damaged is logged and
    /// skipped.
    pub fn next_batch(mut self, reader: &Reader<'_>, limit: usize) -> Result<Batch, StoreError> {
        for segment in self.ended.drain(..) {
            self.segments.remove(&segment);
        }
        let mut candidates = Vec::new();
        let from = match self.through {
            None => Some([0; 16]),
            Some(through) => schema::key_after(through),
        };
        let Some(from) = from else {
            return Ok(Batch {
                candidates,
                through: None,
                pass: self,
            });
        };
        let mut through = None;
        for raw in reader.records_from(from).take(limit) {
            let raw = raw?;
            through = Some(raw.id);
            let id = raw.id;
            let observed = raw.head().map(|head| match head.body {
                BodyKind::Observed(observed) => Some(ContentKey {
                    segment: head.segment,
                    hash: observed.hash,
                }),
                // An ended segment doesn't gain any more contents, so the pass can drop the
                // segment's last tree.
                BodyKind::SegmentEnded => {
                    self.ended.push(head.segment);
                    None
                }
                BodyKind::SegmentStarted(_) => None,
            });
            let ContentKey { segment, hash } = match observed {
                Ok(Some(key)) => key,
                Ok(None) => continue,
                Err(err) if err.is_damage() => {
                    warn!(%err, record = %id, "skipped a damaged desktop capture record");
                    continue;
                }
                Err(err) => return Err(err),
            };
            if let Some(known) = self.segment(reader, segment)?
                && known.done.insert(hash)
            {
                candidates.push(ContentKey { segment, hash });
            }
        }
        self.through = through.or(self.through);
        Ok(Batch {
            candidates,
            through,
            pass: self,
        })
    }

    /// The items of the search index for `wanted`, which are candidates of the pass's last
    /// batch, in order.
    ///
    /// A content whose stored tree is damaged is logged and left out.
    pub fn expand(
        &mut self,
        reader: &Reader<'_>,
        wanted: impl IntoIterator<Item = ContentKey>,
    ) -> Result<Vec<IndexItem>, StoreError> {
        let mut items = Vec::new();
        for key in wanted {
            let Some(Met::Readable(known)) = self.segments.get_mut(&key.segment) else {
                return Err(StoreError::Corrupt("a pass expanded a content that it never met"));
            };
            match known.expand(reader, key) {
                Ok(item) => items.push(item),
                Err(err) if err.is_damage() => {
                    warn!(%err, segment = %key.segment, "skipped a desktop capture content whose stored tree is damaged");
                }
                Err(err) => return Err(err),
            }
        }
        Ok(items)
    }

    /// What the pass knows of `segment`. `None` when the segment's start is missing or damaged.
    fn segment(
        &mut self,
        reader: &Reader<'_>,
        segment: SegmentKey,
    ) -> Result<Option<&mut SegmentPass>, StoreError> {
        let known = match self.segments.entry(segment) {
            hash_map::Entry::Occupied(known) => known.into_mut(),
            hash_map::Entry::Vacant(vacant) => {
                let started = match reader.started(segment) {
                    Ok(Some(started)) => Some(started),
                    Ok(None) => {
                        warn!(%segment, "a desktop capture segment doesn't have a start");
                        None
                    }
                    Err(err) if err.is_damage() => {
                        warn!(%err, %segment, "a desktop capture segment's start is damaged");
                        None
                    }
                    Err(err) => return Err(err),
                };
                vacant.insert(started.map_or(Met::Skipped, |started| {
                    Met::Readable(SegmentPass {
                        info: started.into(),
                        done: HashSet::new(),
                        last: None,
                    })
                }))
            }
        };
        Ok(match known {
            Met::Readable(known) => Some(known),
            Met::Skipped => None,
        })
    }
}

impl SegmentPass {
    /// The item of the search index for a content of the segment.
    fn expand(&mut self, reader: &Reader<'_>, key: ContentKey) -> Result<IndexItem, StoreError> {
        let tree = tree::tree_from(reader, key, self.last.as_ref())?;
        let text = IndexItem::text_of(&decode(&tree)?);
        self.last = Some(Expanded {
            hash: key.hash,
            tree,
        });
        Ok(self.info.item(key, text))
    }
}
