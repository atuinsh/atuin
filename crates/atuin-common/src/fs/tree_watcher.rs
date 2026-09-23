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
//! When a new file is discoevered for the first time, or created, the stream will return a new
//! [`WatchedFile`] for you:
//!
//! ```
//! # use std::num::NonZeroUsize;
//! # use atuin_common::fs::tree_watcher::TreeWatcher;
//! # use atuin_common::sync::BlockingPool;
//! # use futures::StreamExt;
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = BlockingPool::new(NonZeroUsize::new(4).unwrap());
//! let mut files = TreeWatcher::builder(pool)
//!     .watch("/var/log", |path| path.extension().is_some_and(|ext| ext == "log"))?;
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
//! The [`TreeWatcher`] allows you to pass a custom filter which specifies whether you want to track
//! a file or not. When a file event is observed by the [`TreeWatcher`], it can immediately forget
//! about it and avoid notifying you via the [`TreeWatcherBuilder::watch`]'s second parameter.
//!
//! In the following example, we only listen for files which have the "log" file extension.
//!
//! ```
//! # use std::num::NonZeroUsize;
//! # use atuin_common::fs::tree_watcher::TreeWatcher;
//! # use atuin_common::sync::BlockingPool;
//! # use futures::StreamExt;
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = BlockingPool::new(NonZeroUsize::new(4).unwrap());
//! let mut files = TreeWatcher::builder(pool)
//!     .watch("/var/log", |path| path.extension().is_some_and(|ext| ext == "log"))?;
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
//! # Example
//!
//! ```no_run
//! use std::num::NonZeroUsize;
//! use std::sync::Arc;
//!
//! use atuin_common::fs::tree_watcher::TreeWatcher;
//! use atuin_common::sync::BlockingPool;
//! use futures::StreamExt;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = BlockingPool::new(NonZeroUsize::new(4).unwrap());
//! // Yields each `.log` file once, as it appears -- via a filesystem event or the periodic scan.
//! let files = TreeWatcher::builder(pool)
//!     .watch("/var/log", |path| path.extension().is_some_and(|ext| ext == "log"))?;
//!
//! // Merge every file's changes into one stream rather than spawning a task per file: a quiet
//! // file then costs nothing until it is written, however many the tree holds.
//! let mut changes = files.flat_map_unordered(None, |file| {
//!     let (path, stat) = file.into_parts();
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
//! while let Some((path, size)) = changes.next().await {
//!     println!("changed: {} ({size} bytes)", path.display());
//! }
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

use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use futures::{Stream, StreamExt};
use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::os::fs::FdIdentity;
use crate::sync::BlockingPool;
use crate::time::NonZeroDuration;

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

/// The kind of filesystem node at a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl From<std::fs::FileType> for FileKind {
    fn from(value: std::fs::FileType) -> Self {
        if value.is_file() {
            Self::File
        } else if value.is_dir() {
            Self::Dir
        } else if value.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }
}

/// How a change to a file came to the watcher's attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Surfaced by a filesystem event.
    Event,
    /// Surfaced by the periodic reconciling scan or the content poll.
    Scan,
}

/// What a stat reveals about a regular file, compared to detect content changes without
/// reading it. Only [`FileKind::File`] nodes carry one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentMark {
    size: u64,
    modified: Option<SystemTime>,
    identity: Option<FdIdentity>,
}

impl ContentMark {
    /// The mark for a regular file; `None` for any other kind of node.
    fn from_metadata(meta: &Metadata) -> Option<Self> {
        if !meta.file_type().is_file() {
            return None;
        }
        #[cfg(unix)]
        let identity = Some(FdIdentity::from_metadata(meta));
        #[cfg(not(unix))]
        let identity = None;
        Some(Self {
            size: meta.len(),
            modified: meta.modified().ok(),
            identity,
        })
    }

    /// Whether the file was written within `window` of `now`, on either side so a skewed clock
    /// does not pin it hot forever. An unknown modification time counts as hot: the poll is a
    /// backstop, so err toward re-statting.
    fn is_hot(self, now: SystemTime, window: Duration) -> bool {
        self.modified.is_none_or(|m| {
            now.duration_since(m).unwrap_or_else(|future| future.duration()) <= window
        })
    }
}

/// A node's kind plus, for regular files, its content mark.
type Observed = (FileKind, Option<ContentMark>);

/// The facts a watched file was last seen with.
///
/// They are informational: a consumer that re-reads the file needs none of them to be correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    mark: ContentMark,
    origin: Origin,
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

    /// The file's identity (unix only); a different identity than last time means the path now
    /// names a different file.
    #[must_use]
    pub fn identity(&self) -> Option<FdIdentity> {
        self.mark.identity
    }

    /// Whether an event, or the scan or content poll, surfaced these facts.
    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
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

/// A best-effort tree scan: the nodes found, plus the directories fully read.
type ScanResult = (Vec<(Arc<Path>, Observed)>, HashSet<Arc<Path>>);

