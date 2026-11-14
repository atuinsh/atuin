//! Search index with frecency-based ranking.
//!
//! This module provides a deduplicated search index where each unique command
//! is stored once, with metadata about all its invocations. This enables:
//!
//! - Efficient fuzzy matching (fewer items to match)
//! - Frecency-based ranking (frequency + recency)
//! - Dynamic filtering by directory, host, session, etc.

use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::pin::pin;
use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId, ShellSession};
use atuin_common::string::StrExt;
use easy_cast::Conv;
use fixedbitset::FixedBitSet;
use futures::{Stream, TryStreamExt, stream};
use lasso::Rodeo;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use time::OffsetDateTime;
use tracing::{Level, instrument};

use crate::search::index::{FrecencyMultipliers, SearchIndexSettings};

mod command;
mod filter;
mod session_preload;

use command::CommandData;
use filter::CompiledFilter;
pub use filter::IndexFilterMode;
use session_preload::SessionPreload;
pub use session_preload::{LoadedSearch, PendingSearch};

/// Error trying to load history from the database into the search index.
#[derive(Debug, thiserror::Error)]
#[error("failed to load history from the database: {0}")]
pub struct LoadFromDbError(eyre::Report);

/// Identifies a [`SearchIndex`]. A fill or a [`PendingSearch`] uses it to tell whether the
/// installed index is still the one that it began on.
///
/// Clones of an identity are equal. Identities that were created separately are not.
#[derive(Debug, Clone)]
pub(super) struct IndexIdentity(Arc<()>);

impl Default for IndexIdentity {
    fn default() -> Self {
        Self(Arc::new(()))
    }
}

impl PartialEq for IndexIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for IndexIdentity {}

/// A deduplicated search index with frecency-based ranking.
///
/// Commands are stored by their text, with metadata about all invocations. All invocations of a
/// command share a single `CommandData`. Frizbee handles fuzzy matching. Results are ranked by
/// match quality, and frecency breaks ties between equally good matches.
///
/// Global frecency is computed when a command is added. A background task also recomputes it
/// periodically, so that scores decay.
///
/// To share an index between tasks, wrap it in a [`SharedSearchIndex`](super::SharedSearchIndex).
#[derive(Debug, Default)]
pub struct SearchIndex {
    /// Identifies this index. See [`IndexIdentity`].
    pub(super) identity: IndexIdentity,

    /// Map from command text to the command's index.
    indices: HashMap<Arc<str>, u32>,

    /// The fuzzy matcher's haystack: the [normalized](StrExt::normalize_diacritics) text of each
    /// command, with diacritics removed. Shares the `Arc<str>` allocation with the key in
    /// `indices` when normalization changes nothing.
    ///
    /// Stored in insertion order.
    haystack: Vec<Arc<str>>,

    /// Data for each command.
    ///
    /// Stored in insertion order -- aligned with [`Self::haystack`].
    data: Vec<CommandData>,

    /// Frecency score of each command.
    ///
    /// Stored in insertion order -- aligned with [`Self::haystack`].
    frecency: Vec<u32>,

    /// The index's settings. See [`Self::settings`].
    settings: SearchIndexSettings,

    /// String interner for deduplicating cwd, hostname, and directory paths.
    interner: Rodeo,

    /// Counts the changes to the index's commands: one for every entry added and one for every
    /// [sync](Self::sync_commands). A [`PendingSearch`] compares it to tell whether the index
    /// changed since the search began.
    changes: u64,

    /// Cache for the most recent session used with the "session-preload" filter.
    ///
    /// It is in a `Mutex` because a search only reads the index but still needs to fill the
    /// cache.
    session_preload: Mutex<Option<SessionPreload>>,
}

impl SearchIndex {
    /// Create a new empty search index.
    #[must_use]
    pub fn new(settings: SearchIndexSettings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }

    /// The index's settings.
    #[must_use]
    pub const fn settings(&self) -> &SearchIndexSettings {
        &self.settings
    }

    /// Number of history rows loaded per page when (re)building an index from the database.
    pub const HISTORY_LOAD_PAGE_SIZE: usize = 5000;

    /// Load every history entry from `db` into this index, computing frecency as of `now`.
    ///
    /// Each page is passed to `on_page` before it is added.
    pub(super) async fn load_from_db_at(
        &mut self,
        db: &Sqlite,
        now: OffsetDateTime,
        mut on_page: impl FnMut(&[History]),
    ) -> Result<(), LoadFromDbError> {
        let mut pages = pin!(Self::history_pages(db));
        while let Some(histories) = pages.try_next().await? {
            on_page(&histories);
            self.add_history_at(&histories, now);
        }
        Ok(())
    }

    /// Stream every history entry in `db`, one page at a time.
    pub(super) fn history_pages(
        db: &Sqlite,
    ) -> impl Stream<Item = Result<Vec<History>, LoadFromDbError>> {
        stream::try_unfold(
            db.all_paged(Self::HISTORY_LOAD_PAGE_SIZE, false),
            |mut pager| async move {
                match pager.next().await {
                    Ok(Some(histories)) => Ok(Some((histories, pager))),
                    Ok(None) => Ok(None),
                    Err(e) => Err(LoadFromDbError(e.into())),
                }
            },
        )
    }

