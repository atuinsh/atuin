//! A [`SearchIndex`] that is shared between tasks. See [`SharedSearchIndex`].

use std::borrow::{Borrow, Cow};
use std::collections::HashSet;
use std::pin::pin;
use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId};
use futures::{Stream, TryStreamExt};
use parking_lot::{
    MappedRwLockReadGuard, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use time::OffsetDateTime;
use tracing::instrument;

use crate::search::index::core::{IndexIdentity, LoadFromDbError, RemainingRuns, SearchIndex};
use crate::search::index::{FrecencyMultipliers, SearchIndexSettings};

mod rebuild;

use rebuild::{PendingRebuild, Rebuild, RebuildStart};
pub use rebuild::{RebuildSettings, RebuiltIndex};

/// The state behind [`SharedSearchIndex`]'s lock.
#[derive(Debug, derive_more::AsRef, derive_more::AsMut)]
struct SharedState {
    /// The installed index.
    #[as_ref]
    #[as_mut]
    index: SearchIndex,

    /// The rebuilds that have started, oldest first. Each one stays here until it installs, is
    /// superseded, fails, or is dropped. An install also removes every older rebuild. Those
    /// rebuilds are then superseded.
    pending: Vec<Arc<PendingRebuild>>,

    /// The latest frecency multipliers that were asked for.
    ///
    /// A rebuild's start or [`SharedSearchIndex::rebuild_frecency`] sets these. Every install
    /// applies them. That way a change made while a rebuild runs isn't undone when it installs.
    frecency_multipliers: FrecencyMultipliers,

    /// Where the installed index is in its first load. See [`FillState`].
    fill: FillState,
}

impl SharedState {
    /// Get the settings that a rebuild started now would build on. See
    /// [`RebuildSettings::latest`].
    ///
    /// The result is borrowed unless the multipliers differ. Searches check these settings on every
    /// query, so the common case shouldn't clone them.
    fn latest_settings(&self) -> Cow<'_, SearchIndexSettings> {
        let settings = self.pending.last().map_or(self.index.settings(), |p| &p.settings);
        if settings.frecency_multipliers == self.frecency_multipliers {
            Cow::Borrowed(settings)
        } else {
            Cow::Owned(SearchIndexSettings {
                frecency_multipliers: self.frecency_multipliers,
                ..settings.clone()
            })
        }
    }

    /// Call the `settings` callback of a [`SharedSearchIndex::rebuild`] with this state's
    /// settings.
    fn call_settings(
        &self,
        settings: impl Fn(RebuildSettings<'_>) -> Option<SearchIndexSettings>,
    ) -> Option<SearchIndexSettings> {
        let latest = self.latest_settings();
        settings(RebuildSettings {
            installed: self.index.settings(),
            latest: &latest,
        })
    }
}

/// Where the installed index of a [`SharedSearchIndex`] is in its first load.
///
/// The state only moves forward:
///
/// * [`SharedSearchIndex::fill`] moves it from `Waiting` to `Filling` when the fill starts.
/// * The fill moves it from `Filling` to `Filled` when the fill ends.
/// * An install moves it from `Waiting` to `Filled`, because the installed index is then
///   complete. An install during a fill leaves it at `Filling`. The fill then stops at its next
///   page and moves the state on.
#[derive(Debug, Clone)]
enum FillState {
    /// The installed index is the empty one that [`SharedSearchIndex::unfilled`] made. It waits
    /// for its fill.
    ///
    /// Adds are ignored, because the fill's load reads those entries from the database. A pending
    /// rebuild still keeps them, because they register with it.
    Waiting,
    /// A fill is running. Permits record the ids of the entries they register here. The fill's
    /// load skips those entries, because their permits add them.
    Filling(Arc<Mutex<HashSet<HistoryId>>>),
    /// The installed index has its contents. Adds go straight into it.
    Filled,
}

/// A fill that has started. See [`SharedSearchIndex::fill`].
///
/// Dropping it ends the fill, so permits stop recording entries with it. Only one fill can ever
/// start, because only [`FillState::Waiting`] allows one.
struct Fill<'a> {
    shared: &'a SharedSearchIndex,
    /// The ids of entries that permits registered since the fill started.
    registered: Arc<Mutex<HashSet<HistoryId>>>,
    /// The identity of the installed index when the fill started.
    installed: IndexIdentity,
    /// When the fill started. Frecency is computed as of this time.
    now: OffsetDateTime,
}