/// Walk `root` best-effort, returning every readable entry plus the set of
/// directories that were fully read without error. A node may be pruned only when its
/// parent directory is in that set: a node missing from a directory we could not
/// fully read may be unreadable rather than gone, so unread subtrees are left intact
/// while readable ones reconcile independently.
fn scan_fs(root: &Path, recursive: bool) -> ScanResult {
    let mut out = Vec::new();
    let mut scanned: HashSet<Arc<Path>> = HashSet::new();
    let mut stack: Vec<Arc<Path>> = vec![Arc::from(root)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut dir_complete = true;
        for entry in entries {
            let Ok(entry) = entry else {
                dir_complete = false;
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                dir_complete = false;
                continue;
            };
            // Kinds come from the directory read; only regular files pay for a stat.
            let mark = if file_type.is_file() {
                let Ok(meta) = entry.metadata() else {
                    dir_complete = false;
                    continue;
                };
                ContentMark::from_metadata(&meta)
            } else {
                None
            };
            let path: Arc<Path> = Arc::from(entry.path());
            if recursive && file_type.is_dir() {
                stack.push(Arc::clone(&path));
            }
            out.push((path, (FileKind::from(file_type), mark)));
        }
        // Only a fully enumerated directory lets us trust the absence of its children.
        if dir_complete {
            scanned.insert(dir);
        }
    }
    (out, scanned)
}

/// [`scan_fs`] in `pool`. A cancelled scan read nothing, so it prunes nothing.
async fn scan(pool: &BlockingPool, root: &Path, recursive: bool) -> ScanResult {
    let root = root.to_path_buf();
    pool.run(move || scan_fs(&root, recursive)).await.unwrap_or_default()
}

/// Stat `paths` in `pool`, keeping only the ones that still exist.
async fn stat_paths(pool: &BlockingPool, paths: Vec<Arc<Path>>) -> HashMap<Arc<Path>, Observed> {
    let stat = pool.run(move || {
        paths
            .into_iter()
            .filter_map(|path| {
                let meta = std::fs::symlink_metadata(&path).ok()?;
                let observed =
                    (FileKind::from(meta.file_type()), ContentMark::from_metadata(&meta));
                Some((path, observed))
            })
            .collect()
    });
    stat.await.unwrap_or_default()
}

/// Stat every path referenced by `events`, keeping only the ones that still exist.
async fn resolve_kinds(
    pool: &BlockingPool,
    events: &[notify::Event],
) -> HashMap<Arc<Path>, Observed> {
    let paths = events
        .iter()
        .flat_map(|event| event.paths.iter().map(|p| Arc::from(p.as_path())))
        .collect();
    stat_paths(pool, paths).await
}

enum Slot {
    /// A regular file the filter accepted, and the sender its [`WatchedFile`] listens on.
    Tracked(ContentMark, watch::Sender<FileStat>),
    /// Any other node: a directory, a symlink, or a file the filter rejected.
    Untracked(FileKind),
}

impl Slot {
    fn kind(&self) -> FileKind {
        match self {
            Self::Tracked(..) => FileKind::File,
            Self::Untracked(kind) => *kind,
        }
    }

    fn mark(&self) -> Option<ContentMark> {
        match self {
            Self::Tracked(mark, _) => Some(*mark),
            Self::Untracked(_) => None,
        }
    }
}

struct Engine<F> {
    filter: F,
    root: Arc<Path>,
    entries: HashMap<Arc<Path>, Slot>,
    found: flume::Sender<WatchedFile>,
}