    /// Add history entries to the index, computing frecency as of `now`.
    ///
    /// If an entry's command is already in the index, this updates the command's invocation data.
    /// Otherwise it adds the command.
    ///
    /// Some callers add entries over time as part of one operation. For example, a load from the
    /// database adds one page at a time. Those callers should pass the same `now` for every entry.
    /// Otherwise an entry that is added later is scored as older than it really is. It can then
    /// rank below an entry that is actually older but was added earlier.
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_add_histories")]
    pub fn add_history_at<H>(&mut self, histories: H, now: OffsetDateTime)
    where
        H: IntoIterator<Item: Borrow<History>>,
    {
        let mut total = 0;
        let mut added = 0;
        let now = now.unix_timestamp();

        for history in histories {
            if self.add(history.borrow(), now) {
                added += 1;
            }
            total += 1;
        }

        tracing::trace!("added {added}/{total} histories to search index");
    }

    /// Like [`Self::add_history_at`] but takes an index behind an [`RwLock`].
    ///
    /// The write lock is taken for each entry rather than for the whole batch. That way searches
    /// can run in between.
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_add_histories")]
    pub(super) fn add_history_shared<T, H>(lock: &RwLock<T>, histories: H, now: OffsetDateTime)
    where
        T: AsMut<Self>,
        H: IntoIterator<Item: Borrow<History>>,
    {
        let mut total = 0;
        let mut added = 0;
        let now = now.unix_timestamp();

        for history in histories {
            if lock.write().as_mut().add(history.borrow(), now) {
                added += 1;
            }
            total += 1;
        }

        tracing::trace!("added {added}/{total} histories to search index");
    }

    /// Add a history entry, and compute its command's frecency as of `now` in Unix seconds.
    ///
    /// Returns whether the entry was indexed. Some entries are skipped:
    ///
    /// * entries that an agent ran
    /// * tombstones from older versions, which have `deleted_at` set
    /// * entries from shells that the index doesn't include
    /// * entries with invalid session UUIDs
    fn add(&mut self, history: &History, now: i64) -> bool {
        let Some(command_index) = self.add_command(history, now) else {
            return false;
        };
        self.changes += 1;
        if let Some(preload) = self.session_preload.get_mut() {
            preload.record(command_index, history);
        }
        true
    }

    /// The core of [`Self::add`], without the bookkeeping for session preloads.
    ///
    /// Returns the index of the entry's command in [`Self::data`], if the entry was indexed.
    fn add_command(&mut self, history: &History, now: i64) -> Option<u32> {
        if !self.settings.includes(history) {
            return None;
        }

        let command = history.command.as_str();
        if let Some(&index) = self.indices.get(command) {
            // Existing command - update invocations and frecency
            let data = &mut self.data[usize::conv(index)];
            if !data.add_invocation(history, &mut self.interner) {
                return None;
            }
            self.frecency[usize::conv(index)] =
                data.compute_frecency(now, self.settings.frecency_multipliers);
            return Some(index);
        }

        // Skip invalid commands
        let data = CommandData::new(history, &mut self.interner)?;
        self.push_command(Arc::from(command), data, now)
    }

    /// Add `command`, which the index doesn't have yet, with `data`. Its frecency is computed as of
    /// `now` in Unix seconds.
    ///
    /// Returns the index of the command in [`Self::data`], if it was added.
    fn push_command(&mut self, command: Arc<str>, data: CommandData, now: i64) -> Option<u32> {
        let Ok(index) = u32::try_from(self.data.len()) else {
            // TODO: It's very unlikely that we'll have more than 2^32 history entries, but if that
            // ends up being a realistic possibility, we should handle this case better than simply
            // dropping new commands from the index.
            return None;
        };

        let normalized = match command.normalize_diacritics() {
            Cow::Borrowed(_) => command.clone(),
            Cow::Owned(normalized) => normalized.into(),
        };

        self.indices.insert(command, index);
        self.haystack.push(normalized);
        self.frecency.push(data.compute_frecency(now, self.settings.frecency_multipliers));
        self.data.push(data);
        Some(index)
    }

    /// Make the index agree with the database about each command in `commands`. Each command comes
    /// with the runs that it has left there. Frecency is computed as of `now`.
    ///
    /// This lets a delete update the index without reloading all of history. [`CommandData`] keeps
    /// only aggregates, so one run can't be subtracted from it. But a command can be rebuilt from
    /// the runs that it has left. A command that has no runs left is removed. The commands after
    /// it move down to fill the gap.
    #[instrument(skip_all, level = tracing::Level::DEBUG, name = "index_sync_commands")]
    pub(super) fn sync_commands<C>(&mut self, commands: C, now: OffsetDateTime)
    where
        C: IntoIterator<Item = RemainingRuns>,
    {
        let now = now.unix_timestamp();
        let multipliers = self.settings.frecency_multipliers;
        let mut removed = FixedBitSet::with_capacity(self.data.len());

        for runs in commands {
            let RemainingRuns {
                command,
                data,
                interner,
            } = runs;
            let data = data.map(|mut data| {
                data.reintern(&interner, &mut self.interner);
                data
            });
            match (self.indices.get(command.as_str()).copied(), data) {
                (Some(index), Some(data)) => {
                    let index = usize::conv(index);
                    self.frecency[index] = data.compute_frecency(now, multipliers);
                    self.data[index] = data;
                }
                (Some(index), None) => removed.insert(usize::conv(index)),
                // Runs that the index didn't have. A reload would add them.
                (None, Some(data)) => {
                    self.push_command(command.into(), data, now);
                }
                (None, None) => {}
            }
        }

        if !removed.is_clear() {
            self.remove_commands(&removed);
        }
        // The cached preload knows commands by their index. A removal moves those indices. The
        // runs of these commands changed too.
        *self.session_preload.get_mut() = None;
        self.changes += 1;
    }

