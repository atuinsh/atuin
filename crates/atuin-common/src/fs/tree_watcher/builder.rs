use std::num::{NonZeroU8, NonZeroU64};
use std::path::Path;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};

use super::poller::TreeWatcherPoller;
use super::stat::StatPoller;
use super::tracker::FileTracker;
use super::walk::WalkPoller;
use super::{TreeWatcher, TreeWatcherError};
use crate::sync::BlockingPool;
use crate::time::NonZeroDuration;

// The watcher's loop awaits one pool job at a time, so a second worker would sit idle.
const DEFAULT_MAX_WORKERS: NonZeroU8 = NonZeroU8::MIN;
const DEFAULT_SCAN_INTERVAL: NonZeroDuration =
    NonZeroDuration::from_secs(NonZeroU64::new(30).unwrap());
const DEFAULT_CONTENT_POLL_INTERVAL: NonZeroDuration =
    NonZeroDuration::from_secs(NonZeroU64::new(2).unwrap());
const DEFAULT_HOT_WINDOW: Duration = Duration::from_secs(15 * 60);
const DEFAULT_DEBOUNCE_TIMEOUT: NonZeroDuration =
    NonZeroDuration::new(Duration::from_millis(250)).unwrap();

/// Builder for a [`TreeWatcher`].
pub struct TreeWatcherBuilder<F = fn(&Path) -> bool> {
    max_workers: NonZeroU8,
    filter: F,
    recursive: bool,
    scan_interval: NonZeroDuration,
    content_poll_interval: NonZeroDuration,
    hot_window: Duration,
    debounce_timeout: NonZeroDuration,
}

impl TreeWatcherBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_workers: DEFAULT_MAX_WORKERS,
            filter: |_| true,
            recursive: true,
            scan_interval: DEFAULT_SCAN_INTERVAL,
            content_poll_interval: DEFAULT_CONTENT_POLL_INTERVAL,
            hot_window: DEFAULT_HOT_WINDOW,
            debounce_timeout: DEFAULT_DEBOUNCE_TIMEOUT,
        }
    }
}

impl Default for TreeWatcherBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl<F> TreeWatcherBuilder<F>
where
    F: Fn(&Path) -> bool + Send + 'static,
{
    /// Yield only the regular files whose path `filter` accepts (default: every file).
    ///
    /// `filter` runs on the watcher's task, once per file path that appears, so it must not block.
    #[must_use]
    pub fn filter<G>(self, filter: G) -> TreeWatcherBuilder<G>
    where
        G: Fn(&Path) -> bool + Send + 'static,
    {
        TreeWatcherBuilder {
            max_workers: self.max_workers,
            filter,
            recursive: self.recursive,
            scan_interval: self.scan_interval,
            content_poll_interval: self.content_poll_interval,
            hot_window: self.hot_window,
            debounce_timeout: self.debounce_timeout,
        }
    }

    /// Most filesystem operations the watcher runs at once (default: 1).
    #[must_use]
    pub fn max_workers(mut self, workers: NonZeroU8) -> Self {
        self.max_workers = workers;
        self
    }

    /// Descend into subdirectories (default: `true`).
    #[must_use]
    pub fn recursive(mut self, yes: bool) -> Self {
        self.recursive = yes;
        self
    }

    /// Interval between reconciling full scans (default: 30s).
    #[must_use]
    pub fn scan_interval(mut self, interval: NonZeroDuration) -> Self {
        self.scan_interval = interval;
        self
    }

    /// Interval between content polls, which re-stat tracked files written within the
    /// [`hot_window`](Self::hot_window) (default: 2s).
    #[must_use]
    pub fn content_poll_interval(mut self, interval: NonZeroDuration) -> Self {
        self.content_poll_interval = interval;
        self
    }

    /// How recently a file must have been written for the content poll to re-stat it (default: 15
    /// minutes).
    ///
    /// Older files are still covered by events and the full scan.
    #[must_use]
    pub fn hot_window(mut self, window: Duration) -> Self {
        self.hot_window = window;
        self
    }

    /// Window for coalescing filesystem events (default: 250ms).
    #[must_use]
    pub fn debounce_timeout(mut self, timeout: NonZeroDuration) -> Self {
        self.debounce_timeout = timeout;
        self
    }

    /// Start watching `root`, yielding each regular file the [`filter`](Self::filter) accepts.
    pub fn watch(self, root: impl AsRef<Path>) -> Result<TreeWatcher, TreeWatcherError> {
        let root = std::fs::canonicalize(root.as_ref())?;
        if !root.is_dir() {
            return Err(TreeWatcherError::NotADirectory(root));
        }

        let (tx, rx) = flume::unbounded();
        let mode = if self.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        let mut debouncer = new_debouncer(
            self.debounce_timeout.get(),
            None,
            move |result: DebounceEventResult| {
                let _ = tx.send(result);
            },
        )?;
        debouncer.watch(&root, mode)?;

        let (found, files) = flume::unbounded();
        let tracker = FileTracker::new(self.filter, found);
        let pool = BlockingPool::new(self.max_workers.into());
        let walk = WalkPoller::new(pool.clone(), root, self.recursive);
        let stat = StatPoller::new(pool, self.hot_window);
        let poller = TreeWatcherPoller::new(walk, stat, tracker);
        let task = tokio::spawn(poller.run(rx, self.scan_interval, self.content_poll_interval));

        Ok(TreeWatcher {
            files: files.into_stream(),
            task,
            _debouncer: debouncer,
        })
    }
}
