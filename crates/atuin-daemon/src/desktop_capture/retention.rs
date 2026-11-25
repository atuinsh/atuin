//! Keeping desktop capture under its disk budget.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::warn;

use super::index::DesktopIndex;
use super::store::{DesktopStore, Evicted, StoreError};
use crate::disk_budget::Evict;

/// The shortest time between two rewrites after evictions.
const REWRITE_INTERVAL: Duration = Duration::from_hours(2);

/// The store and the index, which a [`Gc`](crate::disk_budget::Gc) evicts from together.
pub struct Retention {
    store: DesktopStore,
    index: Option<DesktopIndex>,
    rewrites: Mutex<Rewrites>,
}

impl Retention {
    pub fn new(store: DesktopStore, index: Option<DesktopIndex>) -> Self {
        Self {
            store,
            index,
            rewrites: Mutex::default(),
        }
    }
}

/// When the evictions call for a [rewrite](DesktopStore::rewrite) of the store.
///
/// The store's estimate of its size starts at its size on disk when it opens, and fjall keeps
/// the space of a removal on disk until a rewrite. A rewrite keeps the evicted space from
/// counting again after a restart. Without one, the next eviction would remove more than the
/// budget calls for. A rewrite also removes the evicted records from fjall's journal. A rewrite
/// copies the whole store, so the store is rewritten only once the evictions since the last
/// rewrite add up to a quarter of the store. The store counts those evictions across restarts.
/// The store is also rewritten at most once per [`REWRITE_INTERVAL`].
#[derive(Debug, Default)]
struct Rewrites {
    /// When the last rewrite after evictions began, whether or not it succeeded.
    last: Option<Instant>,
}

impl Rewrites {
    /// Begins a rewrite at `now` when evictions that freed `evicted` bytes make a store that uses
    /// `usage` bytes due for one.
    fn begin_if_due(&mut self, evicted: u64, usage: u64, now: Instant) -> Result<(), NotDue> {
        let large = evicted > 0 && evicted >= usage / 4;
        let rested = self.last.is_none_or(|last| now.duration_since(last) >= REWRITE_INTERVAL);
        if !(large && rested) {
            return Err(NotDue);
        }
        self.last = Some(now);
        Ok(())
    }
}

/// The evictions don't call for a rewrite yet.
#[derive(Debug, PartialEq, Eq)]
struct NotDue;

impl Evict for Retention {
    type Error = StoreError;

    fn estimated_disk_space(&self) -> u64 {
        self.store.estimated_disk_space()
    }

    /// Evicts from the store, removes the evicted segments from the index, and rewrites the
    /// store and scrubs the index when the evictions since the last rewrite freed a large share
    /// of the store.
    ///
    /// A failure to remove from the index or to scrub the index is only logged. The store notes
    /// the index's removal with each eviction, and the daemon retries the removal until it
    /// succeeds. The scrub after a later rewrite drops the evicted text from the index's files.
    async fn evict(&self, bytes: u64) -> Result<(), StoreError> {
        match &self.index {
            Some(index) => self.evict_from_both(index, bytes).await?,
            None => {
                let _ = self.store.evict(bytes).await?;
            }
        }
        self.rewrite_if_due().await;
        Ok(())
    }
}

impl Retention {
    /// Evicts from the store, and then removes the evicted segments from `index`.
    async fn evict_from_both(&self, index: &DesktopIndex, bytes: u64) -> Result<(), StoreError> {
        let _following = index.follow_store().await;
        let owed_before = self.store.owes_index_removal().await?;
        let Evicted { segments, .. } = self.store.evict(bytes).await?;
        if segments.is_empty() {
            return Ok(());
        }
        if let Err(err) = index.remove_evicted(&segments).await {
            warn!(?err, "could not remove evicted segments from the desktop capture index");
            return Ok(());
        }
        // A removal that the index owed before this eviction is still waiting for the daemon's
        // retry.
        if !owed_before && let Err(err) = self.store.settle_index_removal().await {
            warn!(?err, "could not note that the desktop capture index followed an eviction");
        }
        Ok(())
    }