    /// Remove the commands at the indices in `removed`. The commands after each one move down to
    /// fill the gap.
    fn remove_commands(&mut self, removed: &FixedBitSet) {
        // The new index of each command, by its old index.
        let mut kept = 0;
        let new_indices: Vec<u32> = (0..self.data.len())
            .map(|index| {
                let new_index = kept;
                if !removed.contains(index) {
                    kept += 1;
                }
                new_index
            })
            .collect();

        retain_unremoved(&mut self.haystack, removed);
        retain_unremoved(&mut self.data, removed);
        retain_unremoved(&mut self.frecency, removed);
        self.indices.retain(|_, index| {
            let old_index = usize::conv(*index);
            *index = new_indices[old_index];
            !removed.contains(old_index)
        });
    }

    /// Get the number of unique commands in the index.
    #[must_use]
    pub fn command_count(&self) -> usize {
        self.data.len()
    }

    /// Get the cached session preload, but only if it is for `session`.
    fn cached_session_preload(&self, session: &ShellSession) -> Option<SessionPreload> {
        self.session_preload.lock().as_ref().filter(|cached| cached.session == *session).cloned()
    }

    /// Search for commands matching a query.
    ///
    /// Returns an iterator of history IDs as parsed UUIDs (most recent invocation per command).
    /// Uses global frecency for scoring.
    ///
    /// # Errors
    ///
    /// Some searches need the database to complete. For those, this returns a [`PendingSearch`].
    /// Load it without holding a lock on this index. Then finish it with [`Self::finish_search`].
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_search", fields(query = %query))]
    pub fn search(
        &self,
        query: &str,
        filter_mode: &IndexFilterMode,
        limit: usize,
    ) -> Result<impl Iterator<Item = HistoryId> + use<'_>, PendingSearch> {
        let filter = filter_mode
            .compile(self)
            .map_err(|session| PendingSearch::new(self, query, limit, session))?;
        Ok(self.search_compiled(query, &filter, limit))
    }

    /// Finish a search that [`Self::search`] returned as pending. Call this after the search has
    /// loaded its data from the database.
    ///
    /// The search may have begun on another index.
    pub fn finish_search(&self, search: LoadedSearch) -> impl Iterator<Item = HistoryId> + use<'_> {
        search.finish(self)
    }

    /// The core of [`Self::search`]. Requires a filter that is already
    /// [compiled](IndexFilterMode::compile).
    fn search_compiled(
        &self,
        query: &str,
        filter: &CompiledFilter<'_>,
        limit: usize,
    ) -> impl Iterator<Item = HistoryId> + use<'_> {
        let query = crate::search::truncate_query(query);
        // Parse operators before normalization: e.g. `¡` normalizes to `!`, but must
        // remain literal text rather than becoming a negation operator.
        let mut patterns = frizbee::Pattern::parse_query(query);
        for pattern in &mut patterns {
            pattern.needle = pattern.needle.normalize_diacritics().into_owned();
        }

        // Filter pre-pass: collect the candidate commands for this filter mode. This is a sorted
        // vector of command indices.
        let candidates: Vec<u32> =
            tracing::span!(Level::TRACE, "index_search_filter").in_scope(|| match filter {
                CompiledFilter::All => (0..u32::conv(self.data.len())).collect(),
                CompiledFilter::Nothing => Vec::new(),
                filter => (0..)
                    .zip(&self.data)
                    .filter(|&(index, data)| filter.matches(index, data, &self.interner))
                    .map(|(index, _)| index)
                    .collect(),
            });

        let candidate_frecency = |candidate_index: usize| {
            let cmd_idx = usize::conv(candidates[candidate_index]);
            self.frecency[cmd_idx]
        };

        let config = frizbee::Config::default()
            .casing(frizbee::CaseMatching::Smart)
            .sort(frizbee::SortStrategy::IndexAsc);
        let mut matcher = frizbee::Matcher::from_patterns(&patterns, &config);

        // An empty query matches every candidate with fuzzy score 0, so skip
        // the matcher and rank purely by frecency
        let mut scored: Vec<Score> = if matcher.patterns().is_empty() {
            (0..candidates.len())
                .map(|i| Score {
                    fuzzy_score: 0,
                    frecency: candidate_frecency(i),
                    index: u32::conv(i),
                })
                .collect()
        } else {
            // This is a vec of `&Arc<str>` instead of `&str` because `&Arc<str>` is the size of one
            // pointer while `&str` is the size of two.
            let normalized_commands: Vec<&Arc<str>> =
                candidates.iter().map(|i| &self.haystack[usize::conv(*i)]).collect();
            // Use several cores when the number of commands is sufficiently large. The matcher
            // spawns new threads for every search, and beyond this cap that costs more than it saves.
            const MAX_MATCH_THREADS: usize = 8;
            let threads =
                std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_MATCH_THREADS);
            let matches = tracing::span!(Level::TRACE, "index_search_match").in_scope(|| {
                if threads > 1 && normalized_commands.len() >= 10_000 {
                    matcher.match_list_parallel(&normalized_commands, threads)
                } else {
                    matcher.match_list(&normalized_commands)
                }
            });
            matches
                .iter()
                .map(|m| Score {
                    fuzzy_score: m.score,
                    frecency: candidate_frecency(usize::conv(m.index)),
                    index: m.index,
                })
                .collect()
        };

        tracing::span!(Level::TRACE, "index_search_results").in_scope(|| {
            // only the top `limit` results are returned, so partition them
            // out before sorting instead of sorting every match
            let limit = usize::conv(limit);
            if scored.len() > limit {
                scored.select_nth_unstable(limit);
                scored.truncate(limit);
            }
            scored.sort_unstable();
            scored.into_iter().map(move |score| {
                let index = candidates[usize::conv(score.index)];
                self.data[usize::conv(index)].most_recent_id
            })
        })
    }

    /// Recompute the global frecency of every command.
    ///
    /// Call this periodically, so that scores decay over time. It also applies changes to the
    /// frecency multipliers. Commands that are added later use `multipliers` too.
    #[instrument(skip_all, level = tracing::Level::DEBUG, name = "rebuild_frecency")]
    pub fn rebuild_frecency(&mut self, multipliers: FrecencyMultipliers) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        self.frecency = self.recompute_frecency(now, multipliers);
        self.settings.frecency_multipliers = multipliers;
    }

    /// Like [`Self::rebuild_frecency`], but takes an index behind a [`RwLock`].
    ///
    /// The new frecency is computed under an upgradable read lock, so that searches can still run.
    /// The write lock is taken only to swap the new frecency in.
    ///
    /// `after` is called after the new frecency is swapped in, while the write lock is still held.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: clippy wants drop(state) after it already got moved"
    )]
    pub(super) fn rebuild_frecency_shared<T>(
        lock: &RwLock<T>,
        multipliers: FrecencyMultipliers,
        after: impl FnOnce(&mut T),
    ) where
        T: AsRef<Self> + AsMut<Self>,
    {
        let now = OffsetDateTime::now_utc().unix_timestamp();

        let state = lock.upgradable_read();
        let frecency = state.as_ref().recompute_frecency(now, multipliers);
        let mut state = RwLockUpgradableReadGuard::upgrade(state);

        let index = state.as_mut();
        let old_frecency = std::mem::replace(&mut index.frecency, frecency);
        index.settings.frecency_multipliers = multipliers;
        after(&mut state);
        drop(state);
        // The old frecency is freed after the write lock is released.
        drop(old_frecency);
    }

    /// Recompute the frecency of every command with `multipliers`. These may differ from the
    /// index's current multipliers.
    fn recompute_frecency(&self, now: i64, multipliers: FrecencyMultipliers) -> Vec<u32> {
        self.data.iter().map(|data| data.compute_frecency(now, multipliers)).collect()
    }
}

