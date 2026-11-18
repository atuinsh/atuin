use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId};
use atuin_common::sync::free_in_background;
use parking_lot::{MappedRwLockReadGuard, Mutex, RwLockReadGuard, RwLockWriteGuard};
use time::OffsetDateTime;

use crate::search::index::SearchIndexSettings;
use crate::search::index::core::{LoadFromDbError, SearchIndex};
use crate::search::index::shared::{FillState, SharedSearchIndex};

/// The settings that the `settings` callback of a [`SharedSearchIndex::rebuild`] decides from.
#[derive(Debug, Clone, Copy)]
pub struct RebuildSettings<'a> {
    /// The installed index's settings.
    pub installed: &'a SearchIndexSettings,

    /// The settings that a rebuild started now would build on.
    ///
    /// If a rebuild is running, these are the settings of the most recently started one. Otherwise
    /// they are the installed index's settings. Either way, they use the latest frecency
    /// multipliers. Base the settings for a new rebuild on these. That way the new rebuild keeps
    /// changes that haven't installed yet.
    pub latest: &'a SearchIndexSettings,
}

/// The index that a [`SharedSearchIndex::rebuild`] produced.
///
/// If no rebuild was needed, this is the installed index.
pub enum RebuiltIndex<'a> {
    /// The installed index, locked for reading. It is either unchanged or the index that this
    /// rebuild installed.
    Installed(MappedRwLockReadGuard<'a, SearchIndex>),

    /// The rebuilt index. It wasn't installed, because a newer rebuild installed first. It is
    /// freed on a blocking thread when it is dropped.
    Superseded(Box<SearchIndex>),
}

impl Deref for RebuiltIndex<'_> {
    type Target = SearchIndex;

    fn deref(&self) -> &SearchIndex {
        match self {
            Self::Installed(index) => index,
            Self::Superseded(index) => index,
        }
    }
}

impl Drop for RebuiltIndex<'_> {
    fn drop(&mut self) {
        if let Self::Superseded(index) = self {
            free_in_background(std::mem::take(&mut **index));
        }
    }
}

/// How a call to [`SharedSearchIndex::start_rebuild`] went.
pub(super) enum RebuildStart<'a> {
    /// No rebuild was needed. This holds the installed index, locked for reading.
    NotNeeded(MappedRwLockReadGuard<'a, SearchIndex>),
    Started(Rebuild<'a>),
}

/// A rebuild of a [`SharedSearchIndex`] that has started and hasn't finished yet.
///
/// The identity of its `Arc` identifies the rebuild.
#[derive(Debug)]
pub(super) struct PendingRebuild {
    /// The settings that this rebuild builds with. If this is the most recently started rebuild,
    /// rebuilds that start after it build on these settings.
    pub settings: SearchIndexSettings,
    /// Entries that were registered while the rebuild runs. See
    /// [`AddPermit::register`](super::AddPermit::register).
    ///
    /// Entries are removed once the rebuild's load reads them from the database. The entries that
    /// are left when the rebuild installs are added to the new index.
    pub added: Mutex<HashMap<HistoryId, Arc<History>>>,
}

/// A rebuild that has started. See [`SharedSearchIndex::rebuild`].
///
/// A rebuild can be dropped before it installs. Then the rebuild is abandoned. It stops being
/// pending, and adds stop registering with it.
///
/// In the daemon, a failed load is the realistic cause. Every caller runs its rebuild in a spawned
/// task, so a client that disconnects can't cancel it. The future is otherwise dropped only at
/// shutdown or by a panic.
///
/// Abandoning a rebuild loses no entries that permits added. Each of those entries was also added
/// to the installed index. It was also registered with every other pending rebuild.
pub(super) struct Rebuild<'a> {
    shared: &'a SharedSearchIndex,
    pending: Arc<PendingRebuild>,
    /// When the rebuild started. The rebuilt index's frecency is computed as of this time.
    now: OffsetDateTime,
    /// Whether dropping the rebuild should abandon it. Install clears this flag once it has dealt
    /// with `pending`.
    abandon_on_drop: bool,
}

