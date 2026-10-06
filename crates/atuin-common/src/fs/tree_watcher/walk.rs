use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::PathFingerprint;
use super::tracker::FileTracker;
use crate::sync::BlockingPool;

/// A best-effort tree scan result.
#[derive(Debug, Default)]
pub struct WalkResult {
    /// Every node the scan could read.
    pub entries: Vec<(Arc<Path>, PathFingerprint)>,
    /// The directories whose children were all read, so a node absent beneath one is gone.
    pub complete_dirs: HashSet<Arc<Path>>,
}

/// Walks the whole tree on the periodic scan, reconciling the tracker against what it finds.
pub struct WalkPoller {
    pool: BlockingPool,
    root: PathBuf,
    recursive: bool,
}

impl WalkPoller {
    pub fn new(pool: BlockingPool, root: PathBuf, recursive: bool) -> Self {
        Self {
            pool,
            root,
            recursive,
        }
    }

    /// The directory this walks, which is the watcher's root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Walk the tree in the pool and reconcile `tracker` against it. A cancelled walk read nothing,
    /// so it prunes nothing.
    pub async fn poll<F>(&self, tracker: &mut FileTracker<F>)
    where
        F: Fn(&Path) -> bool,
    {
        let root = self.root.clone();
        let recursive = self.recursive;
        let walk = self.pool.run(move || Self::scan(&root, recursive)).await.unwrap_or_default();
        if walk.complete_dirs.is_empty() {
            tracing::warn!("tree watcher scan read no directories; skipping prune this pass");
        }
        tracker.reconcile(walk);
    }

    /// Walk `root` best-effort, returning every readable entry plus the set of directories that
    /// were fully read without error.
    fn scan(root: &Path, recursive: bool) -> WalkResult {
        let mut pending: Vec<Arc<Path>> = vec![Arc::from(root)];
        let listings = std::iter::from_fn(move || {
            let dir = pending.pop()?;
            let children = Self::read_children(&dir);
            if recursive {
                pending.extend(
                    children
                        .iter()
                        .flatten()
                        .filter(|(_, fingerprint)| matches!(fingerprint, PathFingerprint::Dir))
                        .map(|(path, _)| Arc::clone(path)),
                );
            }
            Some((dir, children))
        });

        listings.fold(WalkResult::default(), |mut walk, (dir, children)| {
            // Only a fully enumerated directory lets us trust the absence of its children.
            if children.iter().all(Result::is_ok) {
                walk.complete_dirs.insert(dir);
            }
            walk.entries.extend(children.into_iter().flatten());
            walk
        })
    }

    /// Read each child of `dir` best-effort. A `dir` that cannot be read at all is one failed read,
    /// so it never counts as complete.
    fn read_children(dir: &Path) -> Vec<io::Result<(Arc<Path>, PathFingerprint)>> {
        std::fs::read_dir(dir).map_or_else(
            |err| vec![Err(err)],
            |entries| {
                entries
                    .map(|entry| {
                        let entry = entry?;
                        // Kinds come from the directory read; only regular files pay for a stat.
                        let fingerprint =
                            PathFingerprint::new(entry.file_type()?, || entry.metadata())?;
                        Ok((Arc::from(entry.path()), fingerprint))
                    })
                    .collect()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::num::NonZeroUsize;

    use rstest::rstest;

    use super::*;
    use crate::fs::tree_watcher::{ContentMark, FileKind};
    #[cfg(unix)]
    use crate::os::fs::FdIdentity;

    fn scanned_kinds(
        entries: &[(Arc<Path>, PathFingerprint)],
        root: &Path,
    ) -> BTreeMap<String, FileKind> {
        entries
            .iter()
            .map(|(p, fingerprint)| {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                (rel, FileKind::from(fingerprint))
            })
            .collect()
    }

    #[rstest]
    #[case(false, false)]
    #[case(true, true)]
    fn scan_recursion_controls_descent(#[case] recursive: bool, #[case] expect_nested: bool) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), b"x").unwrap();

        let walk = WalkPoller::scan(dir.path(), recursive);
        assert!(walk.complete_dirs.contains(dir.path()));
        let map = scanned_kinds(&walk.entries, dir.path());
        assert_eq!(map.get("a"), Some(&FileKind::File));
        assert_eq!(map.get("sub"), Some(&FileKind::Dir));
        assert_eq!(map.get("sub/b"), expect_nested.then_some(&FileKind::File));
    }

    #[cfg(unix)]
    #[rstest]
    fn scan_does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/inner"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();

        let walk = WalkPoller::scan(dir.path(), true);
        assert!(walk.complete_dirs.contains(dir.path()));
        let map = scanned_kinds(&walk.entries, dir.path());
        assert_eq!(map.get("link"), Some(&FileKind::Symlink));
        assert!(!map.contains_key("link/inner"));
        assert_eq!(map.get("real/inner"), Some(&FileKind::File));
    }

    #[rstest]
    fn scan_reports_incomplete_on_unreadable_root() {
        let walk = WalkPoller::scan(Path::new("/this/does/not/exist/anywhere"), true);
        assert!(walk.entries.is_empty());
        assert!(walk.complete_dirs.is_empty());
    }

    // A walk that read no directory is no proof that anything under it is gone.
    #[rstest]
    #[tokio::test]
    async fn poll_of_unreadable_root_prunes_nothing() {
        let root = Path::new("/this/does/not/exist/anywhere");
        let (found, files) = flume::unbounded();
        let mut tracker = FileTracker::new(|_: &Path| true, found);
        let mark = ContentMark {
            size: 1,
            modified: None,
            #[cfg(unix)]
            identity: FdIdentity::from_raw(0, 0),
        };
        tracker.observe_fingerprint(Arc::from(root.join("a")), PathFingerprint::File(mark));

        let pool = BlockingPool::new(NonZeroUsize::MIN);
        WalkPoller::new(pool, root.to_path_buf(), true).poll(&mut tracker).await;

        let file = files.recv().unwrap();
        assert!(file.stat().has_changed().is_ok(), "the walk pruned a file it never read");
    }
}
