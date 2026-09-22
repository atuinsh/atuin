//! Track a caller-chosen handler for every file, directory, and symlink under a directory, kept in
//! sync with the filesystem.
//!
//! A [`TreeWatcher`] walks a `root` directory and, for each node it finds, calls a factory closure
//! with a [`NodeContext`]. The factory must return `Some(handler)` to track the node, or `None` to
//! decline tracking it.
//!
//! A tracked node's handler is a [`NodeHandler`]: it is stored and kept alive for as long as the
//! node exists, and its [`on_change`](NodeHandler::on_change) is called whenever a regular file's
//! content changes (size, modification time or identity). A decline is remembered, so the factory
//! is never asked about the same path twice while it stays unchanged; a declined file whose content
//! changes is offered again. A [`tokio::sync::watch::Sender<()>`] is the ready-made handler for
//! consumers that only need a wakeup to re-read the file.
//!
//! Content changes reach a handler by three paths, all applied on the watcher's own task:
//! filesystem events (fastest, but backends coalesce and drop them), the periodic full scan
//! ([`scan_interval`](TreeWatcherBuilder::scan_interval), which also discovers and prunes nodes)
//! and a cheaper content poll ([`content_poll_interval`](TreeWatcherBuilder::content_poll_interval))
//! that re-stats only files written within the [`hot_window`](TreeWatcherBuilder::hot_window).
//! Changes are coalesced by comparing stat facts, so a burst of writes yields one or a few calls,
//! never one per write.
//!
//! The watcher runs on a background Tokio task (so it must be created from within a Tokio runtime)
//! and keeps running until it is dropped, which stops watching and drops every handler. Use
//! [`recursive`](TreeWatcherBuilder::recursive) to control whether subdirectories are descended.
//!
//! Platform notes: on Windows a content change is detected from size and modification time alone,
//! as file identity is unix-only. On Linux, a subtree whose inotify watches could not be added
//! (the per-user limit is exhausted) gets no events and degrades to the periodic scan plus the
//! hot-set poll. BSD/kqueue backends hold one descriptor per watched node and are not a supported
//! target for the daemon.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! use atuin_common::fs::tree_watcher::{ChangeEvent, NodeContext, NodeHandler, TreeWatcher};
//!
//! // One handler per watched file: building it means the file appeared, a change
//! // means its content moved, dropping it means the file went away.
//! struct WatchedFile(Arc<Path>);
//!
//! impl NodeHandler for WatchedFile {
//!     fn on_change(&mut self, change: &ChangeEvent) {
//!         println!("changed:  {} ({} bytes)", self.0.display(), change.size());
//!     }
//! }
//!
//! impl Drop for WatchedFile {
//!     fn drop(&mut self) {
//!         println!("gone:     {}", self.0.display());
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let watcher = TreeWatcher::watch("/var/log", |ctx: NodeContext| {
//!     // Runs once per file that appears — via a filesystem event or the periodic
//!     // scan. Returning `None` ignores anything that isn't a regular file.
//!     if !ctx.is_file() {
//!         return None;
//!     }
//!     println!("appeared: {}", ctx.path().display());
//!     Some(WatchedFile(ctx.into_path()))
//! })?;
//!
//! // What you observe as the tree changes under /var/log:
//! //   create app.log     -> factory runs       -> "appeared: /var/log/app.log"
//! //   append to app.log  -> on_change runs     -> "changed:  /var/log/app.log (N bytes)"
//! //   rm app.log         -> WatchedFile drops   -> "gone:     /var/log/app.log"
//! //   mv a.log b.log     -> "gone: …/a.log" then "appeared: …/b.log"
//! //   drop(watcher)      -> every WatchedFile drops
//! # let _ = watcher;
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::os::fs::FdIdentity;
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
pub enum FileKind {
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

/// How a node, or a change to it, came to the watcher's attention.
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

/// A content change to a tracked regular file, delivered to its [`NodeHandler`].
///
/// The size, modification time and identity are the facts the change was detected from; they
/// are informational, and a consumer that re-reads the file needs none of them to be correct.
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    path: Arc<Path>,
    kind: FileKind,
    origin: Origin,
    mark: ContentMark,
}

impl ChangeEvent {
    /// The changed node's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The changed node's [`FileKind`].
    #[must_use]
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    /// Whether an event, or the scan or content poll, surfaced this change.
    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// The file's size in bytes when the change was detected.
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
}

/// A handler the factory builds per tracked node: kept alive while the node exists, and told
/// about content changes to regular files.
pub trait NodeHandler: Send + 'static {
    /// The file's content changed (size, modification time or identity). Runs on the watcher's
    /// task, so it must not block: forward a wakeup and return.
    fn on_change(&mut self, change: &ChangeEvent);
}

/// A liveness-only handler: tracks existence, ignores content changes.
impl NodeHandler for () {
    fn on_change(&mut self, _change: &ChangeEvent) {}
}

/// A coalescing wakeup: every change bumps the channel version, so a receiver's
/// [`changed`](tokio::sync::watch::Receiver::changed) wakes once per burst, and fails once the
/// node is gone and the sender dropped.
impl NodeHandler for tokio::sync::watch::Sender<()> {
    fn on_change(&mut self, _change: &ChangeEvent) {
        self.send_replace(());
    }
}

/// A node offered to the factory, with how it was found.
#[derive(Debug, Clone)]
pub struct NodeContext {
    path: Arc<Path>,
    kind: FileKind,
    origin: Origin,
}

impl NodeContext {
    /// The node's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The node's [`FileKind`].
    #[must_use]
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    /// Whether an event or the scan surfaced this node.
    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// Whether this node is a regular file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        matches!(self.kind, FileKind::File)
    }

    /// Whether this node is a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, FileKind::Dir)
    }

    /// Whether this node is a symlink.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, FileKind::Symlink)
    }

    /// Consume the context into its shared path handle.
    #[must_use]
    pub fn into_path(self) -> Arc<Path> {
        self.path
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

async fn scan(root: &Path, recursive: bool) -> ScanResult {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || scan_fs(&root, recursive))
        .await
        .unwrap_or_else(|_| (Vec::new(), HashSet::new()))
}

