use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

#[derive(Debug, thiserror::Error)]
pub enum TreeWatcherError {
    #[error("watch root is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error(transparent)]
    Notify(#[from] notify::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Notify,
    Scan,
}

#[derive(Debug, Clone)]
pub struct NodeContext {
    path: PathBuf,
    kind: FileKind,
    origin: Origin,
}

impl NodeContext {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    #[must_use]
    pub fn is_file(&self) -> bool {
        matches!(self.kind, FileKind::File)
    }

    #[must_use]
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, FileKind::Dir)
    }

    #[must_use]
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, FileKind::Symlink)
    }

    #[must_use]
    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

fn scan_fs(root: &Path, recursive: bool) -> std::io::Result<Vec<(PathBuf, FileKind)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if recursive && file_type.is_dir() {
                stack.push(path.clone());
            }
            out.push((path, FileKind::from(file_type)));
        }
    }
    Ok(out)
}

async fn scan(root: &Path, recursive: bool) -> Option<Vec<(PathBuf, FileKind)>> {
    let root = root.to_path_buf();
    match tokio::task::spawn_blocking(move || scan_fs(&root, recursive)).await {
        Ok(Ok(entries)) => Some(entries),
        _ => None,
    }
}

pub trait TreeNode: Send + 'static {}

impl<T: Send + 'static> TreeNode for T {}

enum Slot<H> {
    Active(H),
    Declined,
}

struct Engine<H, F> {
    root: PathBuf,
    recursive: bool,
    factory: F,
    entries: HashMap<PathBuf, Slot<H>>,
}

impl<H, F> Engine<H, F>
where
    H: TreeNode,
    F: Fn(NodeContext) -> Option<H>,
{
    fn observe(&mut self, path: PathBuf, kind: FileKind, origin: Origin) {
        if self.entries.contains_key(&path) {
            return;
        }
        let ctx = NodeContext {
            path: path.clone(),
            kind,
            origin,
        };
        let slot = match (self.factory)(ctx) {
            Some(handler) => Slot::Active(handler),
            None => Slot::Declined,
        };
        self.entries.insert(path, slot);
    }

    fn forget(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    fn forget_tree(&mut self, path: &Path) {
        let victims: Vec<PathBuf> =
            self.entries.keys().filter(|key| key.starts_with(path)).cloned().collect();
        for victim in victims {
            self.entries.remove(&victim);
        }
    }

    fn reconcile(&mut self, truth: Vec<(PathBuf, FileKind)>) {
        let truth: HashMap<PathBuf, FileKind> = truth.into_iter().collect();
        let gone: Vec<PathBuf> =
            self.entries.keys().filter(|key| !truth.contains_key(*key)).cloned().collect();
        for path in &gone {
            self.forget(path);
        }
        for (path, kind) in truth {
            if !self.entries.contains_key(&path) {
                self.observe(path, kind, Origin::Scan);
            }
        }
    }

    fn observe_path(&mut self, path: PathBuf, origin: Origin) {
        if self.entries.contains_key(&path) {
            return;
        }
        let Ok(kind) = file_kind_of(&path) else {
            return;
        };
        self.observe(path, kind, origin);
    }

    fn apply_event(&mut self, event: &notify::Event) {
        match &event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                for path in &event.paths {
                    self.observe_path(path.clone(), Origin::Notify);
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
                    self.observe_path(to.clone(), Origin::Notify);
                }
            }
            _ => {}
        }
    }
}

fn file_kind_of(path: &Path) -> std::io::Result<FileKind> {
    Ok(FileKind::from(std::fs::symlink_metadata(path)?.file_type()))
}

const DEFAULT_SCAN_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_DEBOUNCE_TIMEOUT: Duration = Duration::from_millis(250);

