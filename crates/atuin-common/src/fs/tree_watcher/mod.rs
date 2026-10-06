//! Find the regular files under a directory, and follow each one's content until it goes away.
//!
//! A [`TreeWatcher`] walks a `root` directory and yields a [`WatchedFile`] for each regular file
//! whose path its filter accepts. Directories and symlinks are never yielded, and a rejected
//! file is never offered to the filter again while it stays in place.
//!
//! A [`WatchedFile`]'s [`stat`](WatchedFile::stat) is a [`tokio::sync::watch::Receiver`] holding
//! the file's latest [`FileStat`]: it changes whenever the file's content does (size, modification
//! time or identity), and closes once the file is gone.
//!
//! # Usage Guide
//!
//! The intended [`TreeWatcher`] interface is its [`Stream`] implementation. You create a new
//! [`TreeWatcher`] via [`TreeWatcherBuilder::watch`] which will start watching a directory. Since
//! [`TreeWatcher`] implements [`Stream`], you can listen to the stream and receive [`WatchedFile`]s.
//!
//! When a new file is discovered for the first time, or created, the stream will return a new
//! [`WatchedFile`] for you:
//!
//! ```
//! # use atuin_common::fs::tree_watcher::TreeWatcher;
//! # use futures::StreamExt;
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut files = TreeWatcher::builder().watch("/var/log")?;
//!
//! while let Some(watched_file) = files.next().await {
//!     eprintln!("Discovered a new file: {}", watched_file.path().display());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The stream will terminate if:
//!
//!   - The file is deleted.
//!   - The tree watcher is dropped.
//!
//! ## Filtering
//!
//! The [`TreeWatcher`] allows you to pass a custom filter, with [`TreeWatcherBuilder::filter`],
//! which specifies whether you want to track a file or not. When a file event is observed by the
//! [`TreeWatcher`], it can immediately forget about it and avoid notifying you.
//!
//! In the following example, we only listen for files which have the "log" file extension.
//!
//! ```
//! # use atuin_common::fs::tree_watcher::TreeWatcher;
//! # use futures::StreamExt;
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut files = TreeWatcher::builder()
//!     .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
//!     .watch("/var/log")?;
//!
//! while let Some(watched_file) = files.next().await {
//!     eprintln!("Discovered a new file: {}", watched_file.path().display());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! We **recommend** you use this rather than drop events on the stream consumer, as it is cheaper.
//!
//! ## Listening for file updates
//!
//! The stream only tells you a file exists. To hear about writes to it, wait on its
//! [`stat`](WatchedFile::stat), a [`tokio::sync::watch::Receiver`] of the file's latest
//! [`FileStat`]:
//!
//! - [`changed`](tokio::sync::watch::Receiver::changed) resolves to `Ok` once the file's size,
//!   modification time or (on unix) [`identity`](FileStat::identity) differs from the last value
//!   you marked seen with [`borrow_and_update`](tokio::sync::watch::Receiver::borrow_and_update).
//! - It resolves to `Err` once the file is removed or renamed away, or the [`TreeWatcher`] is
//!   dropped. Nothing more will arrive on that receiver.
//!
//! The stat a file is yielded with counts as seen, so `changed` waits for the first write after
//! discovery; read the file once when it arrives, then again after each wakeup.
//!
//! ```
//! # use atuin_common::fs::tree_watcher::TreeWatcher;
//! # use futures::StreamExt;
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut files = TreeWatcher::builder().watch("/var/log")?;
//!
//! while let Some(file) = files.next().await {
//!     let (path, mut stat) = file.into_parts();
//!     tokio::spawn(async move {
//!         while stat.changed().await.is_ok() {
//!             let latest = *stat.borrow_and_update();
//!             println!("{} is now {} bytes", path.display(), latest.size());
//!         }
//!         println!("{} is gone", path.display());
//!     });
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Example
//!
//! This example listens to file creation events, as well as file changes, without spawning
//! background tokio tasks:
//!
//! ```
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! use atuin_common::fs::tree_watcher::TreeWatcher;
//! use futures::StreamExt;
//! use tokio::io::AsyncWriteExt;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let dir = tempfile::tempdir()?;
//! let log = dir.path().join("app.log");
//! std::fs::write(&log, "started\n")?;
//!
//! // Yields each `.log` file once.
//! let files = TreeWatcher::builder()
//!     .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
//!     .watch(dir.path())?;
//!
//! // Stands in for the process writing the log.
//! let writer = tokio::spawn(async move {
//!     let mut file = tokio::fs::OpenOptions::new().append(true).open(&log).await.unwrap();
//!     loop {
//!         file.write_all(b"line\n").await.unwrap();
//!         tokio::time::sleep(Duration::from_millis(100)).await;
//!     }
//! });
//!
//! // Merge every file's changes into one stream.
//! let mut changes = files.flat_map_unordered(None, |file| {
//!     let (path, stat) = file.into_parts();
//!
//!     futures::stream::unfold(stat, |mut stat| async move {
//!         // Wakes once per burst of writes, and ends once the file is removed or renamed away.
//!         stat.changed().await.ok()?;
//!         let size = stat.borrow_and_update().size();
//!         Some((size, stat))
//!     })
//!     .map(move |size| (Arc::clone(&path), size))
//!     .boxed()
//! });
//!
//! // A real consumer loops on `changes.next()`; this one stops at the first append.
//! let (path, size) = changes.next().await.expect("the watcher runs until dropped");
//! println!("changed: {} ({size} bytes)", path.display());
//! assert!(size > "started\n".len() as u64);
//!
//! writer.abort();
//! # Ok(())
//! # }
//! ```
//!
//! # Implementation Details
//!
//! The [`TreeWatcher`] utility is backed by [`notify`] which is backed by either
//! [`inotify`](https://man7.org/linux/man-pages/man7/inotify.7.html),
//! [`FSEvents`](https://developer.apple.com/documentation/coreservices/file_system_events) or
//! [`ReadDirectoryChangesW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw).
//! This means that, for most uses, [`TreeWatcher`] will be relatively low latency.
//!
//! Unfortunately, support for these systems is flaky at best, and, consequently, we have a fallback
//! path ([`TreeWatcherBuilder::scan_interval`]) that performs a scan over the given directory,
//! reconciling anything observed between the scan and the real-time notification system.
//! Additionally, for any actively managed file, there is a
//! [`TreeWatcherBuilder::content_poll_interval`]-controlled background poll on the file contents.
//!
//! To summarize:
//!
//!   - The system uses `inotify` if possible, falling back to:
//!   - Periodic polling of actively watched files.
//!   - Periodic polling of the whole directory tree.

