use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::PathFingerprint;
use super::tracker::FileTracker;
use crate::sync::BlockingPool;

/// Stats files in the pool, both on the content poll and for the paths filesystem events name.
pub struct StatPoller {
    pool: BlockingPool,
    hot_window: Duration,
}

impl StatPoller {
    pub fn new(pool: BlockingPool, hot_window: Duration) -> Self {
        Self { pool, hot_window }
    }

    /// Re-stat only the recently written files: a cheap backstop for content events that the
    /// backend coalesced or dropped, between full scans.
    pub async fn poll<F>(&self, tracker: &mut FileTracker<F>)
    where
        F: Fn(&Path) -> bool,
    {
        let hot: Vec<Arc<Path>> =
            tracker.written_within(self.hot_window, SystemTime::now()).collect();
        if hot.is_empty() {
            return;
        }
        for (path, fingerprint) in self.stat(hot).await {
            tracker.observe_fingerprint(path, fingerprint);
        }
    }

    /// Stat `paths` in the pool, keeping only the ones that still exist.
    pub async fn stat(
        &self,
        paths: impl IntoIterator<Item = Arc<Path>, IntoIter: Send + 'static>,
    ) -> HashMap<Arc<Path>, PathFingerprint> {
        let paths = paths.into_iter();
        self.pool
            .run(move || {
                paths
                    .filter_map(|path| {
                        let meta = std::fs::symlink_metadata(&path).ok()?;
                        let fingerprint =
                            PathFingerprint::new(meta.file_type(), || Ok(meta)).ok()?;
                        Some((path, fingerprint))
                    })
                    .collect()
            })
            .await
            .unwrap_or_default()
    }
}
