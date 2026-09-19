//! Track a caller-chosen handler for every file, directory, and symlink under a directory, kept in
//! sync with the filesystem.
//!
//! A [`TreeWatcher`] walks a `root` directory and, for each node it finds, calls a factory closure
//! with a [`NodeContext`]. The factory must return `Some(handler)` to track the node, or `None` to
//! decline tracking it.
//!
//! A tracked node's handler is stored and kept alive for as long as the node exists; a decline is
//! remembered, so the factory is never asked about the same path twice while it stays unchanged.
//!
//! The watcher runs on a background Tokio task (so it must be created from within a Tokio runtime)
//! and keeps running until it is dropped, which stops watching and drops every handler. Use
//! [`recursive`](TreeWatcherBuilder::recursive) to control whether subdirectories are descended.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use std::sync::Arc;
//!
//! use atuin_common::fs::tree_watcher::{NodeContext, TreeWatcher};
//!
//! // One handler per watched file: building it means the file appeared, dropping
//! // it means the file went away.
//! struct WatchedFile(Arc<Path>);
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
//! //   append to app.log  -> nothing (already tracked, still a file)
//! //   rm app.log         -> WatchedFile drops   -> "gone:     /var/log/app.log"
//! //   mv a.log b.log     -> "gone: …/a.log" then "appeared: …/b.log"
//! //   drop(watcher)      -> every WatchedFile drops
//! # let _ = watcher;
//! # Ok(())
//! # }
//! ```
//!
//! TODO(markovejnovic): Instead of the arbitrary factory, it would be good perhaps for the factory
//!                      to have to return some sort of handler, which enables us to listen to more
//!                      fs events, such as mutations, I suppose.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

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

/// How a node came to the watcher's attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Surfaced by a filesystem event.
    Event,
    /// Surfaced by the periodic reconciling scan.
    Scan,
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

/// Walk `root` best-effort, returning every readable entry plus the set of
/// directories that were fully read without error. A node may be pruned only when its
/// parent directory is in that set: a node missing from a directory we could not
/// fully read may be unreadable rather than gone, so unread subtrees are left intact
/// while readable ones reconcile independently.
fn scan_fs(root: &Path, recursive: bool) -> (Vec<(Arc<Path>, FileKind)>, HashSet<Arc<Path>>) {
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
            let path: Arc<Path> = Arc::from(entry.path());
            if recursive && file_type.is_dir() {
                stack.push(Arc::clone(&path));
            }
            out.push((path, FileKind::from(file_type)));
        }
        // Only a fully enumerated directory lets us trust the absence of its children.
        if dir_complete {
            scanned.insert(dir);
        }
    }
    (out, scanned)
}

async fn scan(root: &Path, recursive: bool) -> (Vec<(Arc<Path>, FileKind)>, HashSet<Arc<Path>>) {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || scan_fs(&root, recursive))
        .await
        .unwrap_or_else(|_| (Vec::new(), HashSet::new()))
}

/// Stat every path referenced by `events` off the async executor, keeping only
/// the ones that still exist.
async fn resolve_kinds(events: &[notify::Event]) -> HashMap<Arc<Path>, FileKind> {
    let paths: Vec<Arc<Path>> = events
        .iter()
        .flat_map(|event| event.paths.iter().map(|p| Arc::from(p.as_path())))
        .collect();
    let stat = tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .filter_map(|path| {
                let kind = FileKind::from(std::fs::symlink_metadata(&path).ok()?.file_type());
                Some((Arc::clone(&path), kind))
            })
            .collect()
    });
    stat.await.unwrap_or_default()
}

enum Slot<H> {
    Active(FileKind, H),
    Declined(FileKind),
}

impl<H> Slot<H> {
    fn kind(&self) -> FileKind {
        match self {
            Self::Active(kind, _) | Self::Declined(kind) => *kind,
        }
    }
}

struct Engine<H, F> {
    factory: F,
    entries: HashMap<Arc<Path>, Slot<H>>,
}