impl Drop for Fill<'_> {
    fn drop(&mut self) {
        self.shared.state.write().fill = FillState::Filled;
    }
}

/// A [`SearchIndex`] that is shared between tasks.
///
/// The index can be rebuilt from the database while it is in use. Entries that are added during
/// the rebuild are not lost.
///
/// Entries that are written to the database are added through an [`AddPermit`]. The permit
/// registers the entries with any rebuild in progress before they are written. Before a rebuild
/// installs, it adds the registered entries that its database load didn't catch. As a result, every
/// entry ends up in the new index exactly once.
///
/// Rebuilds can overlap. A permit registers its entries with every rebuild that is pending. A
/// finished rebuild installs unless a newer rebuild installed first. In that case, the rebuild is
/// superseded. So an older index never replaces a newer one. A newer rebuild can install after an
/// older one. Its load started later, and it has every entry that was registered since it started.
/// The most recently started rebuild that finishes its load ends up installed.
#[derive(Debug)]
pub struct SharedSearchIndex {
    state: RwLock<SharedState>,
    /// An [`AddPermit`] holds this lock for reading. It holds the lock from before its entries are
    /// written to the database until they are added.
    ///
    /// A rebuild takes the lock for writing when it starts and again when it installs. So no add
    /// is in progress at either point.
    add_lock: tokio::sync::RwLock<()>,
}

impl From<SearchIndex> for SharedSearchIndex {
    fn from(index: SearchIndex) -> Self {
        Self {
            state: RwLock::new(SharedState {
                frecency_multipliers: index.settings().frecency_multipliers,
                index,
                pending: Vec::new(),
                fill: FillState::Filled,
            }),
            add_lock: tokio::sync::RwLock::new(()),
        }
    }
}

impl SharedSearchIndex {
    /// Create an empty index that waits for [`Self::fill`] to load it.
    ///
    /// Adds are ignored until the fill starts. The fill's load reads those entries from the
    /// database, so ignoring them keeps each entry from being counted twice.
    #[must_use]
    pub fn unfilled(settings: SearchIndexSettings) -> Self {
        let shared = Self::from(SearchIndex::new(settings));
        shared.state.write().fill = FillState::Waiting;
        shared
    }