mod builder;
mod poller;
mod stat;
mod tracker;
mod walk;

use std::fs::{FileType, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

pub use builder::TreeWatcherBuilder;
use futures::{Stream, StreamExt};
use notify::RecommendedWatcher;
use notify_debouncer_full::{Debouncer, RecommendedCache};
use strum_macros::EnumDiscriminants;
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[cfg(unix)]
use crate::os::fs::FdIdentity;

/// Reason a [`TreeWatcher`] could not be started.
#[derive(Debug, thiserror::Error)]
pub enum TreeWatcherError {
    #[error("watch root is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error(transparent)]
    Notify(#[from] notify::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What a stat reveals about a regular file, compared to detect content changes without
/// reading it. Only [`PathFingerprint::File`] carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentMark {
    size: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: FdIdentity,
}

impl ContentMark {
    /// The mark in a regular file's metadata.
    fn from_metadata(meta: &Metadata) -> Self {
        Self {
            size: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            identity: FdIdentity::from_metadata(meta),
        }
    }

    /// Whether the file was written within `window` of `now`, on either side so a skewed clock
    /// does not keep it inside the window forever. An unknown modification time counts as inside:
    /// the content poll is a backstop, so err toward re-statting.
    fn written_within(self, window: Duration, now: SystemTime) -> bool {
        self.modified.is_none_or(|m| {
            now.duration_since(m).unwrap_or_else(|future| future.duration()) <= window
        })
    }
}

/// Fingerprint of whatever a path names on disk: its [`FileKind`], plus a file's content mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumDiscriminants)]
#[strum_discriminants(name(FileKind))]
enum PathFingerprint {
    /// A regular file, and what its stat says about its content.
    File(ContentMark),
    Dir,
    Symlink,
    Other,
}

impl PathFingerprint {
    /// The node a directory entry or stat reports as `file_type`, calling `stat` for the content
    /// mark only when it is a regular file.
    fn new(file_type: FileType, stat: impl FnOnce() -> io::Result<Metadata>) -> io::Result<Self> {
        Ok(if file_type.is_file() {
            Self::File(ContentMark::from_metadata(&stat()?))
        } else if file_type.is_dir() {
            Self::Dir
        } else if file_type.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        })
    }
}

/// The facts a watched file was last seen with.
///
/// They are informational: a consumer that re-reads the file needs none of them to be correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    mark: ContentMark,
}