/// Stat `paths` off the async executor, keeping only the ones that still exist.
async fn stat_paths(paths: Vec<Arc<Path>>) -> HashMap<Arc<Path>, Observed> {
    let stat = tokio::task::spawn_blocking(move || {
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
async fn resolve_kinds(events: &[notify::Event]) -> HashMap<Arc<Path>, Observed> {
    let paths = events
        .iter()
        .flat_map(|event| event.paths.iter().map(|p| Arc::from(p.as_path())))
        .collect();
    stat_paths(paths).await
}

enum Slot<H> {
    Active(FileKind, Option<ContentMark>, H),
    Declined(FileKind, Option<ContentMark>),
}

impl<H> Slot<H> {
    fn kind(&self) -> FileKind {
        match self {
            Self::Active(kind, ..) | Self::Declined(kind, _) => *kind,
        }
    }

    fn mark(&self) -> Option<ContentMark> {
        match self {
            Self::Active(_, mark, _) | Self::Declined(_, mark) => *mark,
        }
    }
}

struct Engine<H, F> {
    factory: F,
    root: Arc<Path>,
    entries: HashMap<Arc<Path>, Slot<H>>,
}

impl<H, F> Engine<H, F>
where
    H: NodeHandler,
    F: Fn(NodeContext) -> Option<H>,
{
    /// Bring the slot for `path` in line with what was just observed on disk: build a handler
    /// for a new or kind-changed node, signal a tracked file whose content mark moved, and
    /// re-offer a declined file whose content changed.
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
            match slot {
                Slot::Active(_, known, handler) => {
                    if *known != mark {
                        *known = mark;
                        // The handler is signalled in place: rebuilding it would
                        // reset whatever read position its owner keeps.
                        if let Some(mark) = mark {
                            handler.on_change(&ChangeEvent {
                                path,
                                kind,
                                origin,
                                mark,
                            });
                        }
                    }
                    return;
                }
                Slot::Declined(_, known) if *known == mark => return,
                Slot::Declined(..) => {}
            }
        }

        // A directory that became something else takes its tracked descendants with it.
        if self.entries.get(&path).is_some_and(|slot| slot.kind() == FileKind::Dir) {
            self.forget_tree(&path);
        } else {
            self.entries.remove(&path);
        }
        let ctx = NodeContext {
            path: Arc::clone(&path),
            kind,
            origin,
        };
        let slot = (self.factory)(ctx)
            .map_or(Slot::Declined(kind, mark), |handler| Slot::Active(kind, mark, handler));
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
            Slot::Active(_, Some(mark), _) if mark.is_hot(now, window) => Some(Arc::clone(path)),
            _ => None,
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
                // case-only rename (`a.log` -> `A.log`) leaves duplicate handlers:
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
struct TreeWatcherPoller<H, F> {
    root: PathBuf,
    recursive: bool,
    hot_window: Duration,
    engine: Engine<H, F>,
}

impl<H, F> TreeWatcherPoller<H, F>
where
    H: NodeHandler,
    F: Fn(NodeContext) -> Option<H> + Send + 'static,
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
                                let kinds = resolve_kinds(&pending).await;
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
        let (truth, scanned_dirs) = scan(&self.root, self.recursive).await;
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
        for (path, (kind, mark)) in stat_paths(hot).await {
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
    recursive: bool,
    scan_interval: NonZeroDuration,
    content_poll_interval: NonZeroDuration,
    hot_window: Duration,
    debounce_timeout: NonZeroDuration,
}

impl Default for TreeWatcherBuilder {
    fn default() -> Self {
        Self {
            recursive: true,
            scan_interval: DEFAULT_SCAN_INTERVAL,
            content_poll_interval: DEFAULT_CONTENT_POLL_INTERVAL,
            hot_window: DEFAULT_HOT_WINDOW,
            debounce_timeout: DEFAULT_DEBOUNCE_TIMEOUT,
        }
    }
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

    /// Start watching `root`, building a handler for each node the factory accepts.
    ///
    /// A factory returning `None` declines the node. If your factory returns `Some(T)`, then the
    /// value will be kept alive for as long as the filesystem node exists (or the watcher drops).
    pub fn watch<H, F>(
        self,
        root: impl AsRef<Path>,
        factory: F,
    ) -> Result<TreeWatcher, TreeWatcherError>
    where
        H: NodeHandler,
        F: Fn(NodeContext) -> Option<H> + Send + 'static,
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

        let engine = Engine {
            factory,
            root: Arc::from(root.as_path()),
            entries: HashMap::new(),
        };
        let poller = TreeWatcherPoller {
            root,
            recursive: self.recursive,
            hot_window: self.hot_window,
            engine,
        };
        let task = tokio::spawn(poller.run(rx, self.scan_interval, self.content_poll_interval));

        Ok(TreeWatcher {
            task,
            _debouncer: debouncer,
        })
    }
}

/// Watches a directory tree, holding one factory-built handler per live node.
#[must_use = "dropping the TreeWatcher stops watching"]
pub struct TreeWatcher {
    task: JoinHandle<()>,
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl TreeWatcher {
    /// Begin configuring a watcher.
    #[must_use]
    pub fn builder() -> TreeWatcherBuilder {
        TreeWatcherBuilder::default()
    }

    /// Watch `root` with default settings, building a handler for each accepted node.
    pub fn watch<H, F>(root: impl AsRef<Path>, factory: F) -> Result<Self, TreeWatcherError>
    where
        H: NodeHandler,
        F: Fn(NodeContext) -> Option<H> + Send + 'static,
    {
        Self::builder().watch(root, factory)
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
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    use notify::event::{DataChange, MetadataKind};
    use parking_lot::Mutex;
    use proptest::prelude::*;
    use rstest::rstest;
    use tokio::sync::mpsc::error::TryRecvError;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    use super::*;

    #[derive(Default)]
    struct Counters {
        alive: AtomicI64,
        created: AtomicU64,
        dropped: AtomicU64,
        changed: AtomicU64,
        last_change: Mutex<Option<ChangeEvent>>,
    }

    struct CountingHandler {
        counters: Arc<Counters>,
    }

    impl CountingHandler {
        fn new(counters: Arc<Counters>) -> Self {
            counters.alive.fetch_add(1, Ordering::SeqCst);
            counters.created.fetch_add(1, Ordering::SeqCst);
            Self { counters }
        }
    }

    impl NodeHandler for CountingHandler {
        fn on_change(&mut self, change: &ChangeEvent) {
            self.counters.changed.fetch_add(1, Ordering::SeqCst);
            *self.counters.last_change.lock() = Some(change.clone());
        }
    }

    impl Drop for CountingHandler {
        fn drop(&mut self) {
            self.counters.alive.fetch_sub(1, Ordering::SeqCst);
            self.counters.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn accept_all_engine(
        counters: &Arc<Counters>,
    ) -> Engine<CountingHandler, impl Fn(NodeContext) -> Option<CountingHandler>> {
        let counters = counters.clone();
        Engine {
            factory: move |_ctx| Some(CountingHandler::new(counters.clone())),
            root: ap("/r"),
            entries: HashMap::new(),
        }
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

    fn kinds_map<const N: usize>(entries: [(&str, FileKind); N]) -> HashMap<Arc<Path>, Observed> {
        entries.into_iter().map(|(p, k)| (ap(p), (k, None))).collect()
    }

    /// A regular-file mark distinguished by size alone.
    fn mark(size: u64) -> ContentMark {
        ContentMark {
            size,
            modified: None,
            identity: None,
        }
    }

    fn marked_map<const N: usize>(entries: [(&str, u64); N]) -> HashMap<Arc<Path>, Observed> {
        entries.into_iter().map(|(p, size)| (ap(p), (FileKind::File, Some(mark(size))))).collect()
    }

    fn truth(ids: &[&str]) -> Vec<(Arc<Path>, Observed)> {
        ids.iter().map(|id| (ap(&format!("/r/{id}")), (FileKind::File, None))).collect()
    }

    fn keys(
        engine: &Engine<CountingHandler, impl Fn(NodeContext) -> Option<CountingHandler>>,
    ) -> std::collections::HashSet<Arc<Path>> {
        engine.entries.keys().cloned().collect()
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
    #[case(FileKind::File, true, false, false)]
    #[case(FileKind::Dir, false, true, false)]
    #[case(FileKind::Symlink, false, false, true)]
    #[case(FileKind::Other, false, false, false)]
    fn node_context_kind_predicates(
        #[case] kind: FileKind,
        #[case] is_file: bool,
        #[case] is_dir: bool,
        #[case] is_symlink: bool,
    ) {
        let ctx = NodeContext {
            path: ap("/root/x"),
            kind,
            origin: Origin::Scan,
        };
        assert_eq!(ctx.is_file(), is_file);
        assert_eq!(ctx.is_dir(), is_dir);
        assert_eq!(ctx.is_symlink(), is_symlink);
        assert_eq!(ctx.path(), Path::new("/root/x"));
        assert_eq!(ctx.origin(), Origin::Scan);
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

    #[tokio::test]
    async fn scan_reports_incomplete_for_missing_root() {
        let missing = Path::new("/this/does/not/exist/anywhere");
        let (found, dirs) = scan(missing, true).await;
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
    fn observe_creates_one_handler_and_dedupes() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, None, Origin::Event);
        engine.observe(ap("/r/a"), FileKind::File, None, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 1);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn observe_rebuilds_on_kind_change() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/x"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/x"), FileKind::File, None, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 2);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    // A directory that turns into a file takes its tracked children with it; otherwise their
    // handlers outlive the file's own removal until the next full scan.
    #[rstest]
    fn observe_kind_change_from_dir_drops_descendants() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/x"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/x/c"), FileKind::File, None, Origin::Scan);
        engine.observe(ap("/r/x"), FileKind::File, None, Origin::Scan);
        assert_eq!(keys(&engine), std::iter::once(ap("/r/x")).collect());
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 2);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn declined_paths_are_recorded_and_not_re_offered() {
        let counters = Arc::new(Counters::default());
        let c = counters.clone();
        let mut engine = Engine {
            factory: move |ctx: NodeContext| ctx.is_file().then(|| CountingHandler::new(c.clone())),
            root: ap("/r"),
            entries: HashMap::new(),
        };
        engine.observe(ap("/r/d"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/d"), FileKind::Dir, None, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 0);
        assert!(matches!(engine.entries.get(Path::new("/r/d")), Some(Slot::Declined(..))));
    }

    #[rstest]
    fn forget_drops_handler() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, None, Origin::Event);
        engine.entries.remove(Path::new("/r/a"));
        assert!(engine.entries.is_empty());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 0);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn forget_tree_drops_subtree() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/sub"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/sub/a"), FileKind::File, None, Origin::Scan);
        engine.observe(ap("/r/sub2/b"), FileKind::File, None, Origin::Scan);
        engine.forget_tree(Path::new("/r/sub"));
        assert_eq!(keys(&engine), std::iter::once(ap("/r/sub2/b")).collect());
    }

    // A tracked file has no descendants, so forgetting it must be a single removal rather than
    // a sweep: a (stale) entry under its path is the witness that no prefix scan ran.
    #[rstest]
    fn forget_tree_of_a_file_removes_only_that_entry() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, None, Origin::Scan);
        engine.observe(ap("/r/a/stale"), FileKind::File, None, Origin::Scan);
        engine.observe(ap("/r/b"), FileKind::File, None, Origin::Scan);
        engine.forget_tree(Path::new("/r/a"));
        assert_eq!(keys(&engine), [ap("/r/a/stale"), ap("/r/b")].into_iter().collect());
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn reconcile_adds_and_removes() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.reconcile(truth(&["a", "b"]), &scanned(&["/r"]));
        assert_eq!(counters.alive.load(Ordering::SeqCst), 2);
        engine.reconcile(truth(&["b", "c"]), &scanned(&["/r"]));
        assert_eq!(keys(&engine), [ap("/r/b"), ap("/r/c")].into_iter().collect());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 2);
        assert_eq!(counters.created.load(Ordering::SeqCst), 3);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn reconcile_repairs_kind_swap() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.reconcile(vec![(ap("/r/x"), (FileKind::Dir, None))], &scanned(&["/r"]));
        engine.reconcile(vec![(ap("/r/x"), (FileKind::File, None))], &scanned(&["/r"]));
        assert_eq!(
            engine.entries.get(Path::new("/r/x")).map(|slot| slot.kind()),
            Some(FileKind::File)
        );
        assert_eq!(counters.created.load(Ordering::SeqCst), 2);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn reconcile_without_prune_keeps_missing() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.reconcile(truth(&["a", "b"]), &scanned(&["/r"]));
        // A scan that read no directories prunes nothing, so "b" must survive.
        engine.reconcile(truth(&["a"]), &HashSet::new());
        assert_eq!(keys(&engine), [ap("/r/a"), ap("/r/b")].into_iter().collect());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 2);
    }

    #[rstest]
    fn reconcile_keeps_nodes_under_unreadable_dir() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/gone"), FileKind::File, None, Origin::Scan);
        engine.observe(ap("/r/sub/kept"), FileKind::File, None, Origin::Scan);
        // The scan read /r and saw /r/sub, but could not read /r/sub itself.
        engine.reconcile(vec![(ap("/r/sub"), (FileKind::Dir, None))], &scanned(&["/r"]));
        // /r/gone: parent /r read, absent from truth -> pruned.
        // /r/sub/kept: shielded by /r/sub, which exists but was not read -> kept.
        assert_eq!(keys(&engine), [ap("/r/sub"), ap("/r/sub/kept")].into_iter().collect());
    }

    #[rstest]
    fn reconcile_prunes_removed_subtree_descendants() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/sub"), FileKind::Dir, None, Origin::Scan);
        engine.observe(ap("/r/sub/child"), FileKind::File, None, Origin::Scan);
        // /r/sub was removed (missed event): the scan read /r, /r/sub is absent and
        // cannot be scanned, so both it and its descendants must be pruned.
        engine.reconcile(vec![], &scanned(&["/r"]));
        assert!(engine.entries.is_empty());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 0);
    }

    #[rstest]
    #[case(true, 1)]
    #[case(false, 0)]
    fn apply_create_event_observes_only_existing(#[case] exists: bool, #[case] alive: i64) {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        let kinds = if exists {
            kinds_map([("/r/a", FileKind::File)])
        } else {
            HashMap::new()
        };
        let ev =
            event(EventKind::Create(notify::event::CreateKind::Any), vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &kinds);
        assert_eq!(engine.entries.contains_key(Path::new("/r/a")), exists);
        assert_eq!(counters.alive.load(Ordering::SeqCst), alive);
    }

    #[rstest]
    fn apply_remove_event_forgets() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, None, Origin::Event);
        let ev =
            event(EventKind::Remove(notify::event::RemoveKind::Any), vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &HashMap::new());
        assert!(engine.entries.is_empty());
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn apply_rename_both_moves_handler() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/from"), FileKind::File, None, Origin::Event);
        let kinds = kinds_map([("/r/to", FileKind::File)]);
        let ev = event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), vec![
            PathBuf::from("/r/from"),
            PathBuf::from("/r/to"),
        ]);
        engine.apply_event(&ev, &kinds);
        assert!(!engine.entries.contains_key(Path::new("/r/from")));
        assert!(engine.entries.contains_key(Path::new("/r/to")));
    }

    #[rstest]
    fn apply_rename_any_observes_moved_in_file() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        let kinds = kinds_map([("/r/a", FileKind::File)]);
        let ev = event(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), vec![PathBuf::from(
            "/r/a",
        )]);
        engine.apply_event(&ev, &kinds);
        assert!(engine.entries.contains_key(Path::new("/r/a")));
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn apply_rename_any_forgets_moved_out_file() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, None, Origin::Event);
        let ev = event(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), vec![PathBuf::from(
            "/r/a",
        )]);
        engine.apply_event(&ev, &HashMap::new());
        assert!(engine.entries.is_empty());
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn mark_change_signals_active_handler_in_place() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(2)), Origin::Event);
        assert_eq!(counters.changed.load(Ordering::SeqCst), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 1, "handler was rebuilt");
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 0, "handler was dropped");
        let change = counters.last_change.lock().take().unwrap();
        assert_eq!(change.path(), Path::new("/r/a"));
        assert_eq!(change.kind(), FileKind::File);
        assert_eq!(change.size(), 2);
        assert_eq!(change.origin(), Origin::Event);
        assert_eq!(engine.entries.get(Path::new("/r/a")).and_then(Slot::mark), Some(mark(2)));

        engine.observe(ap("/r/a"), FileKind::File, Some(mark(2)), Origin::Scan);
        assert_eq!(counters.changed.load(Ordering::SeqCst), 1, "same mark must be a no-op");
    }

    #[rstest]
    fn declined_mark_change_re_offers_factory() {
        let counters = Arc::new(Counters::default());
        let offers = Arc::new(AtomicU64::new(0));
        let (c, o) = (counters.clone(), offers.clone());
        // Declines the first two offers, accepts from the third on.
        let mut engine = Engine {
            factory: move |_ctx: NodeContext| {
                (o.fetch_add(1, Ordering::SeqCst) >= 2).then(|| CountingHandler::new(c.clone()))
            },
            root: ap("/r"),
            entries: HashMap::new(),
        };
        let slot_mark =
            |engine: &Engine<_, _>| engine.entries.get(Path::new("/r/a")).and_then(Slot::mark);

        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        assert_eq!(offers.load(Ordering::SeqCst), 1, "unchanged decline was re-offered");

        engine.observe(ap("/r/a"), FileKind::File, Some(mark(2)), Origin::Scan);
        assert_eq!(offers.load(Ordering::SeqCst), 2);
        assert!(matches!(engine.entries.get(Path::new("/r/a")), Some(Slot::Declined(..))));
        assert_eq!(
            slot_mark(&engine),
            Some(mark(2)),
            "a repeated decline must still update the mark"
        );
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(2)), Origin::Scan);
        assert_eq!(offers.load(Ordering::SeqCst), 2);

        engine.observe(ap("/r/a"), FileKind::File, Some(mark(3)), Origin::Scan);
        assert_eq!(offers.load(Ordering::SeqCst), 3);
        assert_eq!(counters.created.load(Ordering::SeqCst), 1);
        assert_eq!(counters.changed.load(Ordering::SeqCst), 0, "a re-offer is not a change");
        assert_eq!(slot_mark(&engine), Some(mark(3)));
    }

    #[rstest]
    #[case::data(EventKind::Modify(ModifyKind::Data(DataChange::Any)))]
    #[case::metadata(EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)))]
    #[case::any(EventKind::Modify(ModifyKind::Any))]
    #[case::close_write(EventKind::Access(AccessKind::Close(AccessMode::Write)))]
    fn apply_content_event_signals_change(#[case] kind: EventKind) {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        let ev = event(kind, vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &marked_map([("/r/a", 2)]));
        assert_eq!(counters.changed.load(Ordering::SeqCst), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 1);
        let change = counters.last_change.lock().take().unwrap();
        assert_eq!((change.origin(), change.size()), (Origin::Event, 2));
    }

    // The root is watched, not tracked: its own metadata changes must not offer it as a node.
    #[rstest]
    fn apply_content_event_ignores_the_root() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        let ev = event(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)), vec![
            PathBuf::from("/r"),
        ]);
        engine.apply_event(&ev, &kinds_map([("/r", FileKind::Dir)]));
        assert!(engine.entries.is_empty());
        assert_eq!(counters.created.load(Ordering::SeqCst), 0);
    }

    // FSEvents reports a young file's removal as `Create`+`Remove` (sticky flags), which the
    // debouncer cancels out, leaving only a content event for the now-missing path.
    #[rstest]
    #[case::data(EventKind::Modify(ModifyKind::Data(DataChange::Any)))]
    #[case::metadata(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Extended)))]
    fn apply_content_event_forgets_vanished_file(#[case] kind: EventKind) {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, Some(mark(1)), Origin::Scan);
        let ev = event(kind, vec![PathBuf::from("/r/a")]);
        engine.apply_event(&ev, &HashMap::new());
        assert!(engine.entries.is_empty());
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(counters.changed.load(Ordering::SeqCst), 0);
    }

    #[rstest]
    fn reconcile_signals_mark_change() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.reconcile(vec![(ap("/r/a"), (FileKind::File, Some(mark(1))))], &scanned(&["/r"]));
        engine.reconcile(vec![(ap("/r/a"), (FileKind::File, Some(mark(1))))], &scanned(&["/r"]));
        assert_eq!(counters.changed.load(Ordering::SeqCst), 0);
        engine.reconcile(vec![(ap("/r/a"), (FileKind::File, Some(mark(2))))], &scanned(&["/r"]));
        assert_eq!(counters.changed.load(Ordering::SeqCst), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 1);
        let change = counters.last_change.lock().take().unwrap();
        assert_eq!((change.origin(), change.size()), (Origin::Scan, 2));
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
    fn hot_files_selects_recently_written_active_files() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let written = |age_secs: u64| {
            Some(ContentMark {
                size: 1,
                modified: Some(now - Duration::from_secs(age_secs)),
                identity: None,
            })
        };
        let counters = Arc::new(Counters::default());
        let mut engine = Engine {
            factory: move |ctx: NodeContext| {
                (!ctx.path().ends_with("declined")).then(|| CountingHandler::new(counters.clone()))
            },
            root: ap("/r"),
            entries: HashMap::new(),
        };
        engine.observe(ap("/r/hot"), FileKind::File, written(30), Origin::Scan);
        engine.observe(ap("/r/cold"), FileKind::File, written(120), Origin::Scan);
        engine.observe(ap("/r/declined"), FileKind::File, written(0), Origin::Scan);
        engine.observe(ap("/r/dir"), FileKind::Dir, None, Origin::Scan);

        let hot: HashSet<Arc<Path>> = engine.hot_files(now, Duration::from_secs(60)).collect();
        assert_eq!(hot, std::iter::once(ap("/r/hot")).collect());
    }

    proptest! {
        #[test]
        fn reconcile_entries_always_equal_truth(
            states in prop::collection::vec(prop::collection::hash_set(0u16..24, 0..24), 1..24)
        ) {
            let counters = Arc::new(Counters::default());
            let mut engine = accept_all_engine(&counters);
            for state in &states {
                let want: std::collections::HashSet<Arc<Path>> = state
                    .iter()
                    .map(|id| ap(&format!("/r/{id}")))
                    .collect();
                let truth: Vec<(Arc<Path>, Observed)> =
                    want.iter().cloned().map(|p| (p, (FileKind::File, None))).collect();
                engine.reconcile(truth, &scanned(&["/r"]));
                prop_assert_eq!(keys(&engine), want);
            }
            let created = counters.created.load(Ordering::SeqCst);
            let dropped = counters.dropped.load(Ordering::SeqCst);
            let alive = counters.alive.load(Ordering::SeqCst);
            prop_assert_eq!(created - dropped, alive.cast_unsigned());
            prop_assert_eq!(usize::try_from(alive).unwrap(), engine.entries.len());
        }

        #[test]
        fn reconcile_with_declines_conserves_handlers(
            states in prop::collection::vec(prop::collection::hash_set(0u16..24, 0..24), 1..16)
        ) {
            let counters = Arc::new(Counters::default());
            let c = counters.clone();
            let mut engine = Engine {
                factory: move |ctx: NodeContext| {
                    ctx.is_file().then(|| CountingHandler::new(c.clone()))
                },
                root: ap("/r"),
                entries: HashMap::new(),
            };
            for state in &states {
                let truth: Vec<(Arc<Path>, Observed)> = state
                    .iter()
                    .map(|id| {
                        let kind = if id % 2 == 0 { FileKind::File } else { FileKind::Dir };
                        (ap(&format!("/r/{id}")), (kind, None))
                    })
                    .collect();
                engine.reconcile(truth, &scanned(&["/r"]));
            }
            let alive = usize::try_from(counters.alive.load(Ordering::SeqCst)).unwrap();
            let active = engine
                .entries
                .values()
                .filter(|slot| matches!(slot, Slot::Active(..)))
                .count();
            prop_assert_eq!(alive, active);
        }
    }

    struct Probe {
        path: PathBuf,
        dropped: UnboundedSender<PathBuf>,
    }

    impl NodeHandler for Probe {
        fn on_change(&mut self, _change: &ChangeEvent) {}
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.dropped.send(self.path.clone());
        }
    }

    struct ChangeProbe {
        _probe: Probe,
        changed: UnboundedSender<ChangeEvent>,
    }

    impl NodeHandler for ChangeProbe {
        fn on_change(&mut self, change: &ChangeEvent) {
            let _ = self.changed.send(change.clone());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn created_file_builds_handler() {
        let dir = tempfile::tempdir().unwrap();
        let (created_tx, mut created_rx) = unbounded_channel();
        let (drop_tx, _drop_rx) = unbounded_channel::<PathBuf>();
        let _watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path(), move |ctx| {
                let path = ctx.path().to_owned();
                created_tx.send(path.clone()).ok();
                Some(Probe {
                    path,
                    dropped: drop_tx.clone(),
                })
            })
            .unwrap();

        std::fs::write(dir.path().join("a.log"), b"x").unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), created_rx.recv())
            .await
            .expect("handler should be built")
            .unwrap();
        assert!(got.ends_with("a.log"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deleted_file_drops_handler() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b.log");
        std::fs::write(&file, b"x").unwrap();
        let (created_tx, mut created_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, mut drop_rx) = unbounded_channel::<PathBuf>();
        let _watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path(), move |ctx| {
                let path = ctx.path().to_owned();
                created_tx.send(path.clone()).ok();
                Some(Probe {
                    path,
                    dropped: drop_tx.clone(),
                })
            })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), created_rx.recv())
            .await
            .expect("initial handler should be built")
            .unwrap();
        std::fs::remove_file(&file).unwrap();
        let dropped = tokio::time::timeout(Duration::from_secs(5), drop_rx.recv())
            .await
            .expect("handler should be dropped")
            .unwrap();
        assert!(dropped.ends_with("b.log"));
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
        let (created_tx, mut created_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, _drop_rx) = unbounded_channel::<PathBuf>();
        let _watcher = TreeWatcher::builder()
            .recursive(recursive)
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path(), move |ctx| {
                if !ctx.is_file() {
                    return None;
                }
                let path = ctx.path().to_owned();
                created_tx.send(path.clone()).ok();
                Some(Probe {
                    path,
                    dropped: drop_tx.clone(),
                })
            })
            .unwrap();

        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/c.log"), b"x").unwrap();

        let fired =
            tokio::time::timeout(Duration::from_millis(wait_ms), created_rx.recv()).await.is_ok();
        assert_eq!(fired, expect_fire);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_watcher_drops_handlers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.log"), b"x").unwrap();
        let (created_tx, mut created_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, mut drop_rx) = unbounded_channel::<PathBuf>();
        let watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path(), move |ctx| {
                let path = ctx.path().to_owned();
                created_tx.send(path.clone()).ok();
                Some(Probe {
                    path,
                    dropped: drop_tx.clone(),
                })
            })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), created_rx.recv())
            .await
            .expect("initial handler should be built")
            .unwrap();
        drop(watcher);
        let dropped = tokio::time::timeout(Duration::from_secs(5), drop_rx.recv())
            .await
            .expect("handler should be dropped when watcher is dropped")
            .unwrap();
        assert!(dropped.ends_with("a.log"));
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
        let err = TreeWatcher::watch(root, |_ctx: NodeContext| Some(()))
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

    struct ChangeRig {
        _watcher: TreeWatcher,
        built: UnboundedReceiver<PathBuf>,
        changed: UnboundedReceiver<ChangeEvent>,
        dropped: UnboundedReceiver<PathBuf>,
    }

    fn watch_changes(builder: TreeWatcherBuilder, root: &Path) -> ChangeRig {
        let (built_tx, built) = unbounded_channel();
        let (changed_tx, changed) = unbounded_channel();
        let (drop_tx, dropped) = unbounded_channel();
        let watcher = builder
            .watch(root, move |ctx| {
                let path = ctx.path().to_owned();
                built_tx.send(path.clone()).ok();
                Some(ChangeProbe {
                    _probe: Probe {
                        path,
                        dropped: drop_tx.clone(),
                    },
                    changed: changed_tx.clone(),
                })
            })
            .unwrap();
        ChangeRig {
            _watcher: watcher,
            built,
            changed,
            dropped,
        }
    }

    impl ChangeRig {
        async fn built(&mut self, name: &str) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !self.built.recv().await.unwrap().ends_with(name) {}
            })
            .await
            .expect("handler is built");
        }

        /// Drain changes until one for `name` reports `size`. Bursts may deliver several
        /// changes and intermediate sizes, so only the final state is asserted on.
        async fn changed_to(&mut self, name: &str, size: u64) -> ChangeEvent {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let change = self.changed.recv().await.unwrap();
                    if change.path().ends_with(name) && change.size() == size {
                        break change;
                    }
                }
            })
            .await
            .expect("change is delivered")
        }
    }

    // A content write is neither a create/remove nor a kind change, so the handler must
    // survive untouched (not rebuilt by the event path, not churned by the periodic scan)
    // and instead be told about the change in place.
    #[rstest]
    #[case::append(Mutation::Append)]
    #[case::overwrite(Mutation::Overwrite)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn benign_mutation_does_not_rebuild_handler(#[case] mutation: Mutation) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keep.log");
        std::fs::write(&file, b"seed").unwrap();

        let builder = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(50)))
            .debounce_timeout(nz(Duration::from_millis(20)));
        let mut rig = watch_changes(builder, dir.path());
        rig.built("keep.log").await;
        while rig.built.try_recv().is_ok() {}

        for i in 0..3 {
            mutation.apply(&file, i);
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        // Several scan cycles and debounce windows: a bug that rebuilds on modify surfaces here.
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(matches!(rig.built.try_recv(), Err(TryRecvError::Empty)), "handler was rebuilt");
        assert!(matches!(rig.dropped.try_recv(), Err(TryRecvError::Empty)), "handler was dropped");
        let final_size = std::fs::metadata(&file).unwrap().len();
        let change = rig.changed_to("keep.log", final_size).await;
        assert_eq!(change.kind(), FileKind::File);
    }

    // Scan and poll intervals of 30s leave only the event fast path able to deliver in time.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_signals_change_via_events() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("grow.log");
        std::fs::write(&file, b"seed").unwrap();

        let builder = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_secs(30)))
            .content_poll_interval(nz(Duration::from_secs(30)))
            .debounce_timeout(nz(Duration::from_millis(20)));
        let mut rig = watch_changes(builder, dir.path());
        rig.built("grow.log").await;

        Mutation::Append.apply(&file, 7);
        let change = rig.changed_to("grow.log", 6).await;
        assert_eq!(change.origin(), Origin::Event);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn truncate_signals_smaller_size() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shrink.log");
        std::fs::write(&file, b"long seed").unwrap();

        let builder = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(20)));
        let mut rig = watch_changes(builder, dir.path());
        rig.built("shrink.log").await;

        std::fs::write(&file, b"s").unwrap();
        rig.changed_to("shrink.log", 1).await;
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

        let builder = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(scan_ms)))
            .content_poll_interval(nz(Duration::from_millis(poll_ms)))
            .debounce_timeout(nz(Duration::from_secs(30)));
        let mut rig = watch_changes(builder, dir.path());
        rig.built("late.log").await;

        Mutation::Append.apply(&file, 7);
        let change = rig.changed_to("late.log", 6).await;
        assert_eq!(change.origin(), Origin::Scan);
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

        let (built_tx, mut built_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, mut drop_rx) = unbounded_channel::<PathBuf>();
        let _watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_secs(30)))
            .watch(dir.path(), move |ctx| {
                let path = ctx.path().to_owned();
                built_tx.send(path.clone()).ok();
                Some(Probe {
                    path,
                    dropped: drop_tx.clone(),
                })
            })
            .unwrap();

        if pre_exists {
            tokio::time::timeout(Duration::from_secs(5), built_rx.recv())
                .await
                .expect("startup scan builds the handler")
                .unwrap();
            std::fs::remove_file(&file).unwrap();
            let dropped = tokio::time::timeout(Duration::from_secs(5), drop_rx.recv())
                .await
                .expect("periodic scan prunes the handler without any event")
                .unwrap();
            assert!(dropped.ends_with("s.log"));
        } else {
            std::fs::write(&file, b"x").unwrap();
            let built = tokio::time::timeout(Duration::from_secs(5), built_rx.recv())
                .await
                .expect("periodic scan builds the handler without any event")
                .unwrap();
            assert!(built.ends_with("s.log"));
        }
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rename_moves_handler() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.log");
        let to = dir.path().join("to.log");
        std::fs::write(&from, b"x").unwrap();

        let (built_tx, mut built_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, mut drop_rx) = unbounded_channel::<PathBuf>();
        let _watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path(), move |ctx| {
                let path = ctx.path().to_owned();
                built_tx.send(path.clone()).ok();
                Some(Probe {
                    path,
                    dropped: drop_tx.clone(),
                })
            })
            .unwrap();

        let first = tokio::time::timeout(Duration::from_secs(5), built_rx.recv())
            .await
            .expect("initial handler for the source")
            .unwrap();
        assert!(first.ends_with("from.log"));

        std::fs::rename(&from, &to).unwrap();

        let dropped = tokio::time::timeout(Duration::from_secs(5), drop_rx.recv())
            .await
            .expect("source handler is dropped")
            .unwrap();
        assert!(dropped.ends_with("from.log"));

        // The destination may surface as a rename event or via the reconciling scan, and the
        // two channels carry no ordering guarantee, so read builds until the new path shows.
        let built = loop {
            let p = tokio::time::timeout(Duration::from_secs(5), built_rx.recv())
                .await
                .expect("destination handler is built")
                .unwrap();
            if p.ends_with("to.log") {
                break p;
            }
        };
        assert!(built.ends_with("to.log"));
    }

    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum NodeSpec {
        File,
        Dir,
        Symlink,
    }

    #[cfg(unix)]
    impl NodeSpec {
        fn create(self, at: &Path) {
            match self {
                Self::File => std::fs::write(at, b"x").unwrap(),
                Self::Dir => std::fs::create_dir(at).unwrap(),
                // Dangling on purpose: the kind is read without following the link.
                Self::Symlink => std::os::unix::fs::symlink("missing-target", at).unwrap(),
            }
        }
    }

    // The kind the factory sees must match what is on disk, all the way through the scan
    // and stat pipeline -- symlinks especially must not be followed to their target's kind.
    #[cfg(unix)]
    #[rstest]
    #[case::file(NodeSpec::File, FileKind::File)]
    #[case::dir(NodeSpec::Dir, FileKind::Dir)]
    #[case::symlink(NodeSpec::Symlink, FileKind::Symlink)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reported_kind_matches_disk(#[case] spec: NodeSpec, #[case] want: FileKind) {
        let dir = tempfile::tempdir().unwrap();
        let (built_tx, mut built_rx) = unbounded_channel::<(PathBuf, FileKind)>();
        let _watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(100)))
            .debounce_timeout(nz(Duration::from_millis(50)))
            .watch(dir.path(), move |ctx| {
                built_tx.send((ctx.path().to_owned(), ctx.kind())).ok();
                Some(())
            })
            .unwrap();

        spec.create(&dir.path().join("node"));

        let kind = loop {
            let (path, kind) = tokio::time::timeout(Duration::from_secs(5), built_rx.recv())
                .await
                .expect("node surfaces to the factory")
                .unwrap();
            if path.ends_with("node") {
                break kind;
            }
        };
        assert_eq!(kind, want);
    }
}
