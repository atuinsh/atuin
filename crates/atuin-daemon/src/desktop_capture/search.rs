//! Search over the contents that the store holds, through the search index.

use std::collections::HashMap;
use std::sync::Arc;

use futures::{StreamExt as _, stream};
use tracing::warn;

use super::index::{DesktopIndex, IndexError};
use super::store::{
    Admission, ContentKey, DesktopStore, Matched, MatchedContent, MatchedRecord, SegmentFilter,
    SegmentKey,
};

/// The most candidates that one search takes from the index.
const MAX_CANDIDATES: u32 = 2_000;

/// The fewest candidates that the store judges at once.
const MIN_JUDGED_AT_ONCE: usize = 8;

/// How many contents the search reads from the store at once.
const CONCURRENT_READS: usize = 8;

/// The contents that match `query` and `filter` and that the store still holds, best first, up
/// to `limit`.
///
/// Search removes the index rows of the segments and the contents that the store no longer
/// holds.
pub async fn search(
    store: &DesktopStore,
    index: &DesktopIndex,
    query: &str,
    filter: SegmentFilter,
    limit: u32,
) -> Result<Vec<MatchedContent>, IndexError> {
    let search = Search {
        store,
        index,
        query,
        filter: Arc::new(filter),
        limit,
        verdicts: HashMap::new(),
        read: HashMap::new(),
    };
    search.run().await
}

/// One search, which takes more candidates from the index in each round until it has enough
/// hits.
struct Search<'a> {
    store: &'a DesktopStore,
    index: &'a DesktopIndex,
    query: &'a str,
    filter: Arc<SegmentFilter>,
    limit: u32,
    /// What the store said of each candidate that the search has judged, which later rounds
    /// reuse.
    verdicts: HashMap<ContentKey, Verdict>,
    /// The contents that earlier rounds read.
    read: HashMap<ContentKey, MatchedContent>,
}

/// What a search found of one candidate.
#[derive(Debug, Clone)]
enum Verdict {
    Judged(Matched),
    /// The store could not judge the candidate, or admitted the candidate and then could not
    /// read its content.
    Unreadable,
}

/// A candidate that the store admitted.
struct Admitted {
    key: ContentKey,
    found: MatchedRecord,
}

/// The candidates of one round of a search that the store admits, and the rows of the index
/// that are stale.
struct Found {
    admitted: Vec<Admitted>,
    /// The segments that the store no longer holds.
    stale_segments: Vec<SegmentKey>,
    /// The contents that no record of their segment observed.
    stale_contents: Vec<ContentKey>,
    /// Whether the search took every candidate that the index holds.
    exhausted: bool,
}

impl Found {
    const fn has_stale(&self) -> bool {
        !self.stale_segments.is_empty() || !self.stale_contents.is_empty()
    }

    async fn remove_stale(&self, index: &DesktopIndex) -> Result<(), IndexError> {
        index.remove(&self.stale_segments).await?;
        index.remove_contents(&self.stale_contents).await
    }
}

/// The contents of a round's admitted candidates.
struct Contents {
    /// The contents that the store read, in the order of the candidates.
    hits: Vec<MatchedContent>,
    /// The candidates whose content the store could not read.
    unreadable: Vec<ContentKey>,
}