async fn run_engine<H, F>(
    mut engine: Engine<H, F>,
    mut events: UnboundedReceiver<DebounceEventResult>,
    scan_interval: Duration,
) where
    H: TreeNode,
    F: Fn(NodeContext) -> Option<H> + Send + 'static,
{
    if let Some(truth) = scan(&engine.root, engine.recursive).await {
        engine.reconcile(truth);
    }

    let mut interval = tokio::time::interval(scan_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Some(truth) = scan(&engine.root, engine.recursive).await {
                    engine.reconcile(truth);
                }
            }
            received = events.recv() => {
                match received {
                    Some(Ok(batch)) => {
                        let mut force_scan = false;
                        for debounced in batch {
                            if debounced.need_rescan() {
                                force_scan = true;
                                continue;
                            }
                            engine.apply_event(&debounced.event);
                        }
                        if force_scan
                            && let Some(truth) = scan(&engine.root, engine.recursive).await
                        {
                            engine.reconcile(truth);
                        }
                    }
                    Some(Err(_)) => {}
                    None => break,
                }
            }
        }
    }
}

pub struct TreeWatcherBuilder<H> {
    recursive: bool,
    scan_interval: Duration,
    debounce_timeout: Duration,
    _marker: PhantomData<fn() -> H>,
}

impl<H: TreeNode> Default for TreeWatcherBuilder<H> {
    fn default() -> Self {
        Self {
            recursive: true,
            scan_interval: DEFAULT_SCAN_INTERVAL,
            debounce_timeout: DEFAULT_DEBOUNCE_TIMEOUT,
            _marker: PhantomData,
        }
    }
}

impl<H: TreeNode> TreeWatcherBuilder<H> {
    #[must_use]
    pub fn recursive(mut self, yes: bool) -> Self {
        self.recursive = yes;
        self
    }

    #[must_use]
    pub fn scan_interval(mut self, interval: Duration) -> Self {
        self.scan_interval = interval;
        self
    }

    #[must_use]
    pub fn debounce_timeout(mut self, timeout: Duration) -> Self {
        self.debounce_timeout = timeout;
        self
    }

    pub fn watch<F>(
        self,
        root: impl AsRef<Path>,
        factory: F,
    ) -> Result<TreeWatcher<H>, TreeWatcherError>
    where
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
        let task = tokio::spawn(run_engine(engine, rx, self.scan_interval));

        Ok(TreeWatcher {
            task,
            _debouncer: debouncer,
            _marker: PhantomData,
        })
    }
}

pub struct TreeWatcher<H> {
    task: JoinHandle<()>,
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    _marker: PhantomData<fn() -> H>,
}

impl<H: TreeNode> TreeWatcher<H> {
    #[must_use]
    pub fn builder() -> TreeWatcherBuilder<H> {
        TreeWatcherBuilder::default()
    }

    pub fn watch<F>(root: impl AsRef<Path>, factory: F) -> Result<Self, TreeWatcherError>
    where
        F: Fn(NodeContext) -> Option<H> + Send + 'static,
    {
        Self::builder().watch(root, factory)
    }
}

impl<H> Drop for TreeWatcher<H> {
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
        let counters = counters.clone();
        Engine {
            root: PathBuf::from("/r"),
            recursive: true,
            factory: move |_ctx| Some(CountingHandler::new(counters.clone())),
            entries: HashMap::new(),
        }
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

    fn event(kind: EventKind, paths: Vec<PathBuf>) -> notify::Event {
        notify::Event {
            kind,
            paths,
            attrs: notify::event::EventAttributes::new(),
        }
    }

    fn truth(ids: &[&str]) -> Vec<(PathBuf, FileKind)> {
        ids.iter().map(|id| (PathBuf::from(format!("/r/{id}")), FileKind::File)).collect()
    }

    fn kinds(
        entries: &[(PathBuf, FileKind)],
        root: &Path,
    ) -> std::collections::BTreeMap<String, FileKind> {
        entries
            .iter()
            .map(|(p, k)| (p.strip_prefix(root).unwrap().to_string_lossy().into_owned(), *k))
            .collect()
    }

    #[test]
    fn not_a_directory_displays_path() {
        let err = TreeWatcherError::NotADirectory(PathBuf::from("/nope"));
        assert_eq!(err.to_string(), "watch root is not a directory: /nope");
    }

    #[test]
    fn file_kind_from_file_type() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        let kind = FileKind::from(std::fs::symlink_metadata(&file).unwrap().file_type());
        assert_eq!(kind, FileKind::File);