    /// Rewrites the store and scrubs the index when the evictions since the last rewrite call
    /// for a rewrite.
    ///
    /// A failure is only logged, because the evictions themselves succeeded. A failed rewrite
    /// still waits [`REWRITE_INTERVAL`] before the evictions call for the next one, because
    /// evictions often happen on a disk that is nearly full, where a copy of the store fails.
    /// The store counts the evicted bytes until a rewrite succeeds, so the next call that is due
    /// tries again.
    pub async fn rewrite_if_due(&self) {
        let evicted = match self.store.evicted_since_rewrite().await {
            Ok(evicted) => evicted,
            Err(err) => {
                warn!(?err, "could not read the bytes that desktop capture evicted");
                return;
            }
        };
        let usage = self.store.estimated_disk_space();
        if self.rewrites.lock().begin_if_due(evicted, usage, Instant::now()).is_err() {
            return;
        }
        if let Err(err) = self.store.rewrite().await {
            warn!(?err, "could not rewrite the desktop capture store after evictions");
        }
        if let Some(index) = &self.index
            && let Err(err) = index.scrub().await
        {
            warn!(?err, "could not scrub the desktop capture index after evictions");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use atuin_desktop_capture::Node;
    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::desktop_capture::testing::records::{dialog, everything, noise, window, written};
    use crate::desktop_capture::testing::{
        Opened, dir, files_holding, index_in, indexed_keys, opened, store_in,
    };

    #[rstest]
    #[tokio::test]
    async fn eviction_removes_evicted_segments_from_the_index(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        for title in ["first", "second"] {
            let items =
                written(&store, [dialog("gedit", title, SystemTime::now(), Node::default())]).await;
            index.add(items).await.expect("index");
        }
        let retention = Retention::new(store, Some(index.clone()));

        retention.evict(1).await.expect("evict");
        assert!(indexed_keys(&index, "first").await.is_empty());
        assert_eq!(indexed_keys(&index, "second").await.len(), 1);
    }

    /// An eviction waits while a reconciliation is between reading the store and indexing what
    /// the reconciliation read, so that the reconciliation can't index a segment after the
    /// eviction has removed the segment's rows.
    #[rstest]
    #[tokio::test]
    async fn eviction_waits_for_the_index_to_follow_the_store(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let items = written(&store, [dialog("gedit", "first", SystemTime::now(), Node::default())]);
        index.add(items.await).await.expect("index");
        let retention = Retention::new(store, Some(index.clone()));
        let following = index.follow_store().await;

        let mut eviction = std::pin::pin!(retention.evict(1));
        let waited = tokio::time::timeout(Duration::from_millis(200), eviction.as_mut()).await;
        assert!(waited.is_err(), "the eviction finished while the index was following");
        assert_eq!(indexed_keys(&index, "first").await.len(), 1);
        drop(following);

        eviction.await.expect("evict");
        assert!(indexed_keys(&index, "first").await.is_empty());
    }

    /// An eviction that frees a large share of the store rewrites the store, so that the space
    /// that the eviction freed does not count again when the store opens again.
    #[rstest]
    #[tokio::test]
    async fn evicted_space_stays_freed_after_a_restart(dir: TempDir) {
        let store = store_in(&dir).await;
        let records = (0..100)
            .map(|n| dialog("gedit", "noise", SystemTime::now(), noise(&format!("dialog {n}"))));
        let _ = written(&store, records).await;
        // The writes reach the tables, as they do in a store that has run for a while.
        store.rewrite().await.expect("rewrite");
        store.close().await.expect("close");
        let store = store_in(&dir).await;
        let full = store.estimated_disk_space();

        let retention = Retention::new(store, None);
        // An eviction of one dialog frees too little to rewrite the store.
        retention.evict(1).await.expect("evict");
        assert_eq!(retention.rewrites.lock().last, None);
        retention.evict(u64::MAX).await.expect("evict");
        assert!(retention.rewrites.lock().last.is_some());
        retention.store.close().await.expect("close");

        for _ in 0..2 {
            let store = store_in(&dir).await;
            let reopened = store.estimated_disk_space();
            assert!(
                reopened < full * 3 / 4,
                "{reopened} bytes after the restart, {full} bytes before the eviction"
            );
            store.close().await.expect("close");
        }
    }

    /// An eviction that rewrites the store doesn't leave any copy of the evicted text in any file
    /// of the store or the index.
    #[rstest]
    #[tokio::test]
    async fn a_rewrite_after_an_eviction_leaves_no_trace_in_the_data_files(dir: TempDir) {
        const SECRET: &str = "zanzibar quokka 7319";
        let store = store_in(&dir).await;
        let index = index_in(&dir).await;
        let earlier = SystemTime::now() - Duration::from_hours(1);
        let items = written(&store, [
            dialog("kate", SECRET, earlier, noise(SECRET)),
            dialog("gedit", "kept", SystemTime::now(), window("kept")),
        ])
        .await;
        index.add(items).await.expect("index");
        // The index's full-text segments hold each word of the text on its own.
        let holding_secret = || {
            SECRET.split(' ').flat_map(|word| files_holding(dir.path(), word)).collect::<Vec<_>>()
        };
        let holding = holding_secret();
        let store_files = dir.path().join("store");
        assert!(
            holding.iter().any(|path| path.starts_with(&store_files)),
            "the store's journal holds the text"
        );
        assert!(
            holding.iter().any(|path| !path.starts_with(&store_files)),
            "the index holds the text"
        );
        let retention = Retention::new(store, Some(index));

        // The evictions before this one add up to a quarter of the store, so this one is due
        // for a rewrite.
        retention.store.add_evicted(retention.store.estimated_disk_space() / 4);

        retention.evict(1).await.expect("evict");

        assert!(retention.rewrites.lock().last.is_some(), "the eviction rewrote the store");
        assert_eq!(holding_secret(), Vec::<std::path::PathBuf>::new());
        let segments = retention.store.segments(everything()).await.expect("segments");
        assert_eq!(segments.iter().map(|segment| &*segment.title).collect::<Vec<_>>(), ["kept"]);
        retention.store.close().await.expect("close");
    }

    /// One step of the evictions that [`Rewrites`] sees.
    #[derive(Debug, Clone, Copy)]
    enum Step {
        /// An eviction that freed the bytes, with the store's usage after it, at the minute.
        Evicted {
            freed: u64,
            usage: u64,
            minute: u64,
        },
        /// A rewrite that succeeded at the minute.
        Rewritten {
            minute: u64,
        },
        /// A rewrite that failed at the minute.
        Failed {
            minute: u64,
        },
    }

    #[rstest]
    #[case::nothing_freed(&[], Step::Evicted { freed: 0, usage: 0, minute: 0 }, false)]
    #[case::a_small_share(&[], Step::Evicted { freed: 10, usage: 100, minute: 0 }, false)]
    #[case::a_quarter(&[], Step::Evicted { freed: 25, usage: 100, minute: 0 }, true)]
    #[case::small_shares_add_up(
        &[Step::Evicted { freed: 15, usage: 100, minute: 0 }],
        Step::Evicted { freed: 15, usage: 100, minute: 1 },
        true,
    )]
    #[case::a_rewrite_starts_over(
        &[Step::Evicted { freed: 30, usage: 100, minute: 0 }, Step::Rewritten { minute: 0 }],
        Step::Evicted { freed: 10, usage: 100, minute: 200 },
        false,
    )]
    #[case::too_soon_after_a_rewrite(
        &[Step::Rewritten { minute: 0 }],
        Step::Evicted { freed: 50, usage: 100, minute: 119 },
        false,
    )]
    #[case::long_after_a_rewrite(
        &[Step::Rewritten { minute: 0 }],
        Step::Evicted { freed: 50, usage: 100, minute: 120 },
        true,
    )]
    #[case::too_soon_after_a_failed_rewrite(
        &[Step::Evicted { freed: 30, usage: 100, minute: 0 }, Step::Failed { minute: 0 }],
        Step::Evicted { freed: 0, usage: 100, minute: 119 },
        false,
    )]
    #[case::a_failed_rewrite_keeps_the_evictions(
        &[Step::Evicted { freed: 30, usage: 100, minute: 0 }, Step::Failed { minute: 0 }],
        Step::Evicted { freed: 0, usage: 100, minute: 120 },
        true,
    )]
    fn rewrite_follows_the_share_evicted_and_the_interval(
        #[case] before: &[Step],
        #[case] last: Step,
        #[case] due: bool,
    ) {
        let start = Instant::now();
        let at = |minute: u64| start + Duration::from_mins(minute);
        let mut rewrites = Rewrites::default();
        // The bytes that the store counts as evicted since its last rewrite.
        let mut evicted = 0;
        let mut apply = |step: Step| match step {
            Step::Evicted {
                freed,
                usage,
                minute,
            } => {
                evicted += freed;
                rewrites.begin_if_due(evicted, usage, at(minute)).is_ok()
            }
            Step::Rewritten { minute } => {
                rewrites.last = Some(at(minute));
                evicted = 0;
                false
            }
            Step::Failed { minute } => {
                rewrites.last = Some(at(minute));
                false
            }
        };
        for &step in before {
            let _ = apply(step);
        }

        assert_eq!(apply(last), due);
    }

    /// A rewrite after evictions that fails leaves the evicted bytes counted, and the next
    /// rewrite waits for the interval.
    #[rstest]
    #[tokio::test]
    async fn a_failed_rewrite_after_evictions_waits_for_the_interval(dir: TempDir) {
        let store = store_in(&dir).await;
        let _ = written(&store, [dialog("gedit", "kept", SystemTime::now(), window("kept"))]).await;
        store.add_evicted(store.estimated_disk_space());
        // A file where the rewrite builds its copy makes the rewrite fail.
        let blocker = dir.path().join("store").join("db-2.partial");
        std::fs::write(&blocker, b"").expect("block the rewrite");
        let retention = Retention::new(store, None);
        let evicted = || async { retention.store.evicted_since_rewrite().await.expect("read") };

        retention.rewrite_if_due().await;
        assert!(retention.rewrites.lock().last.is_some(), "the rewrite was due");
        assert!(evicted().await > 0);
        std::fs::remove_file(&blocker).expect("unblock the rewrite");
        retention.rewrite_if_due().await;
        assert!(evicted().await > 0, "the rewrite waits for the interval");
        retention.rewrites.lock().last = Some(Instant::now() - REWRITE_INTERVAL);
        retention.rewrite_if_due().await;

        assert_eq!(evicted().await, 0);
        retention.store.close().await.expect("close");
    }
}