    /// Lock the installed index for reading.
    pub fn read(&self) -> MappedRwLockReadGuard<'_, SearchIndex> {
        RwLockReadGuard::map(self.state.read(), |state| &state.index)
    }

    /// Load every history entry from `db` into the installed index, one page at a time.
    ///
    /// Use this to fill an [unfilled][`Self::unfilled`] index. Compared to a rebuild, which would
    /// keep the empty index installed until its whole load finishes, here searches see each page as
    /// soon as it is added. Pages are read newest first, so recent history shows up first.
    ///
    /// Entries that are added through an [`AddPermit`] end up in the index exactly once. That holds
    /// both before and during the load. If a rebuild installs another index during the load, the
    /// load stops, because that index is complete. The load doesn't run at all if the index isn't
    /// waiting for a fill. For example, a rebuild may have installed first, or a fill already ran.
    pub async fn fill(&self, db: &Sqlite) -> Result<(), LoadFromDbError> {
        self.fill_from(SearchIndex::history_pages(db)).await
    }

    /// The core of [`Self::fill`], using the history pages from `pages`.
    async fn fill_from(
        &self,
        pages: impl Stream<Item = Result<Vec<History>, LoadFromDbError>>,
    ) -> Result<(), LoadFromDbError> {
        let Some(fill) = self.start_fill().await else {
            tracing::info!("The search index isn't waiting for a fill; skipping the load");
            return Ok(());
        };
        let mut pages = pin!(pages);
        while let Some(page) = pages.try_next().await? {
            let mut state = self.state.write();
            if state.index.identity != fill.installed {
                tracing::info!("A rebuild installed a complete search index; stopping the load");
                break;
            }
            // A permit adds each registered entry, so the load skips it.
            let registered = fill.registered.lock();
            let unregistered = page.iter().filter(|history| !registered.contains(&history.id));
            state.index.add_history_at(unregistered, fill.now);
        }
        Ok(())
    }

    /// Rebuild the index from `db`, unless `settings` says no rebuild is needed.
    ///
    /// `settings` receives the installed and latest settings. See [`RebuildSettings`]. It returns
    /// the settings to rebuild with. It returns [`None`] to keep the installed index as it is. It
    /// may be called twice under the index's lock, so it should be quick and have no side effects.
    ///
    /// If `settings` returns [`None`], this returns the installed index, locked for reading. That
    /// way the caller uses the index that it checked.
    ///
    /// Otherwise the rebuilt index is installed and returned locked for reading. The caller then
    /// uses that index even if another rebuild installs right after. The old index is freed on a
    /// blocking thread.
    ///
    /// The rebuilt index isn't installed if a newer rebuild installed while this one was running.
    /// Then the rebuilt index is returned without a lock. That way the caller still gets an index
    /// with its settings.
    ///
    /// The installed index stays live until the rebuilt one replaces it. Searches and adds use it
    /// as usual in the meantime. Entries that are added through an [`AddPermit`] during the rebuild
    /// are kept. If the load from `db` fails, the installed index is kept.
    pub async fn rebuild(
        &self,
        db: &Sqlite,
        settings: impl Fn(RebuildSettings<'_>) -> Option<SearchIndexSettings>,
    ) -> Result<RebuiltIndex<'_>, LoadFromDbError> {
        let rebuild = match self.start_rebuild(settings).await {
            RebuildStart::NotNeeded(index) => return Ok(RebuiltIndex::Installed(index)),
            RebuildStart::Started(rebuild) => rebuild,
        };
        tracing::info!("Rebuilding search index from database");
        let new_index = rebuild.load(db).await?;
        tracing::info!(
            "Search index rebuild loaded; {} unique commands",
            new_index.command_count()
        );
        Ok(rebuild.install(new_index).await)
    }

    /// Start a fill, if the installed index is waiting for one.
    async fn start_fill(&self) -> Option<Fill<'_>> {
        // Wait for adds in progress to finish. Those adds were ignored, and their entries are in
        // the database before the load starts. Permits taken after this point record their
        // entries with the fill.
        let _add = self.add_lock.write().await;
        let mut state = self.state.write();
        if !matches!(state.fill, FillState::Waiting) {
            return None;
        }
        let registered = Arc::new(Mutex::default());
        state.fill = FillState::Filling(registered.clone());
        Some(Fill {
            shared: self,
            registered,
            installed: state.index.identity.clone(),
            now: OffsetDateTime::now_utc(),
        })
    }

    /// Start a rebuild, unless `settings` says that it's not needed. See [`Self::rebuild`].
    async fn start_rebuild(
        &self,
        settings: impl Fn(RebuildSettings<'_>) -> Option<SearchIndexSettings>,
    ) -> RebuildStart<'_> {
        // Check without `add_lock` first. That way a caller that needs no rebuild never waits for
        // adds.
        {
            let state = self.state.read();
            if state.call_settings(&settings).is_none() {
                return RebuildStart::NotNeeded(RwLockReadGuard::map(state, |s| &s.index));
            }
        }

        // Wait for adds in progress to finish. Then every entry that was added before this point is
        // in the database before the load starts. Adds after this point register with this
        // rebuild.
        let add = self.add_lock.write().await;
        let mut state = self.state.write();
        // Check again, because the state may have changed while this waited.
        let Some(settings) = state.call_settings(&settings) else {
            drop(add);
            let state = RwLockWriteGuard::downgrade(state);
            return RebuildStart::NotNeeded(RwLockReadGuard::map(state, |s| &s.index));
        };
        state.frecency_multipliers = settings.frecency_multipliers;
        let pending = Arc::new(PendingRebuild {
            settings,
            added: Mutex::default(),
        });
        state.pending.push(pending.clone());
        RebuildStart::Started(Rebuild::new(self, pending))
    }

    /// Get permission to add entries to the index. See [`AddPermit`].
    ///
    /// Waits while a rebuild is starting or installing.
    pub async fn begin_add(&self) -> AddPermit<'_> {
        let add = self.add_lock.read().await;
        // A rebuild or a fill can't start while `add` is held. So these stay the ones to register
        // with. If a rebuild is abandoned or the fill ends in the meantime, that rebuild or fill
        // just collects entries that it won't use.
        let state = self.state.read();
        let (pending, fill) = (state.pending.clone(), state.fill.clone());
        drop(state);
        AddPermit {
            index: self,
            pending,
            fill,
            _add: add,
        }
    }

    /// Get permission to update commands in the installed index in place. Use it after some of
    /// their runs were deleted from the database. See [`SyncPermit`].
    ///
    /// `settings` is the callback that a [rebuild](Self::rebuild) would get. The sync must leave
    /// the index as that rebuild would. This returns [`None`] when it can't:
    ///
    /// * `settings` asks for settings that differ from the installed index's.
    /// * A rebuild is pending. Its load may have read the deleted runs. Its install would then
    ///   bring them back.
    /// * The fill hasn't finished. Its load may read the deleted runs too.
    ///
    /// Rebuild the index instead in those cases.
    ///
    /// Waits for adds in progress to finish. New adds, rebuilds and the fill then wait until the
    /// permit is dropped.
    pub async fn begin_sync(
        &self,
        settings: impl Fn(RebuildSettings<'_>) -> Option<SearchIndexSettings>,
    ) -> Option<SyncPermit<'_>> {
        let add = self.add_lock.write().await;
        let state = self.state.read();
        if !state.pending.is_empty() || !matches!(state.fill, FillState::Filled) {
            return None;
        }
        let installed = state.index.settings();
        if state.call_settings(settings).is_some_and(|settings| settings != *installed) {
            return None;
        }
        let settings = installed.clone();
        drop(state);
        Some(SyncPermit {
            index: self,
            settings,
            _add: add,
        })
    }

    /// Recompute the global frecency of every command.
    ///
    /// Searches can still run while the new frecency is computed. A rebuild that runs at the same
    /// time also installs with `multipliers`. See [`SearchIndex::rebuild_frecency`].
    #[instrument(skip_all, level = tracing::Level::DEBUG, name = "rebuild_frecency")]
    pub fn rebuild_frecency(&self, multipliers: FrecencyMultipliers) {
        SearchIndex::rebuild_frecency_shared(&self.state, multipliers, |state| {
            state.frecency_multipliers = multipliers;
        });
    }
}

