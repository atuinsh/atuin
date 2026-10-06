use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::watch;

use super::walk::WalkResult;
use super::{ContentMark, FileKind, FileStat, PathFingerprint, WatchedFile};

/// A slot for a file managed by this file tracker.
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
}

/// Tracker for the regular files under the watcher's root that its filter accepts.
pub struct FileTracker<F> {
    filter: F,
    entries: HashMap<Arc<Path>, Slot>,
    found: flume::Sender<WatchedFile>,
}

impl<F> FileTracker<F>
where
    F: Fn(&Path) -> bool,
{
    pub fn new(filter: F, found: flume::Sender<WatchedFile>) -> Self {
        Self {
            filter,
            entries: HashMap::new(),
            found,
        }
    }

    /// Bring the slot for `path` in line with what was just observed on disk.
    pub fn observe_fingerprint(&mut self, path: Arc<Path>, fingerprint: PathFingerprint) {
        let kind = FileKind::from(fingerprint);
        if let Some(slot) = self.entries.get_mut(&path)
            && slot.kind() == kind
        {
            // The stat is updated in place: yielding a new file would reset whatever read
            // position its consumer keeps.
            if let Slot::Tracked(known, stat) = slot
                && let PathFingerprint::File(mark) = fingerprint
                && *known != mark
            {
                *known = mark;
                stat.send_replace(FileStat { mark });
            }
            return;
        }

        // A directory that became something else takes its tracked descendants with it.
        self.forget_path(&path);

        let slot = match fingerprint {
            PathFingerprint::File(mark) if (self.filter)(&path) => {
                let (stat, rx) = watch::channel(FileStat { mark });
                // A dropped receiver means the watcher itself is going away.
                let _ = self.found.send(WatchedFile {
                    path: Arc::clone(&path),
                    stat: rx,
                });
                Slot::Tracked(mark, stat)
            }
            PathFingerprint::File(_)
            | PathFingerprint::Dir
            | PathFingerprint::Symlink
            | PathFingerprint::Other => Slot::Untracked(kind),
        };
        self.entries.insert(path, slot);
    }

    /// Given a path, forget it if it is in the entry list.
    fn forget_path(&mut self, path: &Path) {
        if self.entries.get(path).is_some_and(|slot| slot.kind() == FileKind::Dir) {
            self.forget_tree(path);
        } else {
            self.entries.remove(path);
        }
    }

    /// Whether `path` has an entry, tracked or not.
    #[cfg(test)]
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    pub fn forget_tree(&mut self, path: &Path) {
        match self.entries.get(path) {
            Some(slot) if slot.kind() != FileKind::Dir => {
                self.entries.remove(path);
            }
            _ => self.entries.retain(|key, _| !key.starts_with(path)),
        }
    }

    /// Tracked regular files written within `window` of `from`.
    pub fn written_within(
        &self,
        window: Duration,
        from: SystemTime,
    ) -> impl Iterator<Item = Arc<Path>> + '_ {
        self.entries.iter().filter_map(move |(path, slot)| match slot {
            Slot::Tracked(mark, _) if mark.written_within(window, from) => Some(Arc::clone(path)),
            Slot::Tracked(..) | Slot::Untracked(_) => None,
        })
    }

    /// Given a walk result, assume it to be the ground truth, and then update the local state to
    /// match what the walk resulted.
    ///
    /// This function is here to support the background task which walks the root directory and
    /// patches up any holes we may have missed through [`notify`].
    pub fn reconcile(&mut self, walk: WalkResult) {
        let WalkResult {
            entries,
            complete_dirs,
        } = walk;
        let truth: HashMap<Arc<Path>, PathFingerprint> = entries.into_iter().collect();

        self.entries.retain(|key, _| {
            truth.contains_key(key)
                || !key.ancestors().any(|ancestor| {
                    ancestor.parent().is_some_and(|parent| complete_dirs.contains(parent))
                        && !truth.contains_key(ancestor)
                })
        });

        for (path, fingerprint) in truth {
            self.observe_fingerprint(path, fingerprint);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;
    #[cfg(unix)]
    use crate::os::fs::FdIdentity;

    fn tracker(
        filter: impl Fn(&Path) -> bool,
    ) -> (FileTracker<impl Fn(&Path) -> bool>, flume::Receiver<WatchedFile>) {
        let (found, files) = flume::unbounded();
        (FileTracker::new(filter, found), files)
    }

    fn accept_all_tracker() -> (FileTracker<impl Fn(&Path) -> bool>, flume::Receiver<WatchedFile>) {
        tracker(|_| true)
    }

    fn ap(path: &str) -> Arc<Path> {
        Arc::from(Path::new(path))
    }

    fn walk(entries: Vec<(Arc<Path>, PathFingerprint)>, complete_dirs: &[&str]) -> WalkResult {
        WalkResult {
            entries,
            complete_dirs: complete_dirs.iter().map(|d| ap(d)).collect(),
        }
    }

    /// A regular-file mark distinguished by size alone.
    fn mark(size: u64) -> ContentMark {
        ContentMark {
            size,
            modified: None,
            #[cfg(unix)]
            identity: FdIdentity::from_raw(0, 0),
        }
    }

    /// What a stat of a node of `kind` reports: regular files always carry a mark.
    fn fingerprint(kind: FileKind) -> PathFingerprint {
        match kind {
            FileKind::File => PathFingerprint::File(mark(0)),
            FileKind::Dir => PathFingerprint::Dir,
            FileKind::Symlink => PathFingerprint::Symlink,
            FileKind::Other => PathFingerprint::Other,
        }
    }

    fn truth(ids: &[&str]) -> Vec<(Arc<Path>, PathFingerprint)> {
        ids.iter().map(|id| (ap(&format!("/r/{id}")), fingerprint(FileKind::File))).collect()
    }

    fn keys(tracker: &FileTracker<impl Fn(&Path) -> bool>) -> HashSet<Arc<Path>> {
        tracker.entries.keys().cloned().collect()
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

    #[rstest]
    fn observe_fingerprint_yields_a_file_once() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        assert_eq!(tracker.entries.len(), 1);
        assert_eq!(found.len(), 1);
    }

    #[rstest]
    fn observe_fingerprint_re_yields_a_file_after_a_kind_change() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/x"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/x"), PathFingerprint::Dir);
        tracker.observe_fingerprint(ap("/r/x"), PathFingerprint::File(mark(1)));
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 2);
        assert!(files[0].stat.has_changed().is_err(), "the replaced file's stat stayed open");
        assert!(files[1].stat.has_changed().is_ok());
    }

    // A directory that turns into a file takes its tracked children with it; otherwise their
    // stats outlive the file's own removal until the next full scan.
    #[rstest]
    fn observe_fingerprint_kind_change_from_dir_drops_descendants() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/x"), PathFingerprint::Dir);
        tracker.observe_fingerprint(ap("/r/x/c"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/x"), PathFingerprint::File(mark(1)));
        assert_eq!(keys(&tracker), std::iter::once(ap("/r/x")).collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/x/c")).collect());
    }

    // The filter sees each file path once, never directories, and a rejected file stays
    // rejected across content changes.
    #[rstest]
    fn rejected_files_are_recorded_and_not_re_offered() {
        let offers = Arc::new(AtomicU64::new(0));
        let o = Arc::clone(&offers);
        let (mut tracker, found) = tracker(move |_| {
            o.fetch_add(1, Ordering::SeqCst);
            false
        });
        tracker.observe_fingerprint(ap("/r/d"), PathFingerprint::Dir);
        tracker.observe_fingerprint(ap("/r/f"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/f"), PathFingerprint::File(mark(2)));
        assert_eq!(offers.load(Ordering::SeqCst), 1);
        assert!(found.is_empty());
        assert!(matches!(tracker.entries.get(Path::new("/r/f")), Some(Slot::Untracked(_))));
    }

    #[rstest]
    fn forget_closes_the_stat() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        tracker.entries.remove(Path::new("/r/a"));
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn forget_tree_drops_subtree() {
        let (mut tracker, _found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/sub"), PathFingerprint::Dir);
        tracker.observe_fingerprint(ap("/r/sub/a"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/sub2/b"), PathFingerprint::File(mark(1)));
        tracker.forget_tree(Path::new("/r/sub"));
        assert_eq!(keys(&tracker), std::iter::once(ap("/r/sub2/b")).collect());
    }

    // A tracked file has no descendants, so forgetting it must be a single removal rather than
    // a sweep: a (stale) entry under its path is the witness that no prefix scan ran.
    #[rstest]
    fn forget_tree_of_a_file_removes_only_that_entry() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/a/stale"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/b"), PathFingerprint::File(mark(1)));
        tracker.forget_tree(Path::new("/r/a"));
        assert_eq!(keys(&tracker), [ap("/r/a/stale"), ap("/r/b")].into_iter().collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn reconcile_adds_and_removes() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.reconcile(walk(truth(&["a", "b"]), &["/r"]));
        assert_eq!(found.len(), 2);
        tracker.reconcile(walk(truth(&["b", "c"]), &["/r"]));
        assert_eq!(keys(&tracker), [ap("/r/b"), ap("/r/c")].into_iter().collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 3);
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn reconcile_repairs_kind_swap() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.reconcile(walk(vec![(ap("/r/x"), fingerprint(FileKind::Dir))], &["/r"]));
        tracker.reconcile(walk(vec![(ap("/r/x"), fingerprint(FileKind::File))], &["/r"]));
        assert_eq!(tracker.entries.get(Path::new("/r/x")).map(Slot::kind), Some(FileKind::File));
        assert_eq!(found.len(), 1);
    }

    #[rstest]
    fn reconcile_without_prune_keeps_missing() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.reconcile(walk(truth(&["a", "b"]), &["/r"]));
        // A scan that read no directories prunes nothing, so "b" must survive.
        tracker.reconcile(walk(truth(&["a"]), &[]));
        assert_eq!(keys(&tracker), [ap("/r/a"), ap("/r/b")].into_iter().collect());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(live(&files), 2);
    }

    #[rstest]
    fn reconcile_keeps_nodes_under_unreadable_dir() {
        let (mut tracker, _found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/gone"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/sub/kept"), PathFingerprint::File(mark(1)));
        // The scan read /r and saw /r/sub, but could not read /r/sub itself.
        tracker.reconcile(walk(vec![(ap("/r/sub"), fingerprint(FileKind::Dir))], &["/r"]));
        // /r/gone: parent /r read, absent from truth -> pruned.
        // /r/sub/kept: shielded by /r/sub, which exists but was not read -> kept.
        assert_eq!(keys(&tracker), [ap("/r/sub"), ap("/r/sub/kept")].into_iter().collect());
    }

    #[rstest]
    fn reconcile_prunes_removed_subtree_descendants() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/sub"), PathFingerprint::Dir);
        tracker.observe_fingerprint(ap("/r/sub/child"), PathFingerprint::File(mark(1)));
        // /r/sub was removed (missed event): the scan read /r, /r/sub is absent and
        // cannot be scanned, so both it and its descendants must be pruned.
        tracker.reconcile(walk(vec![], &["/r"]));
        assert!(tracker.entries.is_empty());
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(live(&files), 0);
    }

    #[rstest]
    fn mark_change_updates_the_stat_in_place() {
        let (mut tracker, found) = accept_all_tracker();
        tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(2)));
        let mut files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 1, "file was re-yielded");
        let mut stat = files.pop().unwrap().stat;
        assert!(stat.has_changed().unwrap());
        let seen = *stat.borrow_and_update();
        assert_eq!(seen.size(), 2);
        assert!(matches!(
            tracker.entries.get(Path::new("/r/a")),
            Some(Slot::Tracked(known, _)) if *known == mark(2)
        ));

        tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(2)));
        assert!(!stat.has_changed().unwrap(), "same mark must be a no-op");
    }

    #[rstest]
    fn reconcile_signals_mark_change() {
        let (mut tracker, found) = accept_all_tracker();
        let at = |size| vec![(ap("/r/a"), PathFingerprint::File(mark(size)))];
        tracker.reconcile(walk(at(1), &["/r"]));
        let mut stat = found.recv().unwrap().stat;
        tracker.reconcile(walk(at(1), &["/r"]));
        assert!(!stat.has_changed().unwrap());
        tracker.reconcile(walk(at(2), &["/r"]));
        assert!(stat.has_changed().unwrap());
        assert!(found.is_empty(), "file was re-yielded");
        let seen = *stat.borrow_and_update();
        assert_eq!(seen.size(), 2);
    }

    #[rstest]
    fn written_within_selects_tracked_files_inside_the_window() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let written = |age_secs: u64| {
            PathFingerprint::File(ContentMark {
                modified: Some(now - Duration::from_secs(age_secs)),
                ..mark(1)
            })
        };
        let (mut tracker, _found) = tracker(|path| !path.ends_with("rejected"));
        tracker.observe_fingerprint(ap("/r/hot"), written(30));
        tracker.observe_fingerprint(ap("/r/cold"), written(120));
        tracker.observe_fingerprint(ap("/r/rejected"), written(0));
        tracker.observe_fingerprint(ap("/r/dir"), PathFingerprint::Dir);

        let within: HashSet<Arc<Path>> =
            tracker.written_within(Duration::from_secs(60), now).collect();
        assert_eq!(within, std::iter::once(ap("/r/hot")).collect());
    }

    proptest! {
        #[test]
        fn reconcile_entries_always_equal_truth(
            states in prop::collection::vec(prop::collection::hash_set(0u16..24, 0..24), 1..24)
        ) {
            let (mut tracker, found) = accept_all_tracker();
            let mut files = Vec::new();
            for state in &states {
                let want: HashSet<Arc<Path>> = state
                    .iter()
                    .map(|id| ap(&format!("/r/{id}")))
                    .collect();
                let truth: Vec<(Arc<Path>, PathFingerprint)> =
                    want.iter().cloned().map(|p| (p, fingerprint(FileKind::File))).collect();
                tracker.reconcile(walk(truth, &["/r"]));
                prop_assert_eq!(keys(&tracker), want);
                files.extend(found.drain());
            }
            prop_assert_eq!(live(&files), tracker.entries.len());
        }

        #[test]
        fn reconcile_with_rejections_keeps_one_stat_per_tracked_file(
            states in prop::collection::vec(prop::collection::hash_set(0u16..24, 0..24), 1..16)
        ) {
            let (mut tracker, found) = tracker(|path| {
                path.file_name().is_some_and(|name| name.to_string_lossy().parse::<u16>().unwrap() % 3 != 0)
            });
            let mut files = Vec::new();
            for state in &states {
                let truth: Vec<(Arc<Path>, PathFingerprint)> = state
                    .iter()
                    .map(|id| {
                        let kind = if id % 2 == 0 { FileKind::File } else { FileKind::Dir };
                        (ap(&format!("/r/{id}")), fingerprint(kind))
                    })
                    .collect();
                tracker.reconcile(walk(truth, &["/r"]));
                files.extend(found.drain());
            }
            let tracked = tracker
                .entries
                .values()
                .filter(|slot| matches!(slot, Slot::Tracked(..)))
                .count();
            prop_assert_eq!(live(&files), tracked);
        }
    }
}