impl Drop for Rebuild<'_> {
    fn drop(&mut self) {
        if self.abandon_on_drop {
            self.shared.state.write().pending.retain(|p| !Arc::ptr_eq(p, &self.pending));
        }
    }
}

impl<'a> Rebuild<'a> {
    /// A rebuild of `shared` that starts now. `pending` must already be in the list of pending
    /// rebuilds.
    pub fn new(shared: &'a SharedSearchIndex, pending: Arc<PendingRebuild>) -> Self {
        Self {
            shared,
            pending,
            now: OffsetDateTime::now_utc(),
            abandon_on_drop: true,
        }
    }

    /// Load the rebuilt index from `db`. See [`SharedSearchIndex::rebuild`].
    pub async fn load(&self, db: &Sqlite) -> Result<SearchIndex, LoadFromDbError> {
        let mut new_index = SearchIndex::new(self.pending.settings.clone());
        new_index
            .load_from_db_at(db, self.now, |page| {
                // The load has read these entries. So they are no longer needed as leftovers.
                let mut added = self.pending.added.lock();
                if !added.is_empty() {
                    for history in page {
                        added.remove(&history.id);
                    }
                }
            })
            .await?;

        // Apply any frecency multipliers that were asked for during the load. This runs outside
        // the lock.
        let multipliers = self.shared.state.read().frecency_multipliers;
        if new_index.settings().frecency_multipliers != multipliers {
            new_index.rebuild_frecency(multipliers);
        }
        Ok(new_index)
    }

    /// Add the entries that were registered with this rebuild and that its load didn't read.
    fn add_leftovers(&self, new_index: &mut SearchIndex) {
        let leftovers = std::mem::take(&mut *self.pending.added.lock());
        new_index.add_history_at(leftovers.values().map(|history| &**history), self.now);
    }

    /// Add the leftovers to `new_index`, which is the index that this rebuild loaded. Then install
    /// it if the install rules allow it. See [`SharedSearchIndex::rebuild`].
    pub async fn install(mut self, mut new_index: SearchIndex) -> RebuiltIndex<'a> {
        let shared = self.shared;
        // Wait for adds in progress to finish. Then keep new adds out until the new index is
        // installed. Otherwise an entry registered from here on would be added to the new index
        // twice. It would be added once as a leftover and once by its permit.
        let add = shared.add_lock.write().await;

        // Decide whether to install before adding the leftovers. That way a superseded rebuild
        // doesn't block adds while it adds its leftovers. The decision holds until the swap below,
        // because this rebuild holds `add_lock` until then. A rebuild needs `add_lock` to start or
        // to install. So no rebuild can start or change the installed index in the meantime.
        let superseded = {
            let mut state = shared.state.write();
            // `pending` is dealt with here, under this lock. So dropping `self` must not lock
            // again.
            self.abandon_on_drop = false;
            match state.pending.iter().position(|p| Arc::ptr_eq(p, &self.pending)) {
                // This rebuild installs. Every older rebuild stops being pending too. Those
                // rebuilds are superseded.
                Some(i) => {
                    drop(state.pending.drain(..=i));
                    false
                }
                // A newer rebuild installed first. That install removed this rebuild.
                None => true,
            }
        };
        if superseded {
            // An older index never replaces a newer one. The caller still searches this index, so
            // it gets the leftovers too. This rebuild is no longer pending, so no permit registers
            // with it any more. Its leftovers can't change, and adding them doesn't need
            // `add_lock`.
            drop(add);
            self.add_leftovers(&mut new_index);
            tracing::info!("Search index rebuild superseded by a newer one; not installing it");
            return RebuiltIndex::Superseded(Box::new(new_index));
        }

        self.add_leftovers(&mut new_index);
        let mut state = shared.state.write();
        // Apply any multipliers that were asked for after the load checked.
        if new_index.settings().frecency_multipliers != state.frecency_multipliers {
            new_index.rebuild_frecency(state.frecency_multipliers);
        }
        let old = std::mem::replace(&mut state.index, new_index);
        if matches!(state.fill, FillState::Waiting) {
            state.fill = FillState::Filled;
        }
        let state = RwLockWriteGuard::downgrade(state);
        drop(add);
        free_in_background(old);
        RebuiltIndex::Installed(RwLockReadGuard::map(state, |s| &s.index))
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::filter::OrFilter;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
    use crate::search::index::core::tests::*;
    use crate::search::index::shared::tests::{add_through_permit, tag};