/// Permission to add history entries to a [`SharedSearchIndex`].
///
/// A caller holds the permit from before the entries are written to the database until they are
/// added.
///
/// Every caller that writes history to the database and adds it to the index does this:
///
/// 1. Get a permit with [`SharedSearchIndex::begin_add`].
/// 2. [Register](Self::register) the entries before writing them.
/// 3. Write them to the database.
/// 4. [Unregister](Self::unregister) any entries that won't be added.
/// 5. [Add](Self::add_history) the rest.
///
/// A registered entry is kept in an index that is being rebuilt. That holds even if the rebuild's
/// load doesn't read the entry from the database. Sometimes an entry is registered but neither
/// added nor unregistered. For example, the caller may return early. A rebuild still keeps that
/// entry. That is the right result when the entry was written.
///
/// Hold a permit only across the database write and the add. Don't get a second permit while you
/// hold one, and don't rebuild the index either. A rebuild that is waiting to start would wait for
/// this permit. The second permit or rebuild would then wait for that rebuild.
#[must_use = "a permit adds nothing until its entries are added"]
pub struct AddPermit<'a> {
    index: &'a SharedSearchIndex,

    /// The rebuilds that were pending when the permit was taken. A new rebuild can't start while
    /// the permit is held.
    pending: Vec<Arc<PendingRebuild>>,

    /// The fill state when the permit was taken.
    ///
    /// It can't move on from `Waiting` while the permit is held, because only a fill's start or
    /// an install moves it on, and both of those wait for the add lock. A fill can end while the
    /// permit is held, though. The permit then still registers ids in the fill's set. No fill
    /// reads that set any more, so those ids have no effect.
    fill: FillState,
    _add: tokio::sync::RwLockReadGuard<'a, ()>,
}

