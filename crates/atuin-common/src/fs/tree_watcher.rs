//! Watch a directory tree, holding one factory-built handler per live node.
//!
//! [`TreeWatcher`] offers every file, directory, and symlink under a root to a
//! factory closure; whatever the factory returns is kept alive until the node
//! disappears. Filesystem events drive updates in near real time, and a periodic
//! full scan reconciles anything the event stream missed.
//!
//! ```no_run
//! use atuin_common::fs::tree_watcher::{NodeContext, TreeWatcher};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Keep one handler per regular file; here the handler is just the file's path.
//! let watcher = TreeWatcher::watch("/var/log", |ctx: NodeContext| {
//!     ctx.is_file().then(|| ctx.into_path())
//! })?;
//! // Dropping `watcher` stops watching and drops every handler.
//! # let _ = watcher;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

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

fn scan_fs(root: &Path, recursive: bool) -> std::io::Result<Vec<(Arc<Path>, FileKind)>> {
    let mut out = Vec::new();
    let mut stack: Vec<Arc<Path>> = vec![Arc::from(root)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path: Arc<Path> = Arc::from(entry.path());
            if recursive && file_type.is_dir() {
                stack.push(Arc::clone(&path));
            }
            out.push((path, FileKind::from(file_type)));
        }
    }
    Ok(out)
}

async fn scan(root: &Path, recursive: bool) -> std::io::Result<Vec<(Arc<Path>, FileKind)>> {
    let root = root.to_path_buf();
    match tokio::task::spawn_blocking(move || scan_fs(&root, recursive)).await {
        Ok(result) => result,
        Err(join) => Err(std::io::Error::other(join)),
    }
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
            .filter_map(|path| Some((Arc::clone(&path), file_kind_of(&path).ok()?)))
            .collect()
    });
    stat.await.unwrap_or_default()
}

fn file_kind_of(path: &Path) -> std::io::Result<FileKind> {
    Ok(FileKind::from(std::fs::symlink_metadata(path)?.file_type()))
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
    root: PathBuf,
    recursive: bool,
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
        // Absent, or present at a stale kind: (re)build, dropping any old handler.
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

    fn reconcile(&mut self, truth: Vec<(Arc<Path>, FileKind)>) {
        let truth: HashMap<Arc<Path>, FileKind> = truth.into_iter().collect();
        self.entries.retain(|key, _| truth.contains_key(key));
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

impl<H, F> Engine<H, F>
where
    H: Send + 'static,
    F: Fn(NodeContext) -> Option<H> + Send + 'static,
{
    async fn run(
        mut self,
        mut events: UnboundedReceiver<DebounceEventResult>,
        scan_interval: Duration,
    ) {
        self.rescan().await;

        let mut interval = tokio::time::interval(scan_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => self.rescan().await,
                received = events.recv() => {
                    match received {
                        Some(Ok(batch)) => {
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
                                    self.apply_event(event, &kinds);
                                }
                            }
                            if force_scan {
                                self.rescan().await;
                            }
                        }
                        Some(Err(errors)) => tracing::warn!(?errors, "tree watcher backend error"),
                        None => break,
                    }
                }
            }
        }
    }

    async fn rescan(&mut self) {
        match scan(&self.root, self.recursive).await {
            Ok(truth) => self.reconcile(truth),
            Err(err) => tracing::warn!(?err, "tree watcher scan failed; skipping reconcile"),
        }
    }
}

const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(250);

/// Builder for a [`TreeWatcher`].
pub struct TreeWatcherBuilder {
    recursive: bool,
    scan_interval: Duration,
    debounce_timeout: Duration,
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
    pub fn scan_interval(mut self, interval: Duration) -> Self {
        self.scan_interval = interval;
        self
    }

    /// Window for coalescing filesystem events (default: 250ms).
    #[must_use]
    pub fn debounce_timeout(mut self, timeout: Duration) -> Self {
        self.debounce_timeout = timeout;
        self
    }

    /// Start watching `root`, building a handler for each node the factory accepts.
    ///
    /// A factory returning `None` declines the node; the decision is remembered.
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

        let (tx, rx) = unbounded_channel();
        let mode = if self.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        let mut debouncer =
            new_debouncer(self.debounce_timeout, None, move |result: DebounceEventResult| {
                let _ = tx.send(result);
            })?;
        debouncer.watch(&root, mode)?;

        let engine = Engine {
            root,
            recursive: self.recursive,
            factory,
            entries: HashMap::new(),
        };
        let task = tokio::spawn(engine.run(rx, self.scan_interval));

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
        accept_all_engine_rooted(counters, Path::new("/r"))
    }

    fn accept_all_engine_rooted(
        counters: &Arc<Counters>,
        root: &Path,
    ) -> Engine<CountingHandler, impl Fn(NodeContext) -> Option<CountingHandler>> {
        let counters = counters.clone();
        Engine {
            root: root.to_path_buf(),
            recursive: true,
            factory: move |_ctx| Some(CountingHandler::new(counters.clone())),
            entries: HashMap::new(),
        }
    }

    fn ap(path: &str) -> Arc<Path> {
        Arc::from(Path::new(path))
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

        let found = scan_fs(dir.path(), recursive).unwrap();
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

        let found = scan_fs(dir.path(), true).unwrap();
        let map = scanned_kinds(&found, dir.path());
        assert_eq!(map.get("link"), Some(&FileKind::Symlink));
        assert!(!map.contains_key("link/inner"));
        assert_eq!(map.get("real/inner"), Some(&FileKind::File));
    }

    #[tokio::test]
    async fn scan_errors_for_missing_root() {
        let missing = Path::new("/this/does/not/exist/anywhere");
        assert!(scan(missing, true).await.is_err());
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
            root: PathBuf::from("/r"),
            recursive: true,
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
        engine.reconcile(truth(&["a", "b"]));
        assert_eq!(counters.alive.load(Ordering::SeqCst), 2);
        engine.reconcile(truth(&["b", "c"]));
        assert_eq!(keys(&engine), [ap("/r/b"), ap("/r/c")].into_iter().collect());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 2);
        assert_eq!(counters.created.load(Ordering::SeqCst), 3);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[rstest]
    fn reconcile_repairs_kind_swap() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.reconcile(vec![(ap("/r/x"), FileKind::Dir)]);
        engine.reconcile(vec![(ap("/r/x"), FileKind::File)]);
        assert_eq!(
            engine.entries.get(Path::new("/r/x")).map(|slot| slot.kind()),
            Some(FileKind::File)
        );
        assert_eq!(counters.created.load(Ordering::SeqCst), 2);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
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
                engine.reconcile(truth);
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
                root: PathBuf::from("/r"),
                recursive: true,
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
                engine.reconcile(truth);
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
            .scan_interval(Duration::from_millis(100))
            .debounce_timeout(Duration::from_millis(50))
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
            .scan_interval(Duration::from_millis(100))
            .debounce_timeout(Duration::from_millis(50))
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
            .scan_interval(Duration::from_millis(100))
            .debounce_timeout(Duration::from_millis(50))
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
            .scan_interval(Duration::from_millis(100))
            .debounce_timeout(Duration::from_millis(50))
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
}