/// The runs that a command has left in the database, folded into what an index would hold for
/// them. [`SyncPermit::sync`](super::SyncPermit::sync) puts them in an index.
///
/// [`SyncPermit::add_run`](super::SyncPermit::add_run) folds the runs one at a time. So this
/// holds none of the runs, however many the command has.
#[derive(Debug)]
pub struct RemainingRuns {
    command: String,
    /// The data that an index would hold for the command. It is [`None`] while no run was folded.
    data: Option<CommandData>,
    /// Interns the keys of [`Self::data`] until it is put in an index. The index's own interner
    /// can't be borrowed mutably while searches use it.
    interner: Rodeo,
}

impl RemainingRuns {
    /// No runs of `command` yet.
    #[must_use]
    pub fn new(command: String) -> Self {
        Self {
            command,
            data: None,
            interner: Rodeo::new(),
        }
    }

    /// Fold `history`, a run of the command, into these runs. It is folded the way that an index
    /// with `settings` would add it.
    pub(super) fn add(&mut self, history: &History, settings: &SearchIndexSettings) {
        if !settings.includes(history) {
            return;
        }
        match &mut self.data {
            Some(data) => {
                data.add_invocation(history, &mut self.interner);
            }
            None => self.data = CommandData::new(history, &mut self.interner),
        }
    }
}

/// Remove the elements of `vec` at the indices in `removed`.
fn retain_unremoved<T>(vec: &mut Vec<T>, removed: &FixedBitSet) {
    let mut index = 0;
    vec.retain(|_| {
        let keep = !removed.contains(index);
        index += 1;
        keep
    });
}

/// Represents how closely a command matches a search query.
#[derive(Clone, Copy, Eq, PartialEq)]
struct Score {
    pub fuzzy_score: u16,
    pub frecency: u32,
    pub index: u32,
}

/// Scores are ordered as follows:
///
/// * Fuzzy score (highest first)
/// * If equal, frecency (highest first)
/// * If equal, index (lowest first)
impl Ord for Score {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.fuzzy_score, other.frecency, self.index).cmp(&(
            self.fuzzy_score,
            self.frecency,
            other.index,
        ))
    }
}

impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use atuin_client::history::AuthorKind;
    use atuin_common::filter::OrFilter;
    use atuin_common::path::DisplayRichExt;
    use rstest::{fixture, rstest};
    use time::macros::datetime;
    use uuid::Uuid;

    use super::*;

    /// An empty history db.
    #[fixture]
    pub(in crate::search::index) async fn db() -> Sqlite {
        Sqlite::in_memory(std::time::Duration::from_secs(5)).await.unwrap()
    }

    pub(in crate::search::index) fn make_history(
        command: &str,
        cwd: &str,
        timestamp: OffsetDateTime,
    ) -> History {
        History::import()
            .timestamp(timestamp)
            .command(command)
            .cwd(cwd)
            .author("test-user")
            .build()
            .into()
    }

    /// The data for `command` in `index`.
    pub(in crate::search::index) fn command<'a>(
        index: &'a SearchIndex,
        command: &str,
    ) -> &'a CommandData {
        &index.data[usize::conv(index.indices[command])]
    }

    /// The frecency of `command` in `index`.
    pub(in crate::search::index) fn frecency(index: &SearchIndex, command: &str) -> u32 {
        index.frecency[usize::conv(index.indices[command])]
    }

    /// Frecency multipliers that zero every score.
    pub(in crate::search::index) fn zero_frecency() -> FrecencyMultipliers {
        FrecencyMultipliers {
            frecency: 0.0,
            ..FrecencyMultipliers::default()
        }
    }

    /// Whether `index` has `command`.
    pub(in crate::search::index) fn contains(index: &SearchIndex, command: &str) -> bool {
        index.indices.contains_key(command)
    }

    /// The number of times that `command` in `index` was run.
    pub(in crate::search::index) fn count(index: &SearchIndex, command: &str) -> u32 {
        self::command(index, command).count
    }

    /// The error of a load that failed.
    pub(in crate::search::index) fn load_failure() -> LoadFromDbError {
        LoadFromDbError(eyre::eyre!("the load failed"))
    }

    impl SearchIndex {
        /// Build a fresh index with `settings`, and populate it from `db`.
        pub(super) async fn from_db(
            settings: SearchIndexSettings,
            db: &Sqlite,
        ) -> Result<Self, LoadFromDbError> {
            let mut new_index = Self::new(settings);
            new_index.load_from_db(db).await?;
            Ok(new_index)
        }

        /// Load every history entry from `db` into this index. Frecency is computed as of now.
        pub(super) async fn load_from_db(&mut self, db: &Sqlite) -> Result<(), LoadFromDbError> {
            self.load_from_db_at(db, OffsetDateTime::now_utc(), |_| {}).await
        }

        /// Add history entries to the index. Frecency is computed as of now.
        pub(super) fn add_history<H>(&mut self, histories: H)
        where
            H: IntoIterator<Item: Borrow<History>>,
        {
            self.add_history_at(histories, OffsetDateTime::now_utc());
        }

        /// The number of invocations of `command` in this index. It is 0 if the index doesn't
        /// have `command`. Tests outside the index module use this.
        pub(crate) fn invocations(&self, command: &str) -> u32 {
            if contains(self, command) {
                count(self, command)
            } else {
                0
            }
        }
    }

    #[rstest]
    fn search_index_add_and_search() {
        let mut index = SearchIndex::default();

        let h1 = make_history("git status", "/home/user/project", datetime!(2024-01-01 10:00 UTC));
        let h2 = make_history(
            "git commit -m 'test'",
            "/home/user/project",
            datetime!(2024-01-01 10:05 UTC),
        );
        let h3 = make_history("ls -la", "/home/user/other", datetime!(2024-01-01 10:10 UTC));

        index.add_history([h1, h2, h3]);

        assert_eq!(index.command_count(), 3);

        // Search for "git" - should match 2 commands
        assert_eq!(index.search_compiled("git", &CompiledFilter::All, 10).count(), 2);

        // Search with directory filter
        // git status and git commit
        let directory = IndexFilterMode::Directory(
            "/home/user/project".display_rich().trailing_slash(true).to_string(),
        );
        assert_eq!(index.search("", &directory, 10).unwrap().count(), 2);
    }

    /// Regression test for #3702: a frequently-run command whose match is
    /// scattered across words must not outrank a contiguous match, no matter
    /// how large its frecency score is.
    #[rstest]
    fn contiguous_match_beats_frequent_scattered_match() {
        let mut index = SearchIndex::default();

        // contiguous match for "foo bar", run once
        let contiguous = make_history("foo bar --baz", "/tmp", datetime!(2024-01-01 10:00 UTC));
        index.add_history([&contiguous]);

        // scattered match (b..a..r spread over "build-analyzer-report"), run
        // 200 times so its frecency dwarfs any fuzzy score difference
        for _ in 0..200 {
            let h =
                make_history("foo build-analyzer-report", "/tmp", datetime!(2024-01-01 10:00 UTC));
            index.add_history([&h]);
        }

        let results: Vec<_> = index.search_compiled("foo bar", &CompiledFilter::All, 10).collect();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            command(&index, "foo bar --baz").most_recent_id,
            "contiguous match must rank above the high-frecency scattered match"
        );
    }

    /// Adding a command updates its frecency immediately, without waiting for a rebuild.
    #[rstest]
    fn add_updates_frecency_without_rebuild() {
        let mut index = SearchIndex::default();
        let now = OffsetDateTime::now_utc();

        index.add_history([make_history("echo alpha", "/tmp", now)]);
        let once = frecency(&index, "echo alpha");
        assert!(once > 0);

        index.add_history([make_history("echo alpha", "/tmp", now)]);
        assert!(frecency(&index, "echo alpha") > once, "a repeat invocation must raise frecency");

        for _ in 0..50 {
            index.add_history([make_history("echo beta", "/tmp", now)]);
        }
        let results: Vec<_> = index.search_compiled("echo", &CompiledFilter::All, 10).collect();
        assert_eq!(results[0], command(&index, "echo beta").most_recent_id);
    }

    /// Frecency still orders results between equally good matches, so
    /// most-recently/frequently-used behavior is preserved where match
    /// quality can't differentiate.
    #[rstest]
    fn equal_matches_order_by_frecency() {
        let mut index = SearchIndex::default();

        index.add_history([make_history("echo alpha", "/tmp", datetime!(2024-01-01 10:00 UTC))]);
        // same fuzzy score for the query, much higher frecency
        for _ in 0..50 {
            let h = make_history("echo beta", "/tmp", datetime!(2024-01-01 10:00 UTC));
            index.add_history([&h]);
        }

        let results: Vec<_> = index.search_compiled("echo", &CompiledFilter::All, 10).collect();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            command(&index, "echo beta").most_recent_id,
            "ties in match quality should be broken by frecency"
        );
    }

    /// Regression test: frizbee does no unicode normalization of its own, so
    /// the index normalizes both sides — an unaccented query must keep
    /// matching accented history entries (nucleo's old Normalization::Smart
    /// behavior), and an accented query must still find its own entry.
    #[rstest]
    fn diacritics_normalized_for_matching() {
        let mut index = SearchIndex::default();

        index.add_history([make_history("echo déjà-vu", "/tmp", datetime!(2024-01-01 10:00 UTC))]);
        index.add_history([make_history("echo plain", "/tmp", datetime!(2024-01-01 10:00 UTC))]);

        let expected = command(&index, "echo déjà-vu").most_recent_id;

        let results: Vec<_> = index.search_compiled("deja", &CompiledFilter::All, 10).collect();
        assert_eq!(results, vec![expected]);

        let results: Vec<_> = index.search_compiled("déjà", &CompiledFilter::All, 10).collect();
        assert_eq!(results, vec![expected]);
    }

    #[rstest]
    fn normalization_does_not_introduce_query_operators() {
        let mut index = SearchIndex::default();
        let literal = make_history("¡\"", "/tmp", datetime!(2024-01-01 10:00 UTC));
        let plain = make_history("plain", "/tmp", datetime!(2024-01-01 10:00 UTC));
        index.add_history([&literal]);
        index.add_history([&plain]);

        for query in ["¡\"", "^¡\"$", "'¡\"", "\\!\""] {
            let results: Vec<_> = index.search_compiled(query, &CompiledFilter::All, 10).collect();
            assert_eq!(results, vec![literal.id], "query {query:?}");
        }
        let results: Vec<_> = index.search_compiled("!¡\"", &CompiledFilter::All, 10).collect();
        assert_eq!(results, vec![plain.id]);
    }

    /// A deterministic synthetic corpus large enough to cross the 10k
    /// parallel-matching threshold, shaped like real history (repeated
    /// prefixes, multi-word commands, a few accented entries).
    fn equivalence_corpus() -> Vec<String> {
        let prefixes = [
            "git status",
            "git push origin",
            "cargo build --release",
            "docker compose up",
            "ls -la",
            "echo déjà",
            "kubectl get pods -n",
            "make -j",
        ];
        (0..12_000).map(|i| format!("{} run-{i}", prefixes[i % prefixes.len()])).collect()
    }

    /// The parallel-matching switch must be invisible in the results:
    /// frizbee's match_list_parallel must return exactly the same matches,
    /// scores, and order as match_list for our config. Guards the 10k
    /// threshold in search(), which machines cross as history grows.
    #[rstest]
    fn parallel_matching_equals_serial() {
        let corpus = equivalence_corpus();
        let haystack: Vec<&str> = corpus.iter().map(String::as_str).collect();
        let config = frizbee::Config::default()
            .casing(frizbee::CaseMatching::Smart)
            .sort(frizbee::SortStrategy::IndexAsc);

        // multi-atom and negated atoms exercise the progressive-filter path
        for query in ["git", "git p", "docker compose up", "cargo !release", "g"] {
            let serial = frizbee::Matcher::from_query(query, &config).match_list(&haystack);
            for threads in [2, 8] {
                let parallel = frizbee::Matcher::from_query(query, &config)
                    .match_list_parallel(&haystack, threads);
                assert_eq!(serial.len(), parallel.len(), "query {query:?}");
                for (s, p) in serial.iter().zip(&parallel) {
                    assert_eq!(
                        (s.index, s.score),
                        (p.index, p.score),
                        "query {query:?}, {threads} threads"
                    );
                }
            }
        }
    }

    /// End-to-end determinism above the parallel threshold: the same query
    /// against the same index must return the same ranked IDs every time.
    #[rstest]
    fn search_results_stable_above_parallel_threshold() {
        let mut index = SearchIndex::default();
        for (i, command) in equivalence_corpus().iter().enumerate() {
            // spread timestamps so frecency scores actually differ
            let ts = datetime!(2024-01-01 00:00 UTC) + time::Duration::minutes(i64::conv(i % 1440));
            index.add_history([make_history(command, "/tmp", ts)]);
        }
        assert!(index.command_count() > 10_000, "corpus must cross threshold");

        for query in ["git", "git p", "docker compose up", "deja", ""] {
            let first: Vec<_> = index.search_compiled(query, &CompiledFilter::All, 200).collect();
            for _ in 0..2 {
                let again: Vec<_> =
                    index.search_compiled(query, &CompiledFilter::All, 200).collect();
                assert_eq!(first, again, "query {query:?} returned unstable results");
            }
        }
    }

    /// Queries longer than frizbee can score are truncated instead of
    /// panicking in Matcher::from_query.
    #[rstest]
    fn long_query_truncated_not_panicking() {
        let mut index = SearchIndex::default();
        index.add_history([make_history("echo hello", "/tmp", datetime!(2024-01-01 10:00 UTC))]);

        let long_query = "a".repeat(5000);
        assert!(index.search_compiled(&long_query, &CompiledFilter::All, 10).next().is_none());
    }

    /// Frecency is computed as of the `now` that is passed in, not the wall clock. An operation can
    /// pass the same `now` for every entry it adds. Then a newer entry that is added later isn't
    /// scored as older than an older entry that was added earlier.
    #[rstest]
    fn add_history_at_scores_as_of_now() {
        let then = datetime!(2024-01-01 10:00 UTC);
        let mut index = SearchIndex::default();

        index.add_history_at([make_history("echo old", "/tmp", then)], then);
        // This entry was run after the operation started, but it is added with the same `now`. So
        // its age clamps to 0.
        let newer = then + time::Duration::hours(2);
        index.add_history_at([make_history("echo new", "/tmp", newer)], then);
        assert_eq!(frecency(&index, "echo old"), frecency(&index, "echo new"));

        // The same entry is much less recent when it is scored as of a month later.
        index.add_history_at(
            [make_history("echo stale", "/tmp", then)],
            then + time::Duration::days(30),
        );
        assert!(frecency(&index, "echo stale") < frecency(&index, "echo old"));
    }

    /// An index that is built from the database computes frecency with the given settings. No
    /// rebuild is needed.
    #[rstest]
    #[tokio::test]
    async fn from_db_applies_search_settings(#[future(awt)] db: Sqlite) {
        db.save(&make_history("echo hello", "/tmp", OffsetDateTime::now_utc())).await.unwrap();

        let default = SearchIndex::from_db(SearchIndexSettings::default(), &db).await.unwrap();
        assert!(frecency(&default, "echo hello") > 0);

        let settings = SearchIndexSettings {
            frecency_multipliers: zero_frecency(),
            ..SearchIndexSettings::default()
        };
        let zeroed = SearchIndex::from_db(settings.clone(), &db).await.unwrap();
        assert_eq!(frecency(&zeroed, "echo hello"), 0);
        assert_eq!(*zeroed.settings(), settings);
    }

    /// Rebuilding frecency rescores existing commands with the new settings. Later adds use the
    /// new settings too.
    #[rstest]
    fn rebuild_frecency_applies_settings() {
        let now = OffsetDateTime::now_utc();
        let mut index = SearchIndex::default();
        index.add_history([make_history("echo before", "/tmp", now)]);
        assert!(frecency(&index, "echo before") > 0);

        let multipliers = zero_frecency();
        index.rebuild_frecency(multipliers);
        assert_eq!(index.settings().frecency_multipliers, multipliers);
        assert_eq!(frecency(&index, "echo before"), 0);

        index.add_history([make_history("echo after", "/tmp", now)]);
        assert_eq!(frecency(&index, "echo after"), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn load_from_db_counts_every_invocation(#[future(awt)] db: Sqlite) {
        // One group by (command, cwd, host, session), with the newest id holding the oldest
        // timestamp: the grouped load picked that row and its cursor skipped the rest.
        for minute in 0..3 {
            let mut run = make_history(
                "cargo build",
                "/tmp",
                datetime!(2024-01-01 10:00 UTC) - time::Duration::minutes(minute),
            );
            run.session = "0199b2f0aaaa7000800000000000000a".to_owned();
            db.save(&run).await.unwrap();
        }

        let mut index = SearchIndex::default();
        index.load_from_db(&db).await.unwrap();

        assert_eq!(command(&index, "cargo build").count, 3);
    }

    #[rstest]
    #[case::all(&[], 7)]
    #[case::bash(&["bash"], 1)]
    #[case::bash_unknown(&["bash", ""], 5)]
    #[case::bash_zsh(&["bash", "zsh"], 3)]
    #[case::unknown(&[""], 4)]
    #[case::fish(&["fish"], 0)]
    #[case::fish_unknown(&["fish", ""], 4)]
    fn search_with_shell_filter(#[case] shells: &[&str], #[case] expected_count: usize) {
        let filter =
            OrFilter::from_list(shells.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
                .unwrap_or_default();
        let mut index = SearchIndex::new(SearchIndexSettings {
            shells: filter,
            ..SearchIndexSettings::default()
        });

        for (command, shell) in [
            ("echo unknown1", None),
            ("echo zsh1", Some("zsh")),
            ("echo unknown2", None),
            ("echo bash", Some("bash")),
            ("echo unknown3", None),
            ("echo unknown4", None),
            ("echo zsh2", Some("zsh")),
        ] {
            let mut history = make_history(command, "/tmp", OffsetDateTime::now_utc());
            history.shell = shell.map(str::to_owned);
            index.add_history([&history]);
        }

        let results: Vec<_> = index.search_compiled("echo", &CompiledFilter::All, 100).collect();
        assert_eq!(results.len(), expected_count, "{results:?}");
    }

    /// An index loaded with `histories` the way a load from the database would: newest id first.
    fn index_of(histories: &[History]) -> SearchIndex {
        let mut histories = histories.to_vec();
        histories.sort_by_key(|h| std::cmp::Reverse(h.id));
        let mut index = SearchIndex::default();
        index.add_history(&histories);
        index
    }

    /// The runs of `command` in `histories`, folded newest id first. The journal folds what
    /// `Sqlite::history_of_command` streams in the same order.
    pub(in crate::search::index) fn remaining_runs(
        histories: &[History],
        command: &str,
    ) -> RemainingRuns {
        let mut rows: Vec<_> = histories.iter().filter(|h| h.command == command).collect();
        rows.sort_by_key(|h| std::cmp::Reverse(h.id));
        let mut runs = RemainingRuns::new(command.to_owned());
        for row in rows {
            runs.add(row, &SearchIndexSettings::default());
        }
        runs
    }

    /// Per command: use count, most recent timestamp and id, directories, hosts, sessions and
    /// frecency.
    type CommandSnapshot =
        (u32, i64, HistoryId, BTreeSet<String>, BTreeSet<String>, Vec<Uuid>, u32);

    /// What `index` holds for each command, with interned keys resolved. Two indexes then compare
    /// equal when they agree, whatever order they hold their commands in. Also checks that the
    /// index's vectors stay aligned with its map of commands.
    fn snapshot(index: &SearchIndex) -> BTreeMap<String, CommandSnapshot> {
        let len = index.data.len();
        assert_eq!(index.haystack.len(), len, "a haystack entry per command");
        assert_eq!(index.frecency.len(), len, "a frecency per command");
        assert_eq!(index.indices.len(), len, "an index per command");
        index
            .indices
            .iter()
            .map(|(command, &i)| {
                let i = usize::conv(i);
                assert_eq!(&*index.haystack[i], &*command.normalize_diacritics(), "misaligned");
                let data = &index.data[i];
                let (directories, hosts, sessions) = data.places(&index.interner);
                let snapshot = (
                    data.count,
                    data.most_recent_timestamp,
                    data.most_recent_id,
                    directories,
                    hosts,
                    sessions,
                    index.frecency[i],
                );
                (command.to_string(), snapshot)
            })
            .collect()
    }

    /// Syncing the commands that a delete touched leaves the index as a reload of the remaining
    /// rows would.
    #[rstest]
    fn sync_commands_matches_a_reload() {
        let at = |minutes| datetime!(2024-01-01 10:00 UTC) + time::Duration::minutes(minutes);
        let all = [
            make_history("git status", "/a", at(0)),
            make_history("cargo build", "/a", at(1)),
            make_history("git status", "/b", at(2)),
            make_history("ls -la", "/b", at(3)),
            make_history("cargo build", "/a", at(4)),
            make_history("echo déjà", "/c", at(5)),
            make_history("git status", "/c", at(6)),
        ];
        // Delete the newest `git status`, which is its only run in /c. Also delete the only
        // `ls -la` and one of the two `cargo build`.
        let deleted = [all[6].id, all[3].id, all[1].id];
        let remaining: Vec<_> = all.iter().filter(|h| !deleted.contains(&h.id)).cloned().collect();

        let mut index = index_of(&all);
        let synced = ["git status", "ls -la", "cargo build"];
        index.sync_commands(
            synced.map(|command| remaining_runs(&remaining, command)),
            OffsetDateTime::now_utc(),
        );

        let reloaded = index_of(&remaining);
        assert_eq!(snapshot(&index), snapshot(&reloaded));
        let c = IndexFilterMode::Directory("/c".display_rich().trailing_slash(true).to_string());
        for query in ["git", "cargo", "ls", "deja", ""] {
            for filter in [&IndexFilterMode::Global, &c] {
                let hits = |index: &SearchIndex| -> BTreeSet<HistoryId> {
                    index.search(query, filter, 10).unwrap().collect()
                };
                assert_eq!(hits(&index), hits(&reloaded), "query {query:?}, {filter:?}");
            }
        }
    }

    /// A command that has no runs left is removed. The commands after it stay findable with any
    /// filter, and running it again brings it back once.
    #[rstest]
    fn sync_commands_removes_a_command_without_runs() {
        let mut index = SearchIndex::default();
        let gone = make_history("echo gone", "/tmp", datetime!(2024-01-01 10:00 UTC));
        let kept = make_history("echo kept", "/tmp", datetime!(2024-01-01 10:01 UTC));
        index.add_history([&gone, &kept]);

        index.sync_commands([remaining_runs(&[], "echo gone")], OffsetDateTime::now_utc());

        assert_eq!(index.command_count(), 1);
        let global: Vec<_> = index.search("echo", &IndexFilterMode::Global, 10).unwrap().collect();
        assert_eq!(global, vec![kept.id]);
        // A directory filter reads each command's data by its index. The removal moved that index.
        let tmp =
            IndexFilterMode::Directory("/tmp".display_rich().trailing_slash(true).to_string());
        assert_eq!(index.search("", &tmp, 10).unwrap().collect::<Vec<_>>(), vec![kept.id]);

        let again = make_history("echo gone", "/tmp", datetime!(2024-01-01 10:02 UTC));
        index.add_history([&again]);
        let hits: BTreeSet<_> =
            index.search("echo", &IndexFilterMode::Global, 10).unwrap().collect();
        assert_eq!(hits, BTreeSet::from([kept.id, again.id]));
        assert_eq!(snapshot(&index).len(), 2);
    }

    /// Runs that the index leaves out don't keep a command in it. Here the run that is left is an
    /// agent's.
    #[rstest]
    fn sync_commands_ignores_runs_the_index_leaves_out() {
        let mut index = SearchIndex::default();
        let human = make_history("make deploy", "/tmp", datetime!(2024-01-01 10:00 UTC));
        let mut agent = make_history("make deploy", "/tmp", datetime!(2024-01-01 10:01 UTC));
        agent.author_kind = Some(AuthorKind::Agent);
        index.add_history([&human, &agent]);
        assert_eq!(count(&index, "make deploy"), 1);

        index.sync_commands([remaining_runs(&[agent], "make deploy")], OffsetDateTime::now_utc());

        assert_eq!(index.command_count(), 0);
        assert!(snapshot(&index).is_empty());
    }

    /// A command with runs that the index didn't have is added, as a reload would add it.
    #[rstest]
    fn sync_commands_adds_runs_the_index_missed() {
        let mut index = SearchIndex::default();
        let missed = make_history("echo missed", "/tmp", datetime!(2024-01-01 10:00 UTC));

        let runs = remaining_runs(std::slice::from_ref(&missed), "echo missed");
        index.sync_commands([runs], OffsetDateTime::now_utc());

        let hits: Vec<_> = index.search("missed", &IndexFilterMode::Global, 10).unwrap().collect();
        assert_eq!(hits, vec![missed.id]);
        assert_eq!(snapshot(&index), snapshot(&index_of(&[missed])));
    }
}