impl AddPermit<'_> {
    /// Register entries that are about to be written to the database. They are registered with
    /// every pending rebuild and with the fill in progress, if there is one.
    pub fn register<H>(&self, histories: H)
    where
        H: IntoIterator<Item: Borrow<History>>,
    {
        let mut filling = self.filling();
        for history in histories {
            let history = history.borrow();
            if !self.pending.is_empty() {
                // The pending rebuilds share one copy of the entry.
                let shared = Arc::new(history.clone());
                for pending in &self.pending {
                    pending.added.lock().insert(history.id, shared.clone());
                }
            }
            if let Some(filling) = &mut filling {
                filling.insert(history.id);
            }
        }
    }

    /// Lock the ids of the running fill, if a fill was running when the permit was taken.
    fn filling(&self) -> Option<MutexGuard<'_, HashSet<HistoryId>>> {
        match &self.fill {
            FillState::Filling(registered) => Some(registered.lock()),
            FillState::Waiting | FillState::Filled => None,
        }
    }

    /// Unregister entries that this permit won't add. For example, writing them may have failed.
    pub fn unregister(&self, ids: impl IntoIterator<Item = HistoryId>) {
        let mut filling = self.filling();
        for id in ids {
            for pending in &self.pending {
                pending.added.lock().remove(&id);
            }
            if let Some(filling) = &mut filling {
                filling.remove(&id);
            }
        }
    }

    /// Add the registered entries to the installed index, now that they are in the database.
    ///
    /// Frecency is computed as of now.
    pub fn add_history<H>(self, histories: H)
    where
        H: IntoIterator<Item: Borrow<History>>,
    {
        // An index that is waiting for its fill ignores adds. The fill's load reads these entries
        // from the database.
        if matches!(self.fill, FillState::Waiting) {
            return;
        }
        SearchIndex::add_history_shared(&self.index.state, histories, OffsetDateTime::now_utc());
    }
}

/// Permission to update commands in a [`SharedSearchIndex`] in place. It is used after some of
/// their runs were deleted from the database. See [`SharedSearchIndex::begin_sync`].
///
/// A caller does this while it holds the permit:
///
/// 1. For each command, fold the runs that it has left in the database into a [`RemainingRuns`]
///    with [`Self::add_run`].
/// 2. [Sync](Self::sync) the commands.
///
/// No entry is added while the permit is held. So a run that is written meanwhile is neither lost
/// nor counted twice. Hold the permit only across the database reads and the sync. Don't get an
/// [`AddPermit`] while you hold one, and don't rebuild the index either. Both would wait for this
/// permit.
#[must_use = "a permit changes nothing until it syncs"]
pub struct SyncPermit<'a> {
    index: &'a SharedSearchIndex,

    /// The installed index's settings when the permit was taken. Only an install can change their
    /// shell filter. No install can happen while the permit is held.
    settings: SearchIndexSettings,
    _add: tokio::sync::RwLockWriteGuard<'a, ()>,
}