impl Search<'_> {
    async fn run(mut self) -> Result<Vec<MatchedContent>, IndexError> {
        let mut candidates = self.limit;
        loop {
            let found = self.round(candidates).await?;
            // A segment or a content that the store deleted after the index learned of it
            // leaves stale rows, which take the places of other hits. Removing them and
            // searching again fills those places.
            if found.has_stale() {
                match found.remove_stale(self.index).await {
                    Ok(()) => continue,
                    Err(err) => warn!(?err, "could not remove stale desktop capture search hits"),
                }
            }
            // The index cannot apply every part of the filter, so the candidates that the store
            // excludes take the places of other hits. Taking more candidates fills those
            // places.
            let full = found.admitted.len() >= self.limit as usize;
            if full || found.exhausted || candidates >= MAX_CANDIDATES {
                let contents = self.contents(found.admitted).await;
                // A hit whose content the store can't read takes the place of another hit.
                // When the round had enough hits, more candidates may follow, so another round
                // fills that place.
                if contents.unreadable.is_empty() || !full {
                    return Ok(contents.hits);
                }
                let unreadable = contents.unreadable.into_iter();
                self.verdicts.extend(unreadable.map(|key| (key, Verdict::Unreadable)));
                self.read.extend(contents.hits.into_iter().map(|hit| (hit.item.key, hit)));
                continue;
            }
            candidates = candidates.saturating_mul(4).min(MAX_CANDIDATES);
        }
    }

    /// Takes the best `candidates` of the index, and judges them in order until the store has
    /// admitted [`Self::limit`] of them.
    ///
    /// The store judges each candidate once per search.
    async fn round(&mut self, candidates: u32) -> Result<Found, IndexError> {
        let keys = self.index.search(self.query, &self.filter, candidates).await?;
        let mut found = Found {
            admitted: Vec::new(),
            stale_segments: Vec::new(),
            stale_contents: Vec::new(),
            exhausted: keys.len() < candidates as usize,
        };
        let limit = self.limit as usize;
        let mut rest = keys.as_slice();
        while found.admitted.len() < limit && !rest.is_empty() {
            let wanted = (limit - found.admitted.len()).max(MIN_JUDGED_AT_ONCE);
            let next;
            (next, rest) = rest.split_at(wanted.min(rest.len()));
            self.judge(next).await;
            for &key in next {
                if found.admitted.len() >= limit {
                    break;
                }
                match &self.verdicts[&key] {
                    Verdict::Judged(Matched::Found(record)) => found.admitted.push(Admitted {
                        key,
                        found: record.clone(),
                    }),
                    Verdict::Judged(Matched::NoSegment) => found.stale_segments.push(key.segment),
                    Verdict::Judged(Matched::NoObservation) => found.stale_contents.push(key),
                    Verdict::Judged(Matched::Excluded) | Verdict::Unreadable => {}
                }
            }
        }
        Ok(found)
    }

    /// Has the store judge each of `keys` that the search has not judged yet, in one read of
    /// the store.
    async fn judge(&mut self, keys: &[ContentKey]) {
        let new: Vec<ContentKey> =
            keys.iter().copied().filter(|key| !self.verdicts.contains_key(key)).collect();
        if new.is_empty() {
            return;
        }
        match self.store.admit_all(new, Arc::clone(&self.filter)).await {
            Ok(judged) => {
                for Admission { key, matched } in judged {
                    let verdict = matched.map_or_else(
                        |err| {
                            warn!(?err, "could not read a desktop capture search hit");
                            Verdict::Unreadable
                        },
                        Verdict::Judged,
                    );
                    self.verdicts.insert(key, verdict);
                }
            }
            Err(err) => {
                warn!(?err, "could not read the desktop capture search hits");
                for &key in keys {
                    self.verdicts.entry(key).or_insert(Verdict::Unreadable);
                }
            }
        }
    }

    /// Reads the contents of the admitted candidates, in order.
    ///
    /// A content that an earlier round read comes from [`Self::read`]. A content that the store
    /// cannot read is logged and left out.
    async fn contents(&mut self, admitted: Vec<Admitted>) -> Contents {
        let mut contents = Contents {
            hits: Vec::with_capacity(admitted.len()),
            unreadable: Vec::new(),
        };
        let store = self.store;
        let read = &mut self.read;
        let mut reads = stream::iter(admitted)
            .map(|Admitted { key, found }| {
                let earlier = read.remove(&key);
                async move {
                    let content = match earlier {
                        Some(content) => Ok(content),
                        None => store.matched_content(key, found).await,
                    };
                    (key, content)
                }
            })
            .buffered(CONCURRENT_READS);
        while let Some((key, content)) = reads.next().await {
            match content {
                Ok(content) => contents.hits.push(content),
                Err(err) => {
                    warn!(?err, "could not read a desktop capture search hit");
                    contents.unreadable.push(key);
                }
            }
        }
        contents
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use atuin_desktop_capture::ContentHash;
    use rstest::rstest;
    use uuid::Uuid;

    use super::*;
    use crate::desktop_capture::store::{Kinds, SegmentKind};
    use crate::desktop_capture::testing::records::{
        ended, everything, notification, snapshot, started, window, written,
    };
    use crate::desktop_capture::testing::{Opened, indexed_keys, opened};

    fn keys(hits: &[MatchedContent]) -> Vec<ContentKey> {
        hits.iter().map(|hit| hit.item.key).collect()
    }

    fn ago(secs: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(secs)
    }

    /// Runs in real time, because the index's pool times out while the clock is paused.
    #[rstest]
    #[tokio::test]
    async fn a_notification_is_found_by_its_body(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let items = written(&store, [notification("disk", ago(5))]).await;
        index.add(items).await.expect("index");
        let filter = SegmentFilter {
            kinds: Kinds::Only(vec![SegmentKind::Notification]),
            ..everything()
        };

        let hits = search(&store, &index, "body disk", filter, 5).await.expect("search");

        let segments = store.segments(everything()).await.expect("segments");
        assert_eq!(segments.len(), 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item.key.segment, segments[0].key);
        assert_eq!(hits[0].item.kind, SegmentKind::Notification);
    }

    /// Search removes the rows of a segment that the store deleted, and the row of a content
    /// that its segment never observed, and searches again to fill their places.
    ///
    /// Runs in real time, because the index's pool times out while the clock is paused.
    #[rstest]
    #[tokio::test]
    async fn search_removes_the_hits_of_deleted_segments_and_contents(
        #[future(awt)] opened: Opened,
    ) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let items = written(&store, [
            started(1, "gedit", "notes", ago(5)),
            snapshot(1, ago(4), window("quokka")),
        ])
        .await;
        let found = items[0].clone();
        // This row stands for a content that the store lost. For example, a crash can lose a
        // content. Of two rows that rank the same, the newer comes first, so this row ranks
        // first.
        let mut unobserved = found.clone();
        unobserved.key.hash = ContentHash([7; 32]);
        // The deleted segment's row ranks before the lost content's row.
        let mut deleted = found.clone();
        deleted.key.segment = SegmentKey(Uuid::now_v7());
        index.add([found.clone(), unobserved.clone(), deleted]).await.expect("index");

        let hits = search(&store, &index, "quokka", everything(), 1).await.expect("search");

        assert_eq!(keys(&hits), [found.key]);
        assert_eq!(indexed_keys(&index, "quokka").await, [found.key]);
    }

    /// The index cannot tell when a segment ended, so search takes more candidates when the
    /// store excludes the ones that ended before the filter's range.
    ///
    /// Each hit is expanded once, however many rounds the search takes.
    ///
    /// Runs in real time, because the index's pool times out while the clock is paused.
    #[rstest]
    #[tokio::test]
    async fn search_takes_more_candidates_when_the_store_excludes_some(
        #[future(awt)] opened: Opened,
    ) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        // Of rows that rank the same, the newest comes first. The index's first candidate is
        // therefore the second recent segment's row, and the index's last candidate is the
        // first recent segment's row.
        let first = written(&store, [
            started(1, "gedit", "recent", ago(10)),
            snapshot(1, ago(9), window("needle")),
        ])
        .await;
        index.add(first.clone()).await.expect("index");
        for n in 2..30 {
            let old = written(&store, [
                started(n, "gedit", "old", ago(1000)),
                snapshot(n, ago(999), window("needle")),
                ended(n, ago(998)),
            ])
            .await;
            index.add(old).await.expect("index");
        }
        let second = written(&store, [
            started(30, "gedit", "recent", ago(10)),
            snapshot(30, ago(9), window("needle")),
        ])
        .await;
        index.add(second.clone()).await.expect("index");
        let filter = SegmentFilter {
            from: ago(100),
            ..everything()
        };
        let before = store.expansions();

        let hits = search(&store, &index, "needle", filter, 2).await.expect("search");

        assert_eq!(keys(&hits), [second[0].key, first[0].key]);
        assert_eq!(store.expansions() - before, 2, "each hit is expanded once");
    }

    /// Runs in real time, because the index's pool times out while the clock is paused.
    #[rstest]
    #[tokio::test]
    async fn search_leaves_out_hits_that_do_not_read(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let mut items = written(&store, [
            started(1, "gedit", "notes", ago(5)),
            snapshot(1, ago(4), window("quokka")),
        ])
        .await;
        let mut missing = items[0].clone();
        missing.key.segment = SegmentKey(Uuid::now_v7());
        items.push(missing);
        index.add(items).await.expect("index");

        let hits = search(&store, &index, "quokka", everything(), 20).await.expect("search");

        let texts: Vec<_> = hits.iter().map(|hit| hit.item.text.as_str()).collect();
        assert_eq!(texts, ["quokka"]);
    }

    /// A hit whose content the store can't read gives its place to the next candidate.
    ///
    /// Runs in real time, because the index's pool times out while the clock is paused.
    #[rstest]
    #[case::one(1)]
    #[case::more_than_there_are(5)]
    #[tokio::test]
    async fn search_fills_the_place_of_a_content_that_does_not_read(
        #[future(awt)] opened: Opened,
        #[case] limit: u32,
    ) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let older = written(&store, [
            started(1, "gedit", "older", ago(10)),
            snapshot(1, ago(9), window("quokka")),
        ])
        .await;
        // Of two rows that rank the same, the newer comes first.
        let damaged = written(&store, [
            started(2, "gedit", "damaged", ago(5)),
            snapshot(2, ago(4), window("quokka")),
        ])
        .await;
        index.add(older.iter().chain(&damaged).cloned()).await.expect("index");
        store.put_raw_content(damaged[0].key, b"not a content");

        let hits = search(&store, &index, "quokka", everything(), limit).await.expect("search");

        assert_eq!(keys(&hits), [older[0].key]);
    }
}