    /// Rebuilding the shared index reloads it from the database with the settings that the closure
    /// returns.
    ///
    /// In this test, the closure keeps the shell filter and sets new frecency multipliers. A
    /// closure that returns `None` skips the rebuild. Either way, the rebuild returns the index
    /// that is now installed.
    #[rstest]
    #[tokio::test]
    async fn shared_index_rebuild(#[future(awt)] db: Sqlite) {
        for (command, shell) in [("echo bash", "bash"), ("echo zsh", "zsh")] {
            let mut history = make_history(command, "/tmp", OffsetDateTime::now_utc());
            history.shell = Some(shell.to_owned());
            db.save(&history).await.unwrap();
        }
        let shells = OrFilter::from_list(vec!["bash".to_owned()]).unwrap();
        let shared = SharedSearchIndex::from(SearchIndex::new(SearchIndexSettings {
            shells: shells.clone(),
            ..SearchIndexSettings::default()
        }));

        let multipliers = zero_frecency();
        // Returning `None` skips the rebuild. The rebuild then returns the unchanged index.
        let skipped = shared.rebuild(&db, |_| None).await.unwrap();
        assert!(matches!(skipped, RebuiltIndex::Installed(_)));
        assert_eq!(skipped.command_count(), 0);
        drop(skipped);

        // Otherwise the rebuild installs the rebuilt index and returns it.
        let rebuilt = shared.rebuild(&db, |settings| {
            Some(SearchIndexSettings {
                frecency_multipliers: multipliers,
                ..settings.latest.clone()
            })
        });
        let index = rebuilt.await.unwrap();
        assert!(matches!(index, RebuiltIndex::Installed(_)));
        assert_eq!(index.settings().shells, shells);
        assert_eq!(index.settings().frecency_multipliers, multipliers);
        assert_eq!(index.command_count(), 1, "only the bash entry passes the kept filter");
        assert_eq!(frecency(&index, "echo bash"), 0);
    }

    /// Rebuilds start in the order `0..rebuilds` and finish in `finish_order`. `installs` says
    /// whether each rebuild installs or is superseded.
    ///
    /// An older index never replaces a newer one. A rebuild installs unless a newer rebuild
    /// installed first. The most recently started rebuild always ends up installed.
    #[rstest]
    #[case::one(1, &[0], &[true])]
    #[case::newer_first(2, &[1, 0], &[true, false])]
    #[case::older_first(2, &[0, 1], &[true, true])]
    #[case::middle_first(3, &[1, 0, 2], &[true, false, true])]
    #[case::oldest_first(3, &[0, 1, 2], &[true, true, true])]
    #[tokio::test]
    async fn rebuild_install_rules(
        #[case] rebuilds: usize,
        #[case] finish_order: &[usize],
        #[case] installs: &[bool],
        #[future(awt)] db: Sqlite,
    ) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let mut started = Vec::new();
        for i in 0..rebuilds {
            started.push(Some(shared.start_tagged(i).await));
        }

