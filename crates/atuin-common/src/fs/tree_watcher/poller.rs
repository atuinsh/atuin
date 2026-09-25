use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind, RenameMode};
use notify_debouncer_full::DebounceEventResult as NotifyEventResult;
use tokio::time::MissedTickBehavior;

use super::PathFingerprint;
use super::stat::StatPoller;
use super::tracker::FileTracker;
use super::walk::WalkPoller;
use crate::time::NonZeroDuration;

/// Drives a [`FileTracker`] on a background task: reconciles it against a periodic full scan,
/// re-stats recently written files on a shorter poll, and applies debounced filesystem events as
/// they arrive.
pub struct TreeWatcherPoller<F> {
    walk: WalkPoller,
    stat: StatPoller,
    tracker: FileTracker<F>,
}

impl<F> TreeWatcherPoller<F>
where
    F: Fn(&Path) -> bool + Send + 'static,
{
    pub fn new(walk: WalkPoller, stat: StatPoller, tracker: FileTracker<F>) -> Self {
        Self {
            walk,
            stat,
            tracker,
        }
    }

    pub async fn run(
        mut self,
        events: flume::Receiver<NotifyEventResult>,
        scan_interval: NonZeroDuration,
        content_poll_interval: NonZeroDuration,
    ) {
        self.walk.poll(&mut self.tracker).await;

        let mut scan_tick = tokio::time::interval(scan_interval.get());
        scan_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        scan_tick.tick().await;
        let mut poll_tick = tokio::time::interval(content_poll_interval.get());
        poll_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        poll_tick.tick().await;

        loop {
            tokio::select! {
                _ = scan_tick.tick() => self.walk.poll(&mut self.tracker).await,
                _ = poll_tick.tick() => self.stat.poll(&mut self.tracker).await,
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
                                let kinds = Self::resolve_kinds(&self.stat, &pending).await;
                                for event in &pending {
                                    self.observe_event(event, &kinds);
                                }
                            }
                            if force_scan {
                                self.walk.poll(&mut self.tracker).await;
                            }
                        }
                        Ok(Err(errors)) => tracing::warn!(?errors, "tree watcher backend error"),
                        Err(flume::RecvError::Disconnected) => break,
                    }
                }
            }
        }
    }

    /// Stat every path referenced by `events`, keeping only the ones that still exist.
    async fn resolve_kinds<'a>(
        stat: &StatPoller,
        events: impl IntoIterator<Item = &'a notify::Event>,
    ) -> HashMap<Arc<Path>, PathFingerprint> {
        let paths: Vec<Arc<Path>> = events
            .into_iter()
            .flat_map(|event| event.paths.iter().map(|p| Arc::from(p.as_path())))
            .collect();
        stat.stat(paths).await
    }

    fn observe_path(&mut self, path: &Path, kinds: &HashMap<Arc<Path>, PathFingerprint>) {
        let Some((key, &fingerprint)) = kinds.get_key_value(path) else {
            return;
        };
        self.tracker.observe_fingerprint(Arc::clone(key), fingerprint);
    }

    fn observe_event(
        &mut self,
        event: &notify::Event,
        kinds: &HashMap<Arc<Path>, PathFingerprint>,
    ) {
        match &event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                for path in &event.paths {
                    self.observe_path(path, kinds);
                }
            }
            // `observe_path` tells a new node from a changed one. A content event for a path that
            // no longer stats is the only trace of a young file's removal on macOS: FSEvents
            // reports it with sticky `Created`+`Removed` flags, which the debouncer cancels out.
            EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Any)
            | EventKind::Access(AccessKind::Close(AccessMode::Write)) => {
                for path in &event.paths {
                    // The root is watched, not tracked.
                    if path.as_path() == self.walk.root() {
                        continue;
                    }
                    if kinds.contains_key(path.as_path()) {
                        self.observe_path(path, kinds);
                    } else {
                        self.tracker.forget_tree(path);
                    }
                }
            }
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                for path in &event.paths {
                    self.tracker.forget_tree(path);
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let [from, to] = event.paths.as_slice() {
                    self.tracker.forget_tree(from);
                    self.observe_path(to, kinds);
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Any | RenameMode::Other)) => {
                // TODO(markovejnovic): on a case-insensitive filesystem (e.g. macOS APFS), a
                // case-only rename (`a.log` -> `A.log`) leaves duplicate entries:
                // `symlink_metadata` on the stale-case name still succeeds, so we keep it while
                // also observing the new name, and the two byte-distinct keys coexist until the
                // next complete scan prunes the stale one. Fix later by reconciling the renamed
                // parent dir, or case-folding keys on case-insensitive platforms.
                for path in &event.paths {
                    if kinds.contains_key(path.as_path()) {
                        self.observe_path(path, kinds);
                    } else {
                        self.tracker.forget_tree(path);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::NonZeroUsize;
    use std::path::PathBuf;
    use std::time::Duration;

    use notify::event::{DataChange, MetadataKind};
    use rstest::rstest;

    use super::*;
    use crate::fs::tree_watcher::{ContentMark, FileKind, WatchedFile};
    #[cfg(unix)]
    use crate::os::fs::FdIdentity;
    use crate::sync::BlockingPool;

    fn poller()
    -> (TreeWatcherPoller<impl Fn(&Path) -> bool + Send + 'static>, flume::Receiver<WatchedFile>)
    {
        let pool = BlockingPool::new(NonZeroUsize::MIN);
        let (found, files) = flume::unbounded();
        let poller = TreeWatcherPoller::new(
            WalkPoller::new(pool.clone(), PathBuf::from("/r"), true),
            StatPoller::new(pool, Duration::ZERO),
            FileTracker::new(|_: &Path| true, found),
        );
        (poller, files)
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

    fn kinds_map<const N: usize>(
        entries: [(&str, FileKind); N],
    ) -> HashMap<Arc<Path>, PathFingerprint> {
        entries.into_iter().map(|(p, k)| (ap(p), fingerprint(k))).collect()
    }

    fn marked_map<const N: usize>(
        entries: [(&str, u64); N],
    ) -> HashMap<Arc<Path>, PathFingerprint> {
        entries.into_iter().map(|(p, size)| (ap(p), PathFingerprint::File(mark(size)))).collect()
    }

    /// The paths of `files` whose stat has closed: the watcher stopped tracking them.
    fn gone(files: &[WatchedFile]) -> HashSet<Arc<Path>> {
        files
            .iter()
            .filter(|file| file.stat.has_changed().is_err())
            .map(|file| Arc::clone(&file.path))
            .collect()
    }

    #[rstest]
    #[case(true, 1)]
    #[case(false, 0)]
    fn observe_create_event_observes_only_existing(#[case] exists: bool, #[case] yielded: usize) {
        let (mut poller, found) = poller();
        let kinds = if exists {
            kinds_map([("/r/a", FileKind::File)])
        } else {
            HashMap::new()
        };
        let ev =
            event(EventKind::Create(notify::event::CreateKind::Any), vec![PathBuf::from("/r/a")]);
        poller.observe_event(&ev, &kinds);
        assert_eq!(poller.tracker.contains(Path::new("/r/a")), exists);
        assert_eq!(found.len(), yielded);
    }

    #[rstest]
    fn observe_remove_event_forgets() {
        let (mut poller, found) = poller();
        poller.tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        let ev =
            event(EventKind::Remove(notify::event::RemoveKind::Any), vec![PathBuf::from("/r/a")]);
        poller.observe_event(&ev, &HashMap::new());
        assert!(!poller.tracker.contains(Path::new("/r/a")));
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    fn observe_rename_both_moves_the_file() {
        let (mut poller, _found) = poller();
        poller.tracker.observe_fingerprint(ap("/r/from"), PathFingerprint::File(mark(1)));
        let kinds = kinds_map([("/r/to", FileKind::File)]);
        let ev = event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), vec![
            PathBuf::from("/r/from"),
            PathBuf::from("/r/to"),
        ]);
        poller.observe_event(&ev, &kinds);
        assert!(!poller.tracker.contains(Path::new("/r/from")));
        assert!(poller.tracker.contains(Path::new("/r/to")));
    }

    #[rstest]
    fn observe_rename_any_observes_moved_in_file() {
        let (mut poller, found) = poller();
        let kinds = kinds_map([("/r/a", FileKind::File)]);
        let ev = event(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), vec![PathBuf::from(
            "/r/a",
        )]);
        poller.observe_event(&ev, &kinds);
        assert!(poller.tracker.contains(Path::new("/r/a")));
        assert_eq!(found.len(), 1);
    }

    #[rstest]
    fn observe_rename_any_forgets_moved_out_file() {
        let (mut poller, found) = poller();
        poller.tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        let ev = event(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), vec![PathBuf::from(
            "/r/a",
        )]);
        poller.observe_event(&ev, &HashMap::new());
        assert!(!poller.tracker.contains(Path::new("/r/a")));
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
    }

    #[rstest]
    #[case::data(EventKind::Modify(ModifyKind::Data(DataChange::Any)))]
    #[case::metadata(EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)))]
    #[case::any(EventKind::Modify(ModifyKind::Any))]
    #[case::close_write(EventKind::Access(AccessKind::Close(AccessMode::Write)))]
    fn observe_content_event_signals_change(#[case] kind: EventKind) {
        let (mut poller, found) = poller();
        poller.tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        let ev = event(kind, vec![PathBuf::from("/r/a")]);
        poller.observe_event(&ev, &marked_map([("/r/a", 2)]));
        let mut files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(files.len(), 1);
        let mut stat = files.pop().unwrap().stat;
        assert!(stat.has_changed().unwrap());
        let seen = *stat.borrow_and_update();
        assert_eq!(seen.size(), 2);
    }

    // The root is watched, not tracked: its own metadata changes must not offer it as a node.
    #[rstest]
    fn observe_content_event_ignores_the_root() {
        let (mut poller, found) = poller();
        let ev = event(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Permissions)), vec![
            PathBuf::from("/r"),
        ]);
        poller.observe_event(&ev, &kinds_map([("/r", FileKind::Dir)]));
        assert!(!poller.tracker.contains(Path::new("/r")));
        assert!(found.is_empty());
    }

    // FSEvents reports a young file's removal as `Create`+`Remove` (sticky flags), which the
    // debouncer cancels out, leaving only a content event for the now-missing path.
    #[rstest]
    #[case::data(EventKind::Modify(ModifyKind::Data(DataChange::Any)))]
    #[case::metadata(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Extended)))]
    fn observe_content_event_forgets_vanished_file(#[case] kind: EventKind) {
        let (mut poller, found) = poller();
        poller.tracker.observe_fingerprint(ap("/r/a"), PathFingerprint::File(mark(1)));
        let ev = event(kind, vec![PathBuf::from("/r/a")]);
        poller.observe_event(&ev, &HashMap::new());
        assert!(!poller.tracker.contains(Path::new("/r/a")));
        let files: Vec<WatchedFile> = found.drain().collect();
        assert_eq!(gone(&files), std::iter::once(ap("/r/a")).collect());
        assert_eq!(files[0].stat.borrow().size(), 1, "a vanished file is not a change");
    }
}