        let kind = FileKind::from(std::fs::symlink_metadata(dir.path()).unwrap().file_type());
        assert_eq!(kind, FileKind::Dir);
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
            path: PathBuf::from("/root/x"),
            kind,
            origin: Origin::Scan,
        };
        assert_eq!(ctx.is_file(), is_file);
        assert_eq!(ctx.is_dir(), is_dir);
        assert_eq!(ctx.is_symlink(), is_symlink);
        assert_eq!(ctx.path(), Path::new("/root/x"));
        assert_eq!(ctx.origin(), Origin::Scan);
    }

    #[test]
    fn scan_fs_lists_direct_children_only_when_not_recursive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), b"x").unwrap();

        let found = scan_fs(dir.path(), false).unwrap();
        let map = kinds(&found, dir.path());
        assert_eq!(map.get("a"), Some(&FileKind::File));
        assert_eq!(map.get("sub"), Some(&FileKind::Dir));
        assert!(!map.contains_key("sub/b"));
    }

    #[test]
    fn scan_fs_descends_when_recursive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), b"x").unwrap();

        let found = scan_fs(dir.path(), true).unwrap();
        let map = kinds(&found, dir.path());
        assert_eq!(map.get("sub/b"), Some(&FileKind::File));
    }

    #[cfg(unix)]
    #[test]
    fn scan_fs_does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/inner"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();

        let found = scan_fs(dir.path(), true).unwrap();
        let map = kinds(&found, dir.path());
        assert_eq!(map.get("link"), Some(&FileKind::Symlink));
        assert!(!map.contains_key("link/inner"));
        assert_eq!(map.get("real/inner"), Some(&FileKind::File));
    }

    #[tokio::test]
    async fn scan_returns_none_for_missing_root() {
        let missing = PathBuf::from("/this/does/not/exist/anywhere");
        assert!(scan(&missing, true).await.is_none());
    }

    #[test]
    fn observe_creates_one_handler_and_dedupes() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(PathBuf::from("/r/a"), FileKind::File, Origin::Notify);
        engine.observe(PathBuf::from("/r/a"), FileKind::File, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 1);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn declined_paths_are_recorded_and_not_re_offered() {
        let counters = Arc::new(Counters::default());
        let c = counters.clone();
        let mut engine = Engine {
            root: PathBuf::from("/r"),
            recursive: true,
            factory: move |ctx: NodeContext| ctx.is_file().then(|| CountingHandler::new(c.clone())),
            entries: HashMap::new(),
        };
        engine.observe(PathBuf::from("/r/d"), FileKind::Dir, Origin::Scan);
        engine.observe(PathBuf::from("/r/d"), FileKind::Dir, Origin::Scan);
        assert_eq!(engine.entries.len(), 1);
        assert_eq!(counters.created.load(Ordering::SeqCst), 0);
        assert!(matches!(engine.entries.get(Path::new("/r/d")), Some(Slot::Declined)));
    }

    #[test]
    fn forget_drops_handler() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(PathBuf::from("/r/a"), FileKind::File, Origin::Notify);
        engine.forget(Path::new("/r/a"));
        assert!(engine.entries.is_empty());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 0);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn forget_tree_drops_subtree() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.observe(PathBuf::from("/r/sub"), FileKind::Dir, Origin::Scan);
        engine.observe(PathBuf::from("/r/sub/a"), FileKind::File, Origin::Scan);
        engine.observe(PathBuf::from("/r/sub2/b"), FileKind::File, Origin::Scan);
        engine.forget_tree(Path::new("/r/sub"));
        let keys: std::collections::HashSet<PathBuf> = engine.entries.keys().cloned().collect();
        assert_eq!(keys, std::iter::once(PathBuf::from("/r/sub2/b")).collect());
    }

    #[test]
    fn reconcile_adds_and_removes() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.reconcile(truth(&["a", "b"]));
        assert_eq!(counters.alive.load(Ordering::SeqCst), 2);
        engine.reconcile(truth(&["b", "c"]));
        let keys: std::collections::HashSet<PathBuf> = engine.entries.keys().cloned().collect();
        assert_eq!(keys, [PathBuf::from("/r/b"), PathBuf::from("/r/c")].into_iter().collect());
        assert_eq!(counters.alive.load(Ordering::SeqCst), 2);
        assert_eq!(counters.created.load(Ordering::SeqCst), 3);
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_engine_drops_all_handlers() {
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine(&counters);
        engine.reconcile(truth(&["a", "b", "c"]));
        assert_eq!(counters.alive.load(Ordering::SeqCst), 3);
        drop(engine);
        assert_eq!(counters.alive.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn apply_create_event_observes_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a");
        std::fs::write(&file, b"x").unwrap();
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine_rooted(&counters, dir.path());
        engine.apply_event(&event(EventKind::Create(notify::event::CreateKind::Any), vec![
            file.clone(),
        ]));
        assert!(engine.entries.contains_key(&file));
        assert_eq!(counters.alive.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn apply_create_event_skips_vanished_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gone");
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine_rooted(&counters, dir.path());
        engine.apply_event(&event(EventKind::Create(notify::event::CreateKind::Any), vec![file]));
        assert!(engine.entries.is_empty());
    }

    #[test]
    fn apply_remove_event_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a");
        std::fs::write(&file, b"x").unwrap();
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine_rooted(&counters, dir.path());
        engine.observe(file.clone(), FileKind::File, Origin::Notify);
        engine.apply_event(&event(EventKind::Remove(notify::event::RemoveKind::Any), vec![file]));
        assert!(engine.entries.is_empty());
        assert_eq!(counters.dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn apply_rename_both_moves_handler() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::write(&to, b"x").unwrap();
        let counters = Arc::new(Counters::default());
        let mut engine = accept_all_engine_rooted(&counters, dir.path());
        engine.observe(from.clone(), FileKind::File, Origin::Notify);
        engine.apply_event(&event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), vec![
            from.clone(),
            to.clone(),
        ]));
        assert!(!engine.entries.contains_key(&from));
        assert!(engine.entries.contains_key(&to));
    }

    proptest! {
        #[test]
        fn reconcile_entries_always_equal_truth(
            states in prop::collection::vec(prop::collection::hash_set(0u16..24, 0..24), 1..24)
        ) {
            let counters = Arc::new(Counters::default());
            let mut engine = accept_all_engine(&counters);
            for state in &states {
                let want: std::collections::HashSet<PathBuf> = state
                    .iter()
                    .map(|id| PathBuf::from(format!("/r/{id}")))
                    .collect();
                let truth: Vec<(PathBuf, FileKind)> =
                    want.iter().cloned().map(|p| (p, FileKind::File)).collect();
                engine.reconcile(truth);
                let keys: std::collections::HashSet<PathBuf> =
                    engine.entries.keys().cloned().collect();
                prop_assert_eq!(keys, want);
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
                let truth: Vec<(PathBuf, FileKind)> = state
                    .iter()
                    .map(|id| {
                        let kind = if id % 2 == 0 { FileKind::File } else { FileKind::Dir };
                        (PathBuf::from(format!("/r/{id}")), kind)
                    })
                    .collect();
                engine.reconcile(truth);
            }
            let alive = usize::try_from(counters.alive.load(Ordering::SeqCst)).unwrap();
            let active = engine
                .entries
                .values()
                .filter(|slot| matches!(slot, Slot::Active(_)))
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
        let _watcher = TreeWatcher::<Probe>::builder()
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
        let _watcher = TreeWatcher::<Probe>::builder()
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
    #[case(true, true)]
    #[case(false, false)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recursion_controls_nested_files(#[case] recursive: bool, #[case] expect_fire: bool) {
        let dir = tempfile::tempdir().unwrap();
        let (created_tx, mut created_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, _drop_rx) = unbounded_channel::<PathBuf>();
        let _watcher = TreeWatcher::<Probe>::builder()
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
            tokio::time::timeout(Duration::from_millis(1500), created_rx.recv()).await.is_ok();
        assert_eq!(fired, expect_fire);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_watcher_drops_handlers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.log"), b"x").unwrap();
        let (created_tx, mut created_rx) = unbounded_channel::<PathBuf>();
        let (drop_tx, mut drop_rx) = unbounded_channel::<PathBuf>();
        let watcher = TreeWatcher::<Probe>::builder()
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