impl SyncPermit<'_> {
    /// Fold `history`, one of the runs that a command has left in the database, into `runs`.
    pub fn add_run(&self, runs: &mut RemainingRuns, history: &History) {
        runs.add(history, &self.settings);
    }

    /// Make the installed index agree with the database about each command in `commands`. A
    /// command that has no runs left is removed. Frecency is computed as of now.
    pub fn sync(self, commands: impl IntoIterator<Item = RemainingRuns>) {
        self.index.state.write().index.sync_commands(commands, OffsetDateTime::now_utc());
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::filter::OrFilter;
    use futures::stream;
    use rstest::rstest;

    use super::*;
    use crate::search::index::core::tests::*;

    impl SharedSearchIndex {
        /// Start a rebuild with the latest settings.
        pub(super) async fn start(&self) -> Rebuild<'_> {
            let settings = |settings: RebuildSettings<'_>| Some(settings.latest.clone());
            let RebuildStart::Started(rebuild) = self.start_rebuild(settings).await else {
                panic!("rebuild didn't start");
            };
            rebuild
        }

        /// Start a rebuild with the latest settings, but with `tag(tag)` as the shell filter.
        pub(super) async fn start_tagged(&self, tag: usize) -> Rebuild<'_> {
            let settings = |settings: RebuildSettings<'_>| {
                Some(SearchIndexSettings {
                    shells: self::tag(tag),
                    ..settings.latest.clone()
                })
            };
            let RebuildStart::Started(rebuild) = self.start_rebuild(settings).await else {
                panic!("rebuild {tag} didn't start");
            };
            rebuild
        }
    }

    /// A filter with a single shell. The tests use it to tell the indexes of different rebuilds
    /// apart.
    pub(super) fn tag(rebuild: usize) -> OrFilter<Vec<String>> {
        OrFilter::from_list(vec![format!("shell{rebuild}")]).unwrap()
    }

    /// Add `history` through a permit, the way the daemon does. If `db` is given, `history` is
    /// also written to it.
    pub(super) async fn add_through_permit(
        shared: &SharedSearchIndex,
        db: Option<&Sqlite>,
        history: &History,
    ) {
        let permit = shared.begin_add().await;
        permit.register([history]);
        if let Some(db) = db {
            db.save(history).await.unwrap();
        }
        permit.add_history([history]);
    }

    /// The shared index adds entries and rebuilds frecency the same way that the plain index
    /// does.
    #[rstest]
    #[tokio::test]
    async fn shared_index_operations() {
        let now = OffsetDateTime::now_utc();
        let shared = SharedSearchIndex::from(SearchIndex::default());

        for command in ["echo one", "echo two"] {
            add_through_permit(&shared, None, &make_history(command, "/tmp", now)).await;
        }
        assert_eq!(shared.read().command_count(), 2);
        assert!(frecency(&shared.read(), "echo one") > 0);

        shared.rebuild_frecency(zero_frecency());
        assert_eq!(shared.read().settings().frecency_multipliers, zero_frecency());
        assert_eq!(frecency(&shared.read(), "echo one"), 0);
    }

    /// Pages for [`SharedSearchIndex::fill_from`] that the test sends one at a time.
    fn page_channel() -> (
        tokio::sync::mpsc::UnboundedSender<Vec<History>>,
        impl Stream<Item = Result<Vec<History>, LoadFromDbError>>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let pages = stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|page| (Ok(page), receiver))
        });
        (sender, pages)
    }

    /// Yield until `done` returns true. A load that runs alongside the caller can then catch up.
    async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..100 {
            if done() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the load never caught up");
    }

    /// Whether the installed index of `shared` has `command`.
    fn has(shared: &SharedSearchIndex, command: &str) -> bool {
        contains(&shared.read(), command)
    }

    /// Searches during a fill see each page as soon as it is added. Permits stop recording entries
    /// with the fill once it finishes.
    #[rstest]
    #[tokio::test]
    async fn fill_shows_each_page_as_it_loads() {
        let shared = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        let (sender, pages) = page_channel();
        let now = OffsetDateTime::now_utc();

        let shared_ref = &shared;
        let feed = async move {
            sender.send(vec![make_history("echo newest", "/tmp", now)]).unwrap();
            until(|| has(shared_ref, "echo newest")).await;
            assert_eq!(shared_ref.read().command_count(), 1);

            sender.send(vec![make_history("echo oldest", "/tmp", now)]).unwrap();
            until(|| has(shared_ref, "echo oldest")).await;
            assert_eq!(shared_ref.read().command_count(), 2);
        };
        let (filled, ()) = tokio::join!(shared.fill_from(pages), feed);
        filled.unwrap();
        assert!(matches!(shared.state.read().fill, FillState::Filled));
    }

    /// Entries that are added through permits during a fill end up in the index exactly once.
    ///
    /// That holds whether a permit adds an entry before or after the load reads it. It also holds
    /// while rebuilds start during the fill. For example, an entry may register with one rebuild
    /// but not with a rebuild that starts after it.
    #[rstest]
    #[tokio::test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: the permit is consumed by add_history once the load has read it"
    )]
    async fn fill_counts_entries_added_meanwhile_once() {
        let shared = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        let (sender, pages) = page_channel();
        let now = OffsetDateTime::now_utc();
        let history = |command: &str| make_history(command, "/tmp", now);
        let (before, after, replaced) =
            (history("echo before"), history("echo after"), history("echo replaced"));

        let shared_ref = &shared;
        let feed = async move {
            until(|| matches!(shared_ref.state.read().fill, FillState::Filling(_))).await;
            // This permit adds its entry before the load reads it.
            add_through_permit(shared_ref, None, &before).await;
            // This permit adds its entry after the load reads it.
            let permit = shared_ref.begin_add().await;
            permit.register([&after]);
            sender.send(vec![before.clone(), after.clone(), history("echo first")]).unwrap();
            until(|| has(shared_ref, "echo first")).await;
            permit.add_history([&after]);

            // This entry registers only with the first rebuild. The second rebuild starts after
            // the add. It starts before the load reads the entry.
            let first = shared_ref.start().await;
            add_through_permit(shared_ref, None, &replaced).await;
            let second = shared_ref.start().await;
            sender.send(vec![replaced.clone(), history("echo second")]).unwrap();
            until(|| has(shared_ref, "echo second")).await;
            drop((first, second));
        };
        let (filled, ()) = tokio::join!(shared.fill_from(pages), feed);
        filled.unwrap();

        let index = shared.read();
        for name in ["echo before", "echo after", "echo replaced", "echo first", "echo second"] {
            assert_eq!(count(&index, name), 1, "{name}");
        }
        drop(index);
    }

    /// A fill stops adding pages once a rebuild installs a complete index.
    #[rstest]
    #[tokio::test]
    async fn fill_stops_once_a_rebuild_installs(#[future(awt)] db: Sqlite) {
        let now = OffsetDateTime::now_utc();
        db.save(&make_history("echo saved", "/tmp", now)).await.unwrap();
        let shared = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        let (sender, pages) = page_channel();

        let shared_ref = &shared;
        let feed = async move {
            sender.send(vec![make_history("echo early", "/tmp", now)]).unwrap();
            until(|| has(shared_ref, "echo early")).await;
            let rebuilt = shared_ref.rebuild(&db, |settings| Some(settings.latest.clone()));
            assert!(matches!(rebuilt.await.unwrap(), RebuiltIndex::Installed(_)));
            sender.send(vec![make_history("echo late", "/tmp", now)]).unwrap();
        };
        let (filled, ()) = tokio::join!(shared.fill_from(pages), feed);
        filled.unwrap();

        assert!(has(&shared, "echo saved"), "the rebuilt index is installed");
        assert!(!has(&shared, "echo early"), "the rebuild replaced the filled index");
        assert!(!has(&shared, "echo late"), "the fill stopped once the rebuild installed");
    }

    /// An unfilled index ignores adds until its fill starts. The fill's load then counts each of
    /// those entries once, from the database.
    #[rstest]
    #[tokio::test]
    async fn adds_before_the_fill_are_left_to_its_load() {
        let shared = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        let early = make_history("echo early", "/tmp", OffsetDateTime::now_utc());
        add_through_permit(&shared, None, &early).await;
        assert!(!has(&shared, "echo early"), "the add was ignored");

        let pages = stream::iter([Ok(vec![early.clone()])]);
        shared.fill_from(pages).await.unwrap();
        assert_eq!(count(&shared.read(), "echo early"), 1);
    }

    /// An add during a rebuild that started before the fill is ignored by the unfilled index. If
    /// the rebuild fails, the fill's load counts the entry once.
    #[rstest]
    #[tokio::test]
    async fn adds_during_a_failed_rebuild_before_the_fill_are_counted_once() {
        let shared = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        let early = make_history("echo early", "/tmp", OffsetDateTime::now_utc());
        let rebuild = shared.start().await;
        add_through_permit(&shared, None, &early).await;
        assert!(!has(&shared, "echo early"), "the unfilled index ignored the add");
        drop(rebuild);

        shared.fill_from(stream::iter([Ok(vec![early.clone()])])).await.unwrap();
        assert_eq!(count(&shared.read(), "echo early"), 1);
    }

    /// A rebuild that installs before the fill starts leaves the fill nothing to do. Adds after
    /// that install go through.
    #[rstest]
    #[tokio::test]
    async fn fill_skips_an_index_that_a_rebuild_installed(#[future(awt)] db: Sqlite) {
        let now = OffsetDateTime::now_utc();
        db.save(&make_history("echo saved", "/tmp", now)).await.unwrap();
        let shared = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        let rebuilt = shared.rebuild(&db, |settings| Some(settings.latest.clone())).await.unwrap();
        drop(rebuilt);

        add_through_permit(&shared, None, &make_history("echo after", "/tmp", now)).await;
        assert!(has(&shared, "echo after"), "adds go through once a rebuild has installed");

        let pages = stream::iter([Ok(vec![make_history("echo saved", "/tmp", now)])]);
        shared.fill_from(pages).await.unwrap();
        assert_eq!(count(&shared.read(), "echo saved"), 1, "the fill didn't load again");
    }

    /// A fill whose load fails still stops the index from ignoring adds.
    #[rstest]
    #[tokio::test]
    async fn adds_go_through_after_a_failed_fill() {
        let shared = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        let pages = stream::iter([Err(load_failure())]);
        assert!(shared.fill_from(pages).await.is_err());
        assert!(matches!(shared.state.read().fill, FillState::Filled));

        add_through_permit(
            &shared,
            None,
            &make_history("echo after", "/tmp", OffsetDateTime::now_utc()),
        )
        .await;
        assert!(has(&shared, "echo after"));
    }

    /// A sync starts only when it would leave the index as a rebuild would. It doesn't start while
    /// a rebuild is pending or before the fill finished. It doesn't start either when the rebuild
    /// would change the settings.
    #[rstest]
    #[tokio::test]
    async fn begin_sync_only_when_it_matches_a_rebuild(#[future(awt)] db: Sqlite) {
        let latest = |settings: RebuildSettings<'_>| Some(settings.latest.clone());
        let shared = SharedSearchIndex::from(SearchIndex::default());
        assert!(shared.begin_sync(latest).await.is_some());
        assert!(shared.begin_sync(|_| None).await.is_some(), "no rebuild is needed");

        let zero = |settings: RebuildSettings<'_>| {
            Some(SearchIndexSettings {
                frecency_multipliers: zero_frecency(),
                ..settings.latest.clone()
            })
        };
        assert!(shared.begin_sync(zero).await.is_none(), "a rebuild would change the multipliers");

        let rebuild = shared.start().await;
        assert!(shared.begin_sync(latest).await.is_none(), "a rebuild is pending");
        let new_index = rebuild.load(&db).await.unwrap();
        drop(rebuild.install(new_index).await);
        assert!(shared.begin_sync(latest).await.is_some(), "the rebuild installed");

        let unfilled = SharedSearchIndex::unfilled(SearchIndexSettings::default());
        assert!(unfilled.begin_sync(latest).await.is_none(), "the fill hasn't run");
        unfilled.fill(&db).await.unwrap();
        assert!(unfilled.begin_sync(latest).await.is_some(), "the fill finished");
    }

    /// A sync waits for the adds in progress, and new adds wait for the sync. So no run is added
    /// between the sync's reads of the database and its update of the index.
    #[rstest]
    #[tokio::test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: clippy wants drop(sync) after it already got moved"
    )]
    async fn syncs_and_adds_take_turns(#[future(awt)] db: Sqlite) {
        let latest = |settings: RebuildSettings<'_>| Some(settings.latest.clone());
        let wait = std::time::Duration::from_millis(50);
        let shared = SharedSearchIndex::from(SearchIndex::default());
        let now = OffsetDateTime::now_utc();
        add_through_permit(&shared, Some(&db), &make_history("echo kept", "/tmp", now)).await;

        let add = shared.begin_add().await;
        assert!(tokio::time::timeout(wait, shared.begin_sync(latest)).await.is_err());
        drop(add);

        let rows: Vec<History> = db.history_of_command("echo kept").try_collect().await.unwrap();
        let sync = shared.begin_sync(latest).await.unwrap();
        assert!(tokio::time::timeout(wait, shared.begin_add()).await.is_err());
        let mut runs = RemainingRuns::new("echo kept".to_owned());
        for row in &rows {
            sync.add_run(&mut runs, row);
        }
        sync.sync([runs]);
        assert_eq!(count(&shared.read(), "echo kept"), 1);

        add_through_permit(&shared, Some(&db), &make_history("echo kept", "/tmp", now)).await;
        assert_eq!(count(&shared.read(), "echo kept"), 2);
    }
}