impl<F> Engine<F>
where
    F: Fn(&Path) -> bool,
{
    /// Bring the slot for `path` in line with what was just observed on disk: offer a new or
    /// kind-changed regular file to the filter, and publish a tracked file's moved content mark.
    fn observe(
        &mut self,
        path: Arc<Path>,
        kind: FileKind,
        mark: Option<ContentMark>,
        origin: Origin,
    ) {
        if let Some(slot) = self.entries.get_mut(&path)
            && slot.kind() == kind
        {
            // The stat is updated in place: yielding a new file would reset whatever read
            // position its consumer keeps.
            if let Slot::Tracked(known, stat) = slot
                && let Some(mark) = mark
                && *known != mark
            {
                *known = mark;
                stat.send_replace(FileStat { mark, origin });
            }
            return;
        }

        // A directory that became something else takes its tracked descendants with it.
        if self.entries.get(&path).is_some_and(|slot| slot.kind() == FileKind::Dir) {
            self.forget_tree(&path);
        } else {
            self.entries.remove(&path);
        }
        let slot = match (kind, mark) {
            (FileKind::File, Some(mark)) if (self.filter)(&path) => {
                let (stat, rx) = watch::channel(FileStat { mark, origin });
                // A dropped receiver means the watcher itself is going away.
                let _ = self.found.send(WatchedFile {
                    path: Arc::clone(&path),
                    stat: rx,
                });
                Slot::Tracked(mark, stat)
            }
            _ => Slot::Untracked(kind),
        };
        self.entries.insert(path, slot);
    }

    fn forget_tree(&mut self, path: &Path) {
        // Only a directory, or an untracked path that may have been one, can have descendants
        // worth the sweep over every entry.
        match self.entries.get(path) {
            Some(slot) if slot.kind() != FileKind::Dir => {
                self.entries.remove(path);
            }
            _ => self.entries.retain(|key, _| !key.starts_with(path)),
        }
    }

    /// Tracked regular files written within `window` of `now`: the only ones worth
    /// re-statting between full scans.
    fn hot_files(&self, now: SystemTime, window: Duration) -> impl Iterator<Item = Arc<Path>> + '_ {
        self.entries.iter().filter_map(move |(path, slot)| match slot {
            Slot::Tracked(mark, _) if mark.is_hot(now, window) => Some(Arc::clone(path)),
            Slot::Tracked(..) | Slot::Untracked(_) => None,
        })
    }

    fn reconcile(&mut self, truth: Vec<(Arc<Path>, Observed)>, scanned_dirs: &HashSet<Arc<Path>>) {
        let truth: HashMap<Arc<Path>, Observed> = truth.into_iter().collect();
        // Prune a tracked node only once it is confirmed gone: some ancestor whose
        // parent directory was fully read this pass is itself absent from the scan.
        // This drops removed nodes and everything under a removed directory, while
        // keeping nodes shielded by a directory that merely could not be read.
        self.entries.retain(|key, _| {
            truth.contains_key(key)
                || !key.ancestors().any(|ancestor| {
                    ancestor.parent().is_some_and(|parent| scanned_dirs.contains(parent))
                        && !truth.contains_key(ancestor)
                })
        });
        for (path, (kind, mark)) in truth {
            self.observe(path, kind, mark, Origin::Scan);
        }
    }

    fn observe_path(&mut self, path: &Path, kinds: &HashMap<Arc<Path>, Observed>, origin: Origin) {
        let Some((key, &(kind, mark))) = kinds.get_key_value(path) else {
            return;
        };
        if self.entries.get(path).is_some_and(|slot| slot.kind() == kind && slot.mark() == mark) {
            return;
        }
        self.observe(Arc::clone(key), kind, mark, origin);
    }

    fn apply_event(&mut self, event: &notify::Event, kinds: &HashMap<Arc<Path>, Observed>) {
        match &event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                for path in &event.paths {
                    self.observe_path(path, kinds, Origin::Event);
                }
            }
            // `observe_path` tells a new node from a changed one. A content event for a path that
            // no longer stats is the only trace of a young file's removal on macOS: FSEvents
            // reports it with sticky `Created`+`Removed` flags, which the debouncer cancels out.
            EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Any)
            | EventKind::Access(AccessKind::Close(AccessMode::Write)) => {
                for path in &event.paths {
                    // The root is watched, not tracked.
                    if path.as_path() == &*self.root {
                        continue;
                    }
                    if kinds.contains_key(path.as_path()) {
                        self.observe_path(path, kinds, Origin::Event);
                    } else {
                        self.forget_tree(path);
                    }
                }
            }
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                for path in &event.paths {
                    self.forget_tree(path);
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let [from, to] = event.paths.as_slice() {
                    self.forget_tree(from);
                    self.observe_path(to, kinds, Origin::Event);
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Any | RenameMode::Other)) => {
                // TODO: on a case-insensitive filesystem (e.g. macOS APFS), a
                // case-only rename (`a.log` -> `A.log`) leaves duplicate entries:
                // `symlink_metadata` on the stale-case name still succeeds, so we
                // keep it while also observing the new name, and the two byte-distinct
                // keys coexist until the next complete scan prunes the stale one. Fix
                // later by reconciling the renamed parent dir, or case-folding keys on
                // case-insensitive platforms.
                for path in &event.paths {
                    if kinds.contains_key(path.as_path()) {
                        self.observe_path(path, kinds, Origin::Event);
                    } else {
                        self.forget_tree(path);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Drives an [`Engine`] on a background task: reconciles it against a periodic
/// full scan, re-stats recently written files on a shorter poll, and applies
/// debounced filesystem events as they arrive.
struct TreeWatcherPoller<F> {
    root: PathBuf,
    recursive: bool,
    hot_window: Duration,
    pool: BlockingPool,
    engine: Engine<F>,
}

impl<F> TreeWatcherPoller<F>
where
    F: Fn(&Path) -> bool + Send + 'static,
{
    async fn run(
        mut self,
        events: flume::Receiver<DebounceEventResult>,
        scan_interval: NonZeroDuration,
        content_poll_interval: NonZeroDuration,
    ) {
        self.rescan().await;

        let mut scan_tick = tokio::time::interval(scan_interval.get());
        scan_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        scan_tick.tick().await;
        let mut poll_tick = tokio::time::interval(content_poll_interval.get());
        poll_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        poll_tick.tick().await;

        loop {
            tokio::select! {
                _ = scan_tick.tick() => self.rescan().await,
                _ = poll_tick.tick() => self.poll_hot().await,
                received = events.recv_async() => {
                    match received {
                        Ok(Ok(batch)) => {
                            let mut force_scan = false;
                            let mut pending = Vec::new();
                            for debounced in batch {
                                if debounced.need_rescan() {
                                    force_scan = true;
                                } else {
                                    pending.push(debounced.event);
                                }
                            }
                            if !pending.is_empty() {
                                let kinds = resolve_kinds(&self.pool, &pending).await;
                                for event in &pending {
                                    self.engine.apply_event(event, &kinds);
                                }
                            }
                            if force_scan {
                                self.rescan().await;
                            }
                        }
                        Ok(Err(errors)) => tracing::warn!(?errors, "tree watcher backend error"),
                        Err(flume::RecvError::Disconnected) => break,
                    }
                }
            }
        }
    }

    async fn rescan(&mut self) {
        let (truth, scanned_dirs) = scan(&self.pool, &self.root, self.recursive).await;
        if scanned_dirs.is_empty() {
            tracing::warn!("tree watcher scan read no directories; skipping prune this pass");
        }
        self.engine.reconcile(truth, &scanned_dirs);
    }

    /// Re-stat only the recently written files: a cheap backstop for content events that
    /// the backend coalesced or dropped, between full scans.
    async fn poll_hot(&mut self) {
        let hot: Vec<Arc<Path>> =
            self.engine.hot_files(SystemTime::now(), self.hot_window).collect();
        if hot.is_empty() {
            return;
        }
        for (path, (kind, mark)) in stat_paths(&self.pool, hot).await {
            self.engine.observe(path, kind, mark, Origin::Scan);
        }
    }
}

const DEFAULT_SCAN_INTERVAL: NonZeroDuration =
    NonZeroDuration::from_secs(NonZeroU64::new(30).unwrap());
const DEFAULT_CONTENT_POLL_INTERVAL: NonZeroDuration =
    NonZeroDuration::from_secs(NonZeroU64::new(2).unwrap());
const DEFAULT_HOT_WINDOW: Duration = Duration::from_secs(15 * 60);
const DEFAULT_DEBOUNCE_TIMEOUT: NonZeroDuration =
    NonZeroDuration::new(Duration::from_millis(250)).unwrap();

/// Builder for a [`TreeWatcher`].
pub struct TreeWatcherBuilder {
    pool: BlockingPool,
    recursive: bool,
    scan_interval: NonZeroDuration,
    content_poll_interval: NonZeroDuration,
    hot_window: Duration,
    debounce_timeout: NonZeroDuration,
}

impl TreeWatcherBuilder {
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

    /// How recently a file must have been written for the content poll to re-stat it
    /// (default: 15 minutes). Older files are still covered by events and the full scan.
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

    /// Start watching `root`, yielding each regular file whose path `filter` accepts.
    ///
    /// `filter` runs on the watcher's task, once per file path that appears, so it must not block.
    pub fn watch<F>(
        self,
        root: impl AsRef<Path>,
        filter: F,
    ) -> Result<TreeWatcher, TreeWatcherError>
    where
        F: Fn(&Path) -> bool + Send + 'static,
    {
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

        // Unbounded so a slow consumer never stalls the engine; it holds one item per file.
        let (found, files) = flume::unbounded();
        let engine = Engine {
            filter,
            root: Arc::from(root.as_path()),
            entries: HashMap::new(),
            found,
        };
        let poller = TreeWatcherPoller {
            root,
            recursive: self.recursive,
            hot_window: self.hot_window,
            pool: self.pool,
            engine,
        };
        let task = tokio::spawn(poller.run(rx, self.scan_interval, self.content_poll_interval));

        Ok(TreeWatcher {
            files: files.into_stream(),
            task,
            _debouncer: debouncer,
        })
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
    /// Begin configuring a watcher whose filesystem work runs in `pool`.
    #[must_use]
    pub fn builder(pool: BlockingPool) -> TreeWatcherBuilder {
        TreeWatcherBuilder {
            pool,
            recursive: true,
            scan_interval: DEFAULT_SCAN_INTERVAL,
            content_poll_interval: DEFAULT_CONTENT_POLL_INTERVAL,
            hot_window: DEFAULT_HOT_WINDOW,
            debounce_timeout: DEFAULT_DEBOUNCE_TIMEOUT,
        }
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use futures::FutureExt;
    use notify::event::{DataChange, MetadataKind};
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn pool() -> BlockingPool {
        BlockingPool::new(std::num::NonZeroUsize::MIN)
    }

    fn engine(
        filter: impl Fn(&Path) -> bool,
    ) -> (Engine<impl Fn(&Path) -> bool>, flume::Receiver<WatchedFile>) {
        let (found, files) = flume::unbounded();
        let engine = Engine {
            filter,
            root: ap("/r"),
            entries: HashMap::new(),
            found,
        };
        (engine, files)
    }

    fn accept_all_engine() -> (Engine<impl Fn(&Path) -> bool>, flume::Receiver<WatchedFile>) {
        engine(|_| true)
    }

    fn ap(path: &str) -> Arc<Path> {
        Arc::from(Path::new(path))
    }

    fn nz(d: Duration) -> NonZeroDuration {
        NonZeroDuration::new(d).unwrap()
    }

    fn scanned(dirs: &[&str]) -> HashSet<Arc<Path>> {
        dirs.iter().map(|d| ap(d)).collect()
    }

    fn event(kind: EventKind, paths: Vec<PathBuf>) -> notify::Event {
        notify::Event {
            kind,
            paths,
            attrs: notify::event::EventAttributes::new(),
        }
    }

    /// A regular-file mark distinguished by size alone.
    fn mark(size: u64) -> ContentMark {
        ContentMark {
            size,
            modified: None,
            identity: None,
        }
    }

    /// What a stat of a node of `kind` reports: regular files always carry a mark.
    fn observed(kind: FileKind) -> Observed {
        (kind, (kind == FileKind::File).then(|| mark(0)))
    }

    fn kinds_map<const N: usize>(entries: [(&str, FileKind); N]) -> HashMap<Arc<Path>, Observed> {
        entries.into_iter().map(|(p, k)| (ap(p), observed(k))).collect()
    }

    fn marked_map<const N: usize>(entries: [(&str, u64); N]) -> HashMap<Arc<Path>, Observed> {
        entries.into_iter().map(|(p, size)| (ap(p), (FileKind::File, Some(mark(size))))).collect()
    }

    fn truth(ids: &[&str]) -> Vec<(Arc<Path>, Observed)> {
        ids.iter().map(|id| (ap(&format!("/r/{id}")), observed(FileKind::File))).collect()
    }

    fn keys(engine: &Engine<impl Fn(&Path) -> bool>) -> HashSet<Arc<Path>> {
        engine.entries.keys().cloned().collect()
    }

    /// The paths of `files` whose stat has closed: the watcher stopped tracking them.
    fn gone(files: &[WatchedFile]) -> HashSet<Arc<Path>> {
        files
            .iter()
            .filter(|file| file.stat.has_changed().is_err())
            .map(|file| Arc::clone(&file.path))
            .collect()
    }

    fn live(files: &[WatchedFile]) -> usize {
        files.iter().filter(|file| file.stat.has_changed().is_ok()).count()
    }

    fn scanned_kinds(
        entries: &[(Arc<Path>, Observed)],
        root: &Path,
    ) -> std::collections::BTreeMap<String, FileKind> {
        entries
            .iter()
            .map(|(p, (k, _))| {
                (p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), *k)
            })
            .collect()
    }

    #[rstest]
    #[case(false, false)]
    #[case(true, true)]
    fn scan_fs_recursion_controls_descent(#[case] recursive: bool, #[case] expect_nested: bool) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), b"x").unwrap();

        let (found, dirs) = scan_fs(dir.path(), recursive);
        assert!(dirs.contains(dir.path()));
        let map = scanned_kinds(&found, dir.path());
        assert_eq!(map.get("a"), Some(&FileKind::File));
        assert_eq!(map.get("sub"), Some(&FileKind::Dir));
        assert_eq!(map.get("sub/b"), expect_nested.then_some(&FileKind::File));
    }

    #[cfg(unix)]
    #[rstest]
    fn scan_fs_does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/inner"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();

        let (found, dirs) = scan_fs(dir.path(), true);
        assert!(dirs.contains(dir.path()));
        let map = scanned_kinds(&found, dir.path());
        assert_eq!(map.get("link"), Some(&FileKind::Symlink));
        assert!(!map.contains_key("link/inner"));
        assert_eq!(map.get("real/inner"), Some(&FileKind::File));
    }

    #[rstest]
    #[tokio::test]
    async fn scan_reports_incomplete_for_missing_root() {
        let missing = Path::new("/this/does/not/exist/anywhere");
        let (found, dirs) = scan(&pool(), missing, true).await;
        assert!(found.is_empty());
        assert!(dirs.is_empty());
    }

    #[rstest]
    fn scan_fs_reports_incomplete_on_unreadable_root() {
        let (found, dirs) = scan_fs(Path::new("/this/does/not/exist/anywhere"), true);
        assert!(found.is_empty());
        assert!(dirs.is_empty());
    }

    #[rstest]
    fn observe_yields_a_file_once() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Event);
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(found.len(), 1);
    }

    #[rstest]
    fn observe_re_yields_a_file_after_a_kind_change() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/x"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/x"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/x"), FileKind::File, Some(mark(1)), Origin::Scan);
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 2);
        assert!(files[0].stat.has_changed().is_err(), "the replaced file's stat stayed open");
        assert!(files[1].stat.has_changed().is_ok());
    }

    // A directory that turns into a file takes its tracked children with it; otherwise their
    // stats outlive the file's own removal until the next full scan.
    #[rstest]
    fn observe_kind_change_from_dir_drops_descendants() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/x"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/x/c"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/x"), FileKind::File, Some(mark(1)), Origin::Scan);
        assert_eq!(keys(&engine), std::iter::once(ap("/r/x")).collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/x/c")).collect());
    }

    // The filter sees each file path once, never directories, and a rejected file stays
    // rejected across content changes.
    #[rstest]
    fn rejected_files_are_recorded_and_not_re_offered() {
        let offers = Arc::new(AtomicU64::new(0));
        let o = Arc::clone(&offers);
        let (mut engine, found) = engine(move |_| {
            o.fetch_add(1, Ordering::SeqCst);
            false
        });
        engine.observe(ap("/r/d"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/f"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/f"), FileKind::File, Some(mark(2)), Origin::Scan);
        assert_eq!(offers.load(Ordering::SeqCst), 1);
        assert!(found.is_empty());
        assert!(matches!(engine.entries.get(Path::new("/r/f")), Some(Slot::Untracked(_))));
    }

    #[rstest]
    fn forget_closes_the_stat() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Event);
        engine.entries.remove(Path::new("/r/a"));
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn forget_tree_drops_subtree() {
        let (mut engine, _found) = accept_all_engine();
        engine.observe(ap("/r/sub"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/sub/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/sub2/b"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.forget_tree(Path::new("/r/sub"));
        assert_eq!(keys(&engine), std::iter::once(ap("/r/sub2/b")).collect());
    }

    // A tracked file has no descendants, so forgetting it must be a single removal rather than
    // a sweep: a (stale) entry under its path is the witness that no prefix scan ran.
    #[rstest]
    fn forget_tree_of_a_file_removes_only_that_entry() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/a/stale"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/b"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.forget_tree(Path::new("/r/a"));
        assert_eq!(keys(&engine), [ap("/r/a/stale"), ap("/r/b")].into_iter().collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn reconcile_adds_and_removes() {
        let (mut engine, found) = accept_all_engine();
        engine.reconcile(truth(&["a", "b"]), &scanned(&["/r"]));
        assert_eq!(found.len(), 2);
        engine.reconcile(truth(&["b", "c"]), &scanned(&["/r"]));
        assert_eq!(keys(&engine), [ap("/r/b"), ap("/r/c")].into_iter().collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 3);
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn reconcile_repairs_kind_swap() {
        let (mut engine, found) = accept_all_engine();
        engine.reconcile(vec![(ap("/r/x"), observed(FileKind::Dir))], &scanned(&["/r"]));
        engine.reconcile(vec![(ap("/r/x"), observed(FileKind::File))], &scanned(&["/r"]));
        assert_eq!(engine.entries.get(Path::new("/r/x")).map(Slot::kind), Some(FileKind::File));
        assert_eq!(found.len(), 1);
    }

    #[rstest]
    fn reconcile_without_prune_keeps_missing() {
        let (mut engine, found) = accept_all_engine();
        engine.reconcile(truth(&["a", "b"]), &scanned(&["/r"]));
        // A scan that read no directories prunes nothing, so "b" must survive.
        engine.reconcile(truth(&["a"]), &HashSet::new());
        assert_eq!(keys(&engine), [ap("/r/a"), ap("/r/b")].into_iter().collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(live(&files), 2);
    }

    #[rstest]
    fn reconcile_keeps_nodes_under_unreadable_dir() {
        let (mut engine, _found) = accept_all_engine();
        engine.observe(ap("/r/gone"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/sub/kept"), FileKind::File, Some(mark(1)), Origin::Scan);
        // The scan read /r and saw /r/sub, but could not read /r/sub itself.
        engine.reconcile(vec![(ap("/r/sub"), observed(FileKind::Dir))], &scanned(&["/r"]));
        // /r/gone: parent /r read, absent from truth -> pruned.
        // /r/sub/kept: shielded by /r/sub, which exists but was not read -> kept.
        assert_eq!(keys(&engine), [ap("/r/sub"), ap("/r/sub/kept")].into_iter().collect());
    }

    #[rstest]
    fn reconcile_prunes_removed_subtree_descendants() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/sub"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/sub/child"), FileKind::File, Some(mark(1)), Origin::Scan);
        // /r/sub was removed (missed event): the scan read /r, /r/sub is absent and
        // cannot be scanned, so both it and its descendants must be pruned.
        engine.reconcile(vec![], &scanned(&["/r"]));
        assert!(engine.entries.is_empty());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(live(&files), 0);
    }

    #[rstest]
    #[case(true, 1)]
    #[case(false, 0)]
    fn apply_create_event_observes_only_existing(#[case] exists: bool, #[case] yielded: usize) {
        let (mut engine, found) = accept_all_engine();
        let kinds = if exists {
            kinds_map([("/r/a", FileKind::File)])
        } else {
            HashMap::new()
        };
        let ev =
            event(EventKind::Create(notify::event::CreateKind::Any), vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &kinds);
        assert_eq!(engine.entries.contains_key(Path::new("/r/a")), exists);
        assert_eq!(found.len(), yielded);
    }

    #[rstest]
    fn apply_remove_event_forgets() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Event);
        let ev =
            event(EventKind::Remove(notify::event::RemoveKind::Any), vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &HashMap::new());
        assert!(engine.entries.is_empty());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn apply_rename_both_moves_the_file() {
        let (mut engine, _found) = accept_all_engine();
        engine.observe(ap("/r/from"), FileKind::File, Some(mark(1)), Origin::Event);
        let kinds = kinds_map([("/r/to", FileKind::File)]);
        let ev = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            vec![PathBuf::from("/r/from"), PathBuf::from("/r/to")],
        );
        engine.apply_event(&ev, &kinds);
        assert!(!engine.entries.contains_key(Path::new("/r/from")));
        assert!(engine.entries.contains_key(Path::new("/r/to")));
    }

    #[rstest]
    fn apply_rename_any_observes_moved_in_file() {
        let (mut engine, found) = accept_all_engine();
        let kinds = kinds_map([("/r/a", FileKind::File)]);
        let ev = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            vec![PathBuf::from("/r/a")],
        );
        engine.apply_event(&ev, &kinds);
        assert!(engine.entries.contains_key(Path::new("/r/a")));
        assert_eq!(found.len(), 1);
    }

    #[rstest]
    fn apply_rename_any_forgets_moved_out_file() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Event);
        let ev = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            vec![PathBuf::from("/r/a")],
        );
        engine.apply_event(&ev, &HashMap::new());
        assert!(engine.entries.is_empty());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn mark_change_updates_the_stat_in_place() {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(2)), Origin::Event);
        let mut files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 1, "file was re-yielded");
        let mut stat = files.pop().unwrap().stat;
        assert!(stat.has_changed().unwrap());
        let seen = *stat.borrow_and_update();
        assert_eq!((seen.size(), seen.origin()), (2, Origin::Event));
        assert_eq!(engine.entries.get(Path::new("/r/a")).and_then(Slot::mark), Some(mark(2)));

        engine.observe(ap("/r/a"), FileKind::File, Some(mark(2)), Origin::Scan);
        assert!(!stat.has_changed().unwrap(), "same mark must be a no-op");
    }

    #[rstest]
    #[case::data(EventKind::Modify(ModifyKind::Data(DataChange::Any)))]
    #[case::metadata(EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)))]
    #[case::any(EventKind::Modify(ModifyKind::Any))]
    #[case::close_write(EventKind::Access(AccessKind::Close(AccessMode::Write)))]
    fn apply_content_event_signals_change(#[case] kind: EventKind) {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        let ev = event(kind, vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &marked_map([("/r/a", 2)]));
        let mut files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 1);
        let mut stat = files.pop().unwrap().stat;
        assert!(stat.has_changed().unwrap());
        let seen = *stat.borrow_and_update();
        assert_eq!((seen.origin(), seen.size()), (Origin::Event, 2));
    }

    // The root is watched, not tracked: its own metadata changes must not offer it as a node.
    #[rstest]
    fn apply_content_event_ignores_the_root() {
        let (mut engine, found) = accept_all_engine();
        let ev = event(
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)),
            vec![PathBuf::from("/r")],
        );
        engine.apply_event(&ev, &kinds_map([("/r", FileKind::Dir)]));
        assert!(engine.entries.is_empty());
        assert!(found.is_empty());
    }

    // FSEvents reports a young file's removal as `Create`+`Remove` (sticky flags), which the
    // debouncer cancels out, leaving only a content event for the now-missing path.
    #[rstest]
    #[case::data(EventKind::Modify(ModifyKind::Data(DataChange::Any)))]
    #[case::metadata(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Extended)))]
    fn apply_content_event_forgets_vanished_file(#[case] kind: EventKind) {
        let (mut engine, found) = accept_all_engine();
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        let ev = event(kind, vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &HashMap::new());
        assert!(engine.entries.is_empty());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
        assert_eq!(files[0].stat.borrow().size(), 1, "a vanished file is not a change");
    }

    #[rstest]
    fn reconcile_signals_mark_change() {
        let (mut engine, found) = accept_all_engine();
        let at = |size| vec![(ap("/r/a"), (FileKind::File, Some(mark(size))))];
        engine.reconcile(at(1), &scanned(&["/r"]));
        let mut stat = found.recv().unwrap().stat;
        engine.reconcile(at(1), &scanned(&["/r"]));
        assert!(!stat.has_changed().unwrap());
        engine.reconcile(at(2), &scanned(&["/r"]));
        assert!(stat.has_changed().unwrap());
        assert!(found.is_empty(), "file was re-yielded");
        let seen = *stat.borrow_and_update();
        assert_eq!((seen.origin(), seen.size()), (Origin::Scan, 2));
    }

    #[rstest]
    #[case::fresh(Some(0), true)]
    #[case::at_window(Some(60), true)]
    #[case::past_window(Some(61), false)]
    #[case::future(Some(-5), true)]
    #[case::far_future(Some(-61), false)]
    #[case::unknown(None, true)]
    fn content_mark_hotness(#[case] age_secs: Option<i64>, #[case] hot: bool) {
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
            identity: None,
        };
        assert_eq!(mark.is_hot(now, Duration::from_secs(60)), hot);
    }

    #[rstest]
    fn hot_files_selects_recently_written_tracked_files() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let written = |age_secs: u64| {
            Some(ContentMark {
                size: 1,
                modified: Some(now - Duration::from_secs(age_secs)),
                identity: None,
            })
        };
        let (mut engine, _found) = engine(|path| !path.ends_with("rejected"));
        engine.observe(ap("/r/hot"), FileKind::File, written(30), Origin::Scan);
        engine.observe(ap("/r/cold"), FileKind::File, written(120), Origin::Scan);
        engine.observe(ap("/r/rejected"), FileKind::File, written(0), Origin::Scan);
        engine.observe(ap("/r/dir"), FileKind::Dir, None, Origin::Scan);

        let hot: HashSet<Arc<Path>> = engine.hot_files(now, Duration::from_secs(60)).collect();
        assert_eq!(hot, std::iter::once(ap("/r/hot")).collect());
    }

    proptest! {
        #[test]
        fn reconcile_entries_always_equal_truth(
            states in prop::collection::vec(prop::collection::hash_set(0u16..24, 0..24), 1..24)
        ) {
            let (mut engine, found) = accept_all_engine();
            let mut files = Vec::new();
            for state in &states {
                let want: HashSet<Arc<Path>> = state
                    .iter()
                    .map(|id| ap(&format!("/r/{id}")))
                    .collect();
                let truth: Vec<(Arc<Path>, Observed)> =
                    want.iter().cloned().map(|p| (p, observed(FileKind::File))).collect();
                engine.reconcile(truth, &scanned(&["/r"]));
                prop_assert_eq!(keys(&engine), want);
                files.extend(found.drain());
            }
            prop_assert_eq!(live(&files), engine.entries.len());
        }

        #[test]
        fn reconcile_with_rejections_keeps_one_stat_per_tracked_file(
            states in prop::collection::vec(prop::collection::hash_set(0u16..24, 0..24), 1..16)
        ) {
            let (mut engine, found) = engine(|path| {
                path.file_name().is_some_and(|name| name.to_string_lossy().parse::<u16>().unwrap() % 3 != 0)
            });
            let mut files = Vec::new();
            for state in &states {
                let truth: Vec<(Arc<Path>, Observed)> = state
                    .iter()
                    .map(|id| {
                        let kind = if id % 2 == 0 { FileKind::File } else { FileKind::Dir };
                        (ap(&format!("/r/{id}")), observed(kind))
                    })
                    .collect();
                engine.reconcile(truth, &scanned(&["/r"]));
                files.extend(found.drain());
            }
            let tracked = engine
                .entries
                .values()
                .filter(|slot| matches!(slot, Slot::Tracked(..)))
                .count();
            prop_assert_eq!(live(&files), tracked);
        }
    }

    /// A watcher over `root` that yields every file, with the scan and debounce tightened to
    /// `scan_ms` and `debounce_ms`.
    fn watcher(root: &Path, scan_ms: u64, debounce_ms: u64) -> TreeWatcher {
        TreeWatcher::builder(pool())
            .scan_interval(nz(Duration::from_millis(scan_ms)))
            .debounce_timeout(nz(Duration::from_millis(debounce_ms)))
            .watch(root, |_| true)
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
        let mut watcher = TreeWatcher::builder(pool())
            .recursive(recursive)
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path(), |_| true)
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
        let err = TreeWatcher::builder(pool())
            .watch(root, |_| true)
            .err()
            .expect("bad root must be rejected");
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

        let mut watcher = TreeWatcher::builder(pool())
            .scan_interval(nz(Duration::from_secs(30)))
            .content_poll_interval(nz(Duration::from_secs(30)))
            .debounce_timeout(nz(Duration::from_millis(20)))
            .watch(dir.path(), |_| true)
            .unwrap();
        let mut stat = found(&mut watcher, "grow.log").await.stat;

        Mutation::Append.apply(&file, 7);
        let seen = changed_to(&mut stat, 6).await;
        assert_eq!(seen.origin(), Origin::Event);
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

        let mut watcher = TreeWatcher::builder(pool())
            .scan_interval(nz(Duration::from_millis(scan_ms)))
            .content_poll_interval(nz(Duration::from_millis(poll_ms)))
            .debounce_timeout(nz(Duration::from_secs(30)))
            .watch(dir.path(), |_| true)
            .unwrap();
        let mut stat = found(&mut watcher, "late.log").await.stat;

        Mutation::Append.apply(&file, 7);
        let seen = changed_to(&mut stat, 6).await;
        assert_eq!(seen.origin(), Origin::Scan);
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