impl<H, F> Engine<H, F>
where
    H: Send + 'static,
    F: Fn(NodeContext) -> Option<H>,
{
    fn observe(&mut self, path: Arc<Path>, kind: FileKind, origin: Origin) {
        if self.entries.get(&path).is_some_and(|slot| slot.kind() == kind) {
            return;
        }

        self.entries.remove(&path);
        let ctx = NodeContext {
            path: Arc::clone(&path),
            kind,
            origin,
        };
        let slot =
            (self.factory)(ctx).map_or(Slot::Declined(kind), |handler| Slot::Active(kind, handler));
        self.entries.insert(path, slot);
    }

    fn forget_tree(&mut self, path: &Path) {
        self.entries.retain(|key, _| !key.starts_with(path));
    }

    fn reconcile(&mut self, truth: Vec<(Arc<Path>, FileKind)>, scanned_dirs: &HashSet<Arc<Path>>) {
        let truth: HashMap<Arc<Path>, FileKind> = truth.into_iter().collect();
        // Prune a tracked node only when its parent directory was fully read this pass,
        // so nodes under an unreadable subtree survive while readable directories
        // reconcile independently.
        self.entries.retain(|key, _| {
            truth.contains_key(key)
                || key.parent().is_none_or(|parent| !scanned_dirs.contains(parent))
        });
        for (path, kind) in truth {
            self.observe(path, kind, Origin::Scan);
        }
    }

    fn observe_path(&mut self, path: &Path, kinds: &HashMap<Arc<Path>, FileKind>, origin: Origin) {
        let Some((key, &kind)) = kinds.get_key_value(path) else {
            return;
        };
        if self.entries.get(path).is_some_and(|slot| slot.kind() == kind) {
            return;
        }
        self.observe(Arc::clone(key), kind, origin);
    }

    fn apply_event(&mut self, event: &notify::Event, kinds: &HashMap<Arc<Path>, FileKind>) {
        match &event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                for path in &event.paths {
                    self.observe_path(path, kinds, Origin::Event);
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
/// full scan and applies debounced filesystem events as they arrive.
struct TreeWatcherPoller<H, F> {
    root: PathBuf,
    recursive: bool,
    engine: Engine<H, F>,
}

impl<H, F> TreeWatcherPoller<H, F>
where
    H: Send + 'static,
    F: Fn(NodeContext) -> Option<H> + Send + 'static,
{
    async fn run(
        mut self,
        events: flume::Receiver<DebounceEventResult>,
        scan_interval: NonZeroDuration,
    ) {
        self.rescan().await;

        let mut interval = tokio::time::interval(scan_interval.get());
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => self.rescan().await,
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
}

const DEFAULT_SCAN_INTERVAL: NonZeroDuration =
    NonZeroDuration::from_secs(NonZeroU64::new(30).unwrap());
const DEFAULT_DEBOUNCE_TIMEOUT: NonZeroDuration =
    NonZeroDuration::new(Duration::from_millis(250)).unwrap();

/// Builder for a [`TreeWatcher`].
pub struct TreeWatcherBuilder {
    recursive: bool,
    scan_interval: NonZeroDuration,
    debounce_timeout: NonZeroDuration,
}

impl Default for TreeWatcherBuilder {
    fn default() -> Self {
        Self {
            recursive: true,
            scan_interval: DEFAULT_SCAN_INTERVAL,
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
        H: Send + 'static,
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

        let poller = TreeWatcherPoller {
            root,
            recursive: self.recursive,
            engine: Engine {
                factory,
                entries: HashMap::new(),
            },
        };
        let task = tokio::spawn(poller.run(rx, self.scan_interval));

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
        H: Send + 'static,
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

    use proptest::prelude::*;
    use rstest::rstest;
    use tokio::sync::mpsc::error::TryRecvError;
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

    use super::*;

    #[derive(Default)]
    struct Counters {
        alive: AtomicI64,
        created: AtomicU64,
        dropped: AtomicU64,
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

    fn kinds_map<const N: usize>(entries: [(&str, FileKind); N]) -> HashMap<Arc<Path>, FileKind> {
        entries.into_iter().map(|(p, k)| (ap(p), k)).collect()
    }

    fn truth(ids: &[&str]) -> Vec<(Arc<Path>, FileKind)> {
        ids.iter().map(|id| (ap(&format!("/r/{id}")), FileKind::File)).collect()
    }

    fn keys(
        engine: &Engine<CountingHandler, impl Fn(NodeContext) -> Option<CountingHandler>>,
    ) -> std::collections::HashSet<Arc<Path>> {
        engine.entries.keys().cloned().collect()
    }

    fn scanned_kinds(
        entries: &[(Arc<Path>, FileKind)],
        root: &Path,
    ) -> std::collections::BTreeMap<String, FileKind> {
        entries
            .iter()
            .map(|(p, k)| (p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), *k))
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
        engine.observe(ap("/r/a"), FileKind::File, Origin::Event);
        engine.observe(ap("/r/a"), FileKind::File, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 1);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn observe_rebuilds_on_kind_change() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/x"), FileKind::Dir, Origin::Scan);
        engine.observe(ap("/r/x"), FileKind::File, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 2);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn declined_paths_are_recorded_and_not_re_offered() {
        let counters = Arc::new(Counters::default());
        let c = counters.clone();
        let mut engine = Engine {
            factory: move |ctx: NodeContext| ctx.is_file().then(|| CountingHandler::new(c.clone())),
            entries: HashMap::new(),
        };
        engine.observe(ap("/r/d"), FileKind::Dir, Origin::Scan);
        engine.observe(ap("/r/d"), FileKind::Dir, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 0);
        assert!(matches!(engine.entries.get(Path::new("/r/d")), Some(Slot::Declined(_))));
    }

    #[rstest]
    fn forget_drops_handler() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/a"), FileKind::File, Origin::Event);
        engine.entries.remove(Path::new("/r/a"));
        assert!(engine.entries.is_empty());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 0);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn forget_tree_drops_subtree() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/sub"), FileKind::Dir, Origin::Scan);
        engine.observe(ap("/r/sub/a"), FileKind::File, Origin::Scan);
        engine.observe(ap("/r/sub2/b"), FileKind::File, Origin::Scan);
        engine.forget_tree(Path::new("/r/sub"));
        assert_eq!(keys(&engine), std::iter::once(ap("/r/sub2/b")).collect());
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
        engine.reconcile(vec![(ap("/r/x"), FileKind::Dir)], &scanned(&["/r"]));
        engine.reconcile(vec![(ap("/r/x"), FileKind::File)], &scanned(&["/r"]));
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
    fn reconcile_prunes_only_under_scanned_dirs() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(ap("/r/gone"), FileKind::File, Origin::Scan);
        engine.observe(ap("/r/sub/kept"), FileKind::File, Origin::Scan);
        // The scan read /r but not /r/sub (unreadable subtree); truth is empty.
        engine.reconcile(vec![], &scanned(&["/r"]));
        // /r/gone: parent /r was scanned and it is absent -> pruned.
        // /r/sub/kept: parent /r/sub was not scanned -> retained.
        assert_eq!(keys(&engine), std::iter::once(ap("/r/sub/kept")).collect());
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
        engine.observe(ap("/r/a"), FileKind::File, Origin::Event);
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
        engine.observe(ap("/r/from"), FileKind::File, Origin::Event);
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
        engine.observe(ap("/r/a"), FileKind::File, Origin::Event);
        let ev = event(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), vec![PathBuf::from(
            "/r/a",
        )]);
        engine.apply_event(&ev, &HashMap::new());
        assert!(engine.entries.is_empty());
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
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
                let truth: Vec<(Arc<Path>, FileKind)> =
                    want.iter().cloned().map(|p| (p, FileKind::File)).collect();
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
                entries: HashMap::new(),
            };
            for state in &states {
                let truth: Vec<(Arc<Path>, FileKind)> = state
                    .iter()
                    .map(|id| {
                        let kind = if id % 2 == 0 { FileKind::File } else { FileKind::Dir };
                        (ap(&format!("/r/{id}")), kind)
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

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.dropped.send(self.path.clone());
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

    // A content write is neither a create/remove nor a kind change, so the handler must
    // survive untouched: not rebuilt by the event path, not churned by the periodic scan.
    #[rstest]
    #[case::append(Mutation::Append)]
    #[case::overwrite(Mutation::Overwrite)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn benign_mutation_does_not_rebuild_handler(#[case] mutation: Mutation) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("keep.log");
        std::fs::write(&file, b"seed").unwrap();

        let (built_tx, mut built_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, mut drop_rx) = unbounded_channel::<PathBuf>();
        let _watcher = TreeWatcher::builder()
            .scan_interval(nz(Duration::from_millis(50)))
            .debounce_timeout(nz(Duration::from_millis(20)))
            .watch(dir.path(), move |ctx| {
                let path = ctx.path().to_owned();
                built_tx.send(path.clone()).ok();
                Some(Probe {
                    path,
                    dropped: drop_tx.clone(),
                })
            })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), built_rx.recv())
            .await
            .expect("startup scan builds the handler")
            .unwrap();
        while built_rx.try_recv().is_ok() {}

        for i in 0..3 {
            mutation.apply(&file, i);
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        // Several scan cycles and debounce windows: a bug that rebuilds on modify surfaces here.
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(matches!(built_rx.try_recv(), Err(TryRecvError::Empty)), "handler was rebuilt");
        assert!(matches!(drop_rx.try_recv(), Err(TryRecvError::Empty)), "handler was dropped");
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