        for (&i, &installs) in finish_order.iter().zip(installs) {
            let rebuild = started[i].take().unwrap();
            let loaded = rebuild.load(&db).await.unwrap();
            let result = rebuild.install(loaded).await;
            assert_eq!(matches!(result, RebuiltIndex::Installed(_)), installs, "rebuild {i}");
            assert_eq!(result.settings().shells, tag(i), "rebuild {i} returns its own index");
            drop(result);
            if installs {
                assert_eq!(shared.read().settings().shells, tag(i));
            }
        }
        assert_eq!(shared.read().settings().shells, tag(rebuilds - 1));
        assert!(shared.state.read().pending.is_empty());
    }

    /// A rebuild can be dropped before it installs, for example because its load failed. The
    /// dropped rebuild removes only itself from the pending rebuilds.
    ///
    /// After that, an older rebuild can still install. It installs because no newer rebuild
    /// installed first.
    #[rstest]
    #[tokio::test]
    async fn abandoned_rebuild_removes_only_itself(#[future(awt)] db: Sqlite) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let oldest = shared.start_tagged(0).await;
        let older = shared.start_tagged(1).await;
        let newer = shared.start_tagged(2).await;
        let is_pending = |rebuild: &Rebuild<'_>| {
            shared.state.read().pending.iter().any(|p| Arc::ptr_eq(p, &rebuild.pending))
        };

        drop(oldest);
        assert!(is_pending(&older));
        assert!(is_pending(&newer));
        drop(newer);
        assert!(is_pending(&older));
        assert_eq!(shared.state.read().pending.len(), 1);

        let new_index = older.load(&db).await.unwrap();
        let installed = older.install(new_index).await;
        assert!(matches!(installed, RebuiltIndex::Installed(_)));
        drop(installed);
    }

    /// Entries that are added through permits while a rebuild runs end up in the rebuilt index
    /// exactly once. That holds whether or not the rebuild's load read them. Unregistered entries
    /// are the exception.
    #[rstest]
    #[tokio::test]
    async fn adds_during_a_rebuild_are_kept_once(#[future(awt)] db: Sqlite) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let history = |command| make_history(command, "/tmp", OffsetDateTime::now_utc());
        let rebuild = shared.start().await;
        let rebuild_settings = rebuild.pending.settings.clone();

        // This entry is written before the load. So the load reads it.
        add_through_permit(&shared, Some(&db), &history("echo read")).await;

        let loaded = rebuild.load(&db).await.unwrap();

        // This entry is written after the load. So it is a leftover.
        add_through_permit(&shared, Some(&db), &history("echo missed")).await;
        // This entry is never written, but it is added anyway. For example, its save may have
        // failed.
        add_through_permit(&shared, None, &history("echo unsaved")).await;

        // This entry is written, but the permit is dropped without adding it.
        let dropped = history("echo dropped");
        let permit = shared.begin_add().await;
        permit.register([&dropped]);
        db.save(&dropped).await.unwrap();
        drop(permit);

        // This entry won't be added after all.
        let skipped = history("echo skipped");
        let permit = shared.begin_add().await;
        permit.register([&skipped]);
        permit.unregister([skipped.id]);
        drop(permit);

        let installed = rebuild.install(loaded).await;
        assert!(matches!(installed, RebuiltIndex::Installed(_)));
        assert_eq!(*installed.settings(), rebuild_settings);
        drop(installed);

        // This entry is added after the install. It goes straight into the rebuilt index.
        add_through_permit(&shared, Some(&db), &history("echo after")).await;

        let index = shared.read();
        for name in ["echo read", "echo missed", "echo unsaved", "echo dropped", "echo after"] {
            assert_eq!(count(&index, name), 1, "{name}");
        }
        assert!(!contains(&index, "echo skipped"));
        drop(index);
    }

    /// An older rebuild can install while a newer one is pending. An entry that is added after
    /// the older rebuild's load registers with both rebuilds, so the older index has it too. The
    /// entry stays searchable whether the newer rebuild then installs or is abandoned.
    #[rstest]
    #[tokio::test]
    async fn older_rebuild_keeps_entries_added_while_a_newer_one_is_pending(
        #[values(false, true)] newer_installs: bool,
        #[future(awt)] db: Sqlite,
    ) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let older = shared.start().await;
        let older_loaded = older.load(&db).await.unwrap();
        let newer = shared.start().await;

        let history = make_history("echo meanwhile", "/tmp", OffsetDateTime::now_utc());
        add_through_permit(&shared, Some(&db), &history).await;

        let installed = older.install(older_loaded).await;
        assert!(matches!(installed, RebuiltIndex::Installed(_)));
        assert_eq!(count(&installed, "echo meanwhile"), 1);
        drop(installed);

        if newer_installs {
            let newer_loaded = newer.load(&db).await.unwrap();
            assert!(matches!(newer.install(newer_loaded).await, RebuiltIndex::Installed(_)));
        } else {
            drop(newer);
        }
        assert_eq!(count(&shared.read(), "echo meanwhile"), 1);
    }

    /// A superseded rebuild still adds its leftovers to the index that it returns. The caller
    /// searches that index.
    #[rstest]
    #[tokio::test]
    async fn superseded_rebuild_keeps_its_leftovers(#[future(awt)] db: Sqlite) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let older = shared.start().await;
        let loaded = older.load(&db).await.unwrap();

        // This entry is written after the older rebuild's load. So it is a leftover of that
        // rebuild.
        let history = make_history("echo leftover", "/tmp", OffsetDateTime::now_utc());
        add_through_permit(&shared, Some(&db), &history).await;

        let newer = shared.start().await;
        let newer_loaded = newer.load(&db).await.unwrap();
        assert!(matches!(newer.install(newer_loaded).await, RebuiltIndex::Installed(_)));

        let superseded = older.install(loaded).await;
        assert!(matches!(superseded, RebuiltIndex::Superseded(_)));
        assert_eq!(count(&superseded, "echo leftover"), 1);
        drop(superseded);
    }

    /// A rebuild that turns out to be unnecessary doesn't wait for adds in progress.
    #[rstest]
    #[tokio::test]
    async fn rebuild_check_does_not_wait_for_adds(#[future(awt)] db: Sqlite) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let permit = shared.begin_add().await;

        let checked =
            tokio::time::timeout(std::time::Duration::from_secs(5), shared.rebuild(&db, |_| None))
                .await
                .expect("the check waited for the permit")
                .unwrap();
        assert!(matches!(checked, RebuiltIndex::Installed(_)));
        drop(checked);
        drop(permit);
    }

    /// A rebuild whose future is dropped before it installs stops being pending. Adds then stop
    /// registering with it.
    #[rstest]
    #[tokio::test]
    async fn cancelled_rebuild_is_abandoned(#[future(awt)] db: Sqlite) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        // Holding a permit blocks the first rebuild's start. So the first rebuild is cancelled
        // before it is pending. The second rebuild is dropped after it starts but before it loads.
        let permit = shared.begin_add().await;
        let rebuild = shared.rebuild(&db, |settings| Some(settings.latest.clone()));
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), rebuild).await.is_err());
        drop(permit);

        let rebuild = shared.start().await;
        assert_eq!(shared.state.read().pending.len(), 1);
        drop(rebuild);
        assert!(shared.state.read().pending.is_empty());
    }

    /// Frecency multipliers can be set while a rebuild runs, either before or after its load. The
    /// install doesn't undo them. That holds for the latest rebuild. It also holds for an older
    /// rebuild that installs first.
    #[rstest]
    #[tokio::test]
    async fn rebuild_keeps_multipliers_set_while_it_runs(
        #[values(false, true)] newer_pending: bool,
        #[values(false, true)] after_load: bool,
        #[future(awt)] db: Sqlite,
    ) {
        db.save(&make_history("echo hello", "/tmp", OffsetDateTime::now_utc())).await.unwrap();
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let rebuild = shared.start().await;
        let newer = if newer_pending {
            Some(shared.start().await)
        } else {
            None
        };

        if !after_load {
            shared.rebuild_frecency(zero_frecency());
        }
        let loaded = rebuild.load(&db).await.unwrap();
        if after_load {
            shared.rebuild_frecency(zero_frecency());
        }
        let installed = rebuild.install(loaded).await;
        assert!(matches!(installed, RebuiltIndex::Installed(_)));
        drop(installed);
        drop(newer);

        let index = shared.read();
        assert_eq!(index.settings().frecency_multipliers, zero_frecency());
        assert_eq!(frecency(&index, "echo hello"), 0);
    }

    /// A rebuild's settings build on the settings of the most recently started rebuild. They also
    /// use the latest frecency multipliers. They don't just copy the installed index's settings.
    #[rstest]
    #[tokio::test]
    async fn rebuild_latest_settings_include_pending_changes(#[future(awt)] db: Sqlite) {
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let pending = shared.start_tagged(0).await;
        shared.rebuild_frecency(zero_frecency());

        let rebuilt = shared.rebuild(&db, |settings| Some(settings.latest.clone())).await.unwrap();
        assert!(matches!(rebuilt, RebuiltIndex::Installed(_)));
        assert_eq!(rebuilt.settings().shells, tag(0));
        assert_eq!(rebuilt.settings().frecency_multipliers, zero_frecency());
        drop(rebuilt);
        drop(pending);
    }
}