impl FileStat {
    /// The file's size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.mark.size
    }

    /// The file's modification time, if the platform reports one.
    #[must_use]
    pub fn modified(&self) -> Option<SystemTime> {
        self.mark.modified
    }

    /// The file's identity; a different identity than last time means the path now names a
    /// different file.
    #[cfg(unix)]
    #[must_use]
    pub fn identity(&self) -> FdIdentity {
        self.mark.identity
    }
}

/// A regular file a [`TreeWatcher`] found, followed for as long as it exists.
#[derive(Debug, Clone)]
pub struct WatchedFile {
    path: Arc<Path>,
    stat: watch::Receiver<FileStat>,
}

impl WatchedFile {
    /// The file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's latest [`FileStat`]: marked changed on each content change, and closed once the
    /// file is gone or the watcher drops.
    #[must_use]
    pub fn stat(&self) -> &watch::Receiver<FileStat> {
        &self.stat
    }

    /// Split into the shared path handle and the stat receiver.
    #[must_use]
    pub fn into_parts(self) -> (Arc<Path>, watch::Receiver<FileStat>) {
        (self.path, self.stat)
    }
}

/// A stream of the regular files under a directory tree, each yielded once as it is found.
///
/// Dropping it stops watching and closes the stat of every [`WatchedFile`] it yielded.
#[must_use = "dropping the TreeWatcher stops watching"]
pub struct TreeWatcher {
    files: flume::r#async::RecvStream<'static, WatchedFile>,
    task: JoinHandle<()>,
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl TreeWatcher {
    /// Begin configuring a watcher.
    #[must_use]
    pub fn builder() -> TreeWatcherBuilder {
        TreeWatcherBuilder::new()
    }
}

impl Stream for TreeWatcher {
    type Item = WatchedFile;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<WatchedFile>> {
        self.files.poll_next_unpin(cx)
    }
}

impl Drop for TreeWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use rstest::rstest;

    use super::*;
    use crate::time::NonZeroDuration;

    fn nz(d: Duration) -> NonZeroDuration {
        NonZeroDuration::new(d).unwrap()
    }

