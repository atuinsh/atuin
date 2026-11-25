//! The background task that indexes the contents that the store holds and the index lacks.

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::warn;

use super::index::{DesktopIndex, IndexError};
use super::store::{DesktopStore, Pass, StoreError};

const INTERVAL: Duration = Duration::from_mins(15);

/// The number of records that one batch of a pass reads.
const BATCH: usize = 256;

#[derive(Debug, thiserror::Error)]
enum ReconcileError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// Indexes the missing contents of `store` at once and then every 15 minutes, until the returned
/// task is aborted.
pub fn spawn_reconciler(store: DesktopStore, index: DesktopIndex) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            if let Err(err) = reconcile(&store, &index).await {
                warn!(?err, "failed to reconcile the desktop capture search index");
            }
        }
    })
}

/// Indexes the contents that the index lacks among the `Observed` records after the index's
/// watermark. The function reads one batch at a time and moves the watermark past each batch.
async fn reconcile(store: &DesktopStore, index: &DesktopIndex) -> Result<(), ReconcileError> {
    let mut pass = Pass::after(index.reconciled_through().await?);
    loop {
        let _following = index.follow_store().await;
        let batch = store.reconcile_batch(pass, BATCH).await?;
        let Some(through) = batch.through else {
            return Ok(());
        };
        let missing = index.missing(&batch.candidates).await?;
        let items = if missing.is_empty() {
            pass = batch.pass;
            Vec::new()
        } else {
            let expansion = store.expand_candidates(batch.pass, missing).await?;
            pass = expansion.pass;
            expansion.items
        };
        index.add_reconciled(items, through).await?;
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::desktop_capture::store::RunState;
    use crate::desktop_capture::testing::records::{
        background, ended, snapshot, started, window, written,
    };
    use crate::desktop_capture::testing::{Opened, dir, index_in, indexed_keys, opened, store_in};

    /// The reconciler indexes what the live path missed, and a second pass, also after the
    /// store and the index open again, doesn't change anything.
    #[rstest]
    #[tokio::test]
    async fn reconciler_fills_gaps(dir: TempDir) {
        let store = store_in(&dir).await;
        let index = index_in(&dir).await;
        let now = SystemTime::now();
        let items = written(&store, [
            started(1, "kate", "notes", now),
            snapshot(1, now, window("zebra")),
            snapshot(1, now, window("yak")),
            snapshot(1, now, window("xerus")),
        ])
        .await;
        assert_eq!(items.len(), 3);
        index.add([items[0].clone(), items[2].clone()]).await.expect("index the first and last");
        assert!(indexed_keys(&index, "yak").await.is_empty());

        reconcile(&store, &index).await.expect("reconcile");

        for (item, word) in items.iter().zip(["zebra", "yak", "xerus"]) {
            assert_eq!(indexed_keys(&index, word).await, [item.key]);
        }
        let watermark = index.reconciled_through().await.expect("watermark");
        assert!(watermark.is_some());

        reconcile(&store, &index).await.expect("a second pass changes nothing");
        store.close().await.expect("close");
        let store = store_in(&dir).await;
        let index = index_in(&dir).await;
        reconcile(&store, &index).await.expect("a pass after reopening changes nothing");
        assert_eq!(indexed_keys(&index, "yak").await, [items[1].key]);
        assert_eq!(index.reconciled_through().await.expect("watermark"), watermark);
        store.close().await.expect("close");
    }

    #[rstest]
    #[tokio::test]
    async fn reconciler_fills_gaps_across_batches(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let count = BATCH * 2 + 5;
        let now = SystemTime::now();
        let records = std::iter::once(started(1, "kate", "notes", now))
            .chain((0..count).map(|n| snapshot(1, now, window(&format!("word{n}")))));
        let items = written(&store, records).await;
        assert_eq!(items.len(), count);

        reconcile(&store, &index).await.expect("reconcile");

        for n in [0, BATCH, count - 1] {
            assert_eq!(indexed_keys(&index, &format!("word{n}")).await.len(), 1);
        }
    }

    /// A background segment lasts up to an hour, so its later contents come long after its
    /// start and after many other records.
    #[rstest]
    #[tokio::test]
    async fn a_later_content_of_an_old_background_segment_is_found(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let mut run = RunState::default();
        let mut write = async |record| store.write(&mut run, record).await.expect("write").item;
        let read = |line| background(7, "mail", "inbox", SystemTime::now(), window(line));
        let _ = write(read("lemur")).await;
        reconcile(&store, &index).await.expect("reconcile");
        for segment in 1..=3 {
            let _ = write(started(segment, "kate", "notes", SystemTime::now())).await;
            let other = window(&format!("other{segment}"));
            let _ = write(snapshot(segment, SystemTime::now(), other)).await;
        }
        reconcile(&store, &index).await.expect("reconcile");

        let later = write(read("okapi")).await.expect("a new content");
        reconcile(&store, &index).await.expect("reconcile");

        assert_eq!(indexed_keys(&index, "okapi").await, [later.key]);
    }

    /// A pass reads only the records after its watermark, and decodes each delta chain once.
    #[rstest]
    #[tokio::test]
    async fn a_pass_expands_only_what_is_new(#[future(awt)] opened: Opened) {
        const HISTORY: usize = 100;
        const NEW: usize = 5;
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let mut run = RunState::default();
        let mut write = async |record| {
            let _ = store.write(&mut run, record).await.expect("write");
        };
        let page = |n: usize| snapshot(1, SystemTime::now(), window(&format!("word{n}")));
        write(started(1, "kate", "notes", SystemTime::now())).await;
        for n in 0..HISTORY {
            write(page(n)).await;
        }
        let before = store.expansions();
        reconcile(&store, &index).await.expect("reconcile");
        assert_eq!(store.expansions() - before, HISTORY, "each content is expanded once");

        for n in HISTORY..HISTORY + NEW {
            write(page(n)).await;
        }
        let before = store.expansions();
        reconcile(&store, &index).await.expect("reconcile");
        let expanded = store.expansions() - before;

        // The first new content's chain reaches back at most to the last whole tree.
        assert!(expanded < NEW + 32, "{expanded} expansions for {NEW} new contents");
        assert_eq!(indexed_keys(&index, &format!("word{}", HISTORY + NEW - 1)).await.len(), 1);
    }

    /// A pass doesn't expand the contents that the live path has already indexed.
    #[rstest]
    #[tokio::test]
    async fn a_pass_over_indexed_contents_expands_nothing(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let now = SystemTime::now();
        let records = std::iter::once(started(1, "kate", "notes", now))
            .chain((0..40).map(|n| snapshot(1, now, window(&format!("word{n}")))));
        let items = written(&store, records).await;
        index.add(items).await.expect("index");
        let before = store.expansions();

        reconcile(&store, &index).await.expect("reconcile");

        assert_eq!(store.expansions(), before);
        assert!(index.reconciled_through().await.expect("watermark").is_some());
    }

    /// A damaged record or content is skipped, and the pass indexes what comes after it.
    #[rstest]
    #[tokio::test]
    async fn a_pass_carries_on_past_damage(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let now = SystemTime::now();
        let damaged =
            written(&store, [started(1, "kate", "notes", now), snapshot(1, now, window("lost"))])
                .await;
        store.put_raw_content(damaged[0].key, b"not a content");
        store.put_raw_record(uuid::Uuid::now_v7(), b"not a record");
        let kept =
            written(&store, [started(2, "kate", "notes", now), snapshot(2, now, window("kept"))])
                .await;

        reconcile(&store, &index).await.expect("reconcile");

        assert!(indexed_keys(&index, "lost").await.is_empty());
        assert_eq!(indexed_keys(&index, "kept").await, [kept[0].key]);
    }

    #[rstest]
    #[tokio::test]
    async fn search_finds_background_and_closed_segments(#[future(awt)] opened: Opened) {
        let Opened {
            dir: _dir,
            store,
            index,
        } = opened;
        let now = SystemTime::now();
        let items = written(&store, [
            started(1, "kate", "closing", now),
            snapshot(1, now, window("ibis")),
            ended(1, now),
            background(7, "mail", "inbox", now, window("lemur")),
        ])
        .await;
        assert_eq!(items.len(), 2);
        index.add(items.clone()).await.expect("add");

        for (item, word) in items.iter().zip(["ibis", "lemur"]) {
            assert_eq!(indexed_keys(&index, word).await, [item.key]);
        }
        assert_eq!(indexed_keys(&index, "inbox").await.len(), 1);
    }
}