    #[rstest]
    #[case::fresh(Some(0), true)]
    #[case::at_window(Some(60), true)]
    #[case::past_window(Some(61), false)]
    #[case::future(Some(-5), true)]
    #[case::far_future(Some(-61), false)]
    #[case::unknown(None, true)]
    fn content_mark_written_within(#[case] age_secs: Option<i64>, #[case] within: bool) {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let modified = age_secs.map(|age| {
            if age < 0 {
                now + Duration::from_secs(age.unsigned_abs())
            } else {
                now - Duration::from_secs(age.unsigned_abs())
            }
        });
        let mark = ContentMark {
            size: 1,
            modified,
            #[cfg(unix)]
            identity: FdIdentity::from_raw(0, 0),
        };
        assert_eq!(mark.written_within(Duration::from_secs(60), now), within);
    }

    /// A watcher over `root` that yields every file, with the scan and debounce tightened to
    /// `scan_ms` and `debounce_ms`.
    fn watcher(root: &Path, scan_ms: u64, debounce_ms: u64) -> TreeWatcher {
        TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(scan_ms)))
            .debounce_timeout(nz(Duration::from_millis(debounce_ms)))
            .watch(root)
            .unwrap()
    }

    /// The next file the watcher yields named `name`, skipping any others.
    async fn found(watcher: &mut TreeWatcher, name: &str) -> WatchedFile {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let file = watcher.next().await.expect("the watcher never ends");
                if file.path().ends_with(name) {
                    break file;
                }
            }
        })
        .await
        .expect("file is yielded")
    }

    async fn closes(mut stat: watch::Receiver<FileStat>) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while stat.changed().await.is_ok() {}
        })
        .await
        .expect("stat closes");
    }

    /// Wait until the stat reports `size`. Bursts may deliver several changes and intermediate
    /// sizes, so only the final state is asserted on.
    async fn changed_to(stat: &mut watch::Receiver<FileStat>, size: u64) -> FileStat {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                stat.changed().await.expect("file still exists");
                let seen = *stat.borrow_and_update();
                if seen.size() == size {
                    break seen;
                }
            }
        })
        .await
        .expect("change is delivered")
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn created_file_is_yielded() {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = watcher(dir.path(), 100, 50);
        std::fs::write(dir.path().join("a.log"), b"x").unwrap();
        found(&mut watcher, "a.log").await;
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn filter_limits_the_yielded_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("a.log"), b"x").unwrap();
        let mut watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
            .watch(dir.path())
            .unwrap();

        let first = tokio::time::timeout(Duration::from_secs(5), watcher.next())
            .await
            .expect("file is yielded")
            .unwrap();
        assert!(first.path().ends_with("a.log"), "yielded {}", first.path().display());
        // Several scan passes: an unapplied filter would have yielded `b.txt` by now.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(watcher.next().now_or_never().is_none(), "a rejected file was yielded");
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deleted_file_closes_its_stat() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b.log");
        std::fs::write(&file, b"x").unwrap();
        let mut watcher = watcher(dir.path(), 100, 50);

        let stat = found(&mut watcher, "b.log").await.stat;
        std::fs::remove_file(&file).unwrap();
        closes(stat).await;
    }

    #[rstest]
    #[case(true, true, 5000)]
    #[case(false, false, 1000)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recursion_controls_nested_files(
        #[case] recursive: bool,
        #[case] expect_fire: bool,
        #[case] wait_ms: u64,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = TreeWatcher::builder()
            .recursive(recursive)
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path())
            .unwrap();

        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/c.log"), b"x").unwrap();

        let fired =
            tokio::time::timeout(Duration::from_millis(wait_ms), watcher.next()).await.is_ok();
        assert_eq!(fired, expect_fire);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_watcher_closes_every_stat() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.log"), b"x").unwrap();
        let mut watcher = watcher(dir.path(), 100, 50);

        let stat = found(&mut watcher, "a.log").await.stat;
        drop(watcher);
        closes(stat).await;
    }

    // Both bad-root checks run before any Tokio task is spawned, so this needs no runtime.
    #[rstest]
    #[case::missing_root(false)]
    #[case::file_root(true)]
    fn watch_rejects_bad_root(#[case] root_is_file: bool) {
        let dir = tempfile::tempdir().unwrap();
        let root = if root_is_file {
            let file = dir.path().join("not-a-dir");
            std::fs::write(&file, b"x").unwrap();
            file
        } else {
            dir.path().join("does-not-exist")
        };
        let err = TreeWatcher::builder().watch(root).err().expect("bad root must be rejected");
        if root_is_file {
            assert!(matches!(err, TreeWatcherError::NotADirectory(_)), "got {err:?}");
        } else {
            assert!(matches!(err, TreeWatcherError::Io(_)), "got {err:?}");
        }
    }

    #[derive(Clone, Copy)]
    enum Mutation {
        Append,
        Overwrite,
    }

    impl Mutation {
        fn apply(self, path: &Path, i: usize) {
            match self {
                Self::Append => {
                    use std::io::Write as _;
                    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
                    write!(f, "-{i}").unwrap();
                }
                Self::Overwrite => std::fs::write(path, format!("body-{i}")).unwrap(),
            }
        }
    }

    // A content write is neither a create/remove nor a kind change, so the file must not be
    // yielded again (not by the event path, not by the periodic scan) and its stat must stay
    // open, updated in place.
    #[rstest]
    #[case::append(Mutation::Append)]
    #[case::overwrite(Mutation::Overwrite)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn benign_mutation_does_not_re_yield_the_file(#[case] mutation: Mutation) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keep.log");
        std::fs::write(&file, b"seed").unwrap();

        let mut watcher = watcher(dir.path(), 50, 20);
        let mut stat = found(&mut watcher, "keep.log").await.stat;

        for i in 0..3 {
            mutation.apply(&file, i);
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        // Several scan cycles and debounce windows: a bug that re-yields on modify surfaces here.
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(watcher.next().now_or_never().is_none(), "file was re-yielded");
        assert!(stat.has_changed().is_ok(), "stat was closed");
        let final_size = std::fs::metadata(&file).unwrap().len();
        changed_to(&mut stat, final_size).await;
    }

    // Scan and poll intervals of 30s leave only the event fast path able to deliver in time.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_signals_change_via_events() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grow.log");
        std::fs::write(&file, b"seed").unwrap();

        let mut watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_secs(30)))
            .content_poll_interval(nz(Duration::from_secs(30)))
            .debounce_timeout(nz(Duration::from_millis(20)))
            .watch(dir.path())
            .unwrap();
        let mut stat = found(&mut watcher, "grow.log").await.stat;

        Mutation::Append.apply(&file, 7);
        changed_to(&mut stat, 6).await;
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn truncate_signals_smaller_size() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shrink.log");
        std::fs::write(&file, b"long seed").unwrap();

        let mut watcher = watcher(dir.path(), 100, 20);
        let mut stat = found(&mut watcher, "shrink.log").await.stat;

        std::fs::write(&file, b"s").unwrap();
        changed_to(&mut stat, 1).await;
    }

    // A 30s debounce means no filesystem event can be delivered inside the 5s window, and
    // whichever of the scan and the content poll is left fast is provably what delivered.
    #[rstest]
    #[case::content_poll(30_000, 50)]
    #[case::scan(100, 30_000)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_signals_change_when_events_are_delayed(
        #[case] scan_ms: u64,
        #[case] poll_ms: u64,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("late.log");
        std::fs::write(&file, b"seed").unwrap();

        let mut watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(scan_ms)))
            .content_poll_interval(nz(Duration::from_millis(poll_ms)))
            .debounce_timeout(nz(Duration::from_secs(30)))
            .watch(dir.path())
            .unwrap();
        let mut stat = found(&mut watcher, "late.log").await.stat;

        Mutation::Append.apply(&file, 7);
        changed_to(&mut stat, 6).await;
    }

    // A 30s debounce means no filesystem event can be delivered inside the 5s window, so
    // whatever reconciles the change is provably the periodic scan alone.
    #[rstest]
    #[case::appearance(false)]
    #[case::disappearance(true)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn periodic_scan_reconciles_when_events_are_delayed(#[case] pre_exists: bool) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.log");
        if pre_exists {
            std::fs::write(&file, b"x").unwrap();
        }
        let mut watcher = watcher(dir.path(), 100, 30_000);

        if pre_exists {
            let stat = found(&mut watcher, "s.log").await.stat;
            std::fs::remove_file(&file).unwrap();
            closes(stat).await;
        } else {
            std::fs::write(&file, b"x").unwrap();
            found(&mut watcher, "s.log").await;
        }
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rename_moves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.log");
        let to = dir.path().join("to.log");
        std::fs::write(&from, b"x").unwrap();
        let mut watcher = watcher(dir.path(), 100, 50);

        let stat = found(&mut watcher, "from.log").await.stat;
        std::fs::rename(&from, &to).unwrap();
        closes(stat).await;
        // The destination may surface as a rename event or via the reconciling scan.
        found(&mut watcher, "to.log").await;
    }

    // Directories and symlinks are tracked but never yielded -- symlinks especially must not be
    // followed to their target's kind. The file is created last, so it must be the first yield.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_regular_files_are_yielded() {
        let dir = tempfile::tempdir().unwrap();
        let mut watcher = watcher(dir.path(), 100, 50);

        std::fs::create_dir(dir.path().join("dir")).unwrap();
        // Dangling on purpose: the kind is read without following the link.
        std::os::unix::fs::symlink("missing-target", dir.path().join("link")).unwrap();
        std::fs::write(dir.path().join("file"), b"x").unwrap();

        let first = tokio::time::timeout(Duration::from_secs(5), watcher.next())
            .await
            .expect("file is yielded")
            .unwrap();
        assert!(first.path().ends_with("file"), "yielded {}", first.path().display());
    }
}
