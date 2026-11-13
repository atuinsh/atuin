use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId, ShellSession};
use atuin_common::filter::OrFilter;
use easy_cast::Conv;
use fixedbitset::FixedBitSet;
use tracing::{error, instrument};

use crate::search::index::core::{CompiledFilter, IndexIdentity, SearchIndex};

/// Which commands were run before a session started.
/// [`IndexFilterMode::SessionPreload`](super::IndexFilterMode::SessionPreload) uses this.
///
/// The set is built once, when the preload is prepared. A search then checks one bit per command.
/// That keeps the search from reading each command's [`CommandData`](super::CommandData).
#[derive(Debug, Clone)]
pub(super) struct SessionPreload {
    pub session: ShellSession,

    /// The commands that were run before the start of the session, stored as indices into
    /// [`SearchIndex::data`](super::SearchIndex::data).
    ran_before: Arc<FixedBitSet>,
}

impl SessionPreload {
    /// The preload for `session` in `index`.
    ///
    /// `rerun` holds the commands that the database query found. Those commands were run both
    /// before the start of the session and since the start of that second. Commands that `index`
    /// doesn't have yet are ignored.
    ///
    /// A command was run before the session started in one of two cases. One case is that its
    /// most recent invocation is in an earlier second than the start. The other case is that the
    /// query found it. [`CommandData::most_recent_timestamp`](super::CommandData) has a precision
    /// of whole seconds, so the index alone can't decide the commands that were run in the start's
    /// own second.
    pub fn new(session: ShellSession, index: &SearchIndex, rerun: &[String]) -> Self {
        let mut ran_before = FixedBitSet::with_capacity(index.data.len());
        let start_second = session.start.unix_timestamp();
        for (i, data) in index.data.iter().enumerate() {
            if data.most_recent_timestamp < start_second {
                ran_before.insert(i);
            }
        }
        for command in rerun {
            if let Some(&command) = index.indices.get(command.as_str()) {
                ran_before.insert(usize::conv(command));
            }
        }
        Self {
            session,
            ran_before: Arc::new(ran_before),
        }
    }

    /// Whether the command at `index` was run before the session started.
    pub fn ran_before_session(&self, index: u32) -> bool {
        self.ran_before.contains(usize::conv(index))
    }

    /// Record an invocation of the command at `command_index` that was added to the index.
    /// `history` is the invocation.
    ///
    /// An invocation before the start of the session marks its command. An invocation after the
    /// start changes nothing, because a command that was run before the session stays that way.
    pub fn record(&mut self, command_index: u32, history: &History) {
        if history.timestamp < self.session.start {
            let index = usize::conv(command_index);
            let ran_before = Arc::make_mut(&mut self.ran_before);
            ran_before.grow(index + 1);
            ran_before.insert(index);
        }
    }
}

/// A search that needs the database to complete.
///
/// A session-preload search is one example. The index alone can't find every command that was run
/// before the session started.
///
/// [`SearchIndex::search`](super::SearchIndex::search) returns this type. [Load](Self::load) it
/// without holding a lock on the index, then finish it with
/// [`SearchIndex::finish_search`](super::SearchIndex::finish_search).
#[derive(Debug)]
#[must_use = "a pending search returns nothing until it's finished"]
pub struct PendingSearch {
    query: String,
    limit: usize,
    session: ShellSession,
    /// The index that the search began on.
    index: IndexIdentity,
    /// The shell filter of that index. The database query applies the same filter.
    shells: OrFilter<Vec<String>>,
    /// The total number of entries that had been added to that index when the search began.
    num_total: u64,
}

impl PendingSearch {
    /// A search on `index` for `query` that needs the database to find the commands that were run
    /// before `session` started.
    pub(super) fn new(
        index: &SearchIndex,
        query: &str,
        limit: usize,
        session: ShellSession,
    ) -> Self {
        Self {
            query: query.to_owned(),
            limit,
            session,
            index: index.identity.clone(),
            shells: index.settings.shells.clone(),
            num_total: index.num_total,
        }
    }

    /// Load the data that the search needs from `db`.
    ///
    /// The data is the set of commands that were run before the session started and also since the
    /// start of that second.
    pub async fn load(self, db: &Sqlite) -> LoadedSearch {
        let rerun = db
            .commands_run_since_and_before(
                self.session.start.truncate_to_second(),
                self.session.start,
                self.shells.as_slice_filter(),
            )
            .await
            .inspect_err(|e| error!("Failed to load history from before the session started: {e}"))
            .ok();
        LoadedSearch {
            pending: self,
            rerun,
        }
    }
}

/// A [`PendingSearch`] that has loaded its data from the database. Pass it to
/// [`SearchIndex::finish_search`](super::SearchIndex::finish_search).
#[derive(Debug)]
#[must_use = "a loaded search returns nothing until it's finished"]
pub struct LoadedSearch {
    pending: PendingSearch,
    /// The commands that were run before the session started and also since the start of that
    /// second. It is `None` if the query failed.
    rerun: Option<Vec<String>>,
}

impl LoadedSearch {
    /// Finish this search on `index`. The search may have begun on another index.
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_search", fields(query = %self.pending.query))]
    pub(super) fn finish(self, index: &SearchIndex) -> impl Iterator<Item = HistoryId> + use<'_> {
        let filter = CompiledFilter::SessionPreload(self.session_preload(index));
        index.search_compiled(&self.pending.query, &filter, self.pending.limit)
    }

    /// Prepare the [`SessionPreload`] for this search on `index`.
    ///
    /// The result is cached if it is complete for `index`.
    fn session_preload(&self, index: &SearchIndex) -> SessionPreload {
        // If the query failed, a command matches only if `index` alone knows that it was run
        // before the session started.
        let rerun = self.rerun.as_deref().unwrap_or_default();
        let preload = SessionPreload::new(self.pending.session, index, rerun);

        // Cache the preload only if it is complete for `index`. That requires a successful query.
        // It also requires that no entries were added to `index` since the search began. The
        // query may have missed such entries, and the preload didn't record them. An incomplete
        // preload is still used for the current search. Adds need `&mut SearchIndex`, so no add
        // can run between this check and the caching.
        if self.rerun.is_some()
            && self.pending.index == index.identity
            && index.num_total == self.pending.num_total
        {
            *index.session_preload.lock() = Some(preload.clone());
        }
        preload
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::history::{AuthorKind, HistoryId};
    use rstest::{fixture, rstest};
    use time::OffsetDateTime;
    use time::macros::datetime;
    use uuid::Uuid;

    use super::*;
    use crate::search::index::core::IndexFilterMode;
    use crate::search::index::core::tests::*;

    /// A session that started at 2024-01-01 12:00:00.5 UTC. Its UUIDv7 id records the start time.
    /// Because of the half second, runs just before and just after the start fall in the same
    /// second as the start.
    #[fixture]
    fn session() -> ShellSession {
        let start = datetime!(2024-01-01 12:00:00.5 UTC);
        let secs = u64::try_from(start.unix_timestamp()).unwrap();
        let id = Uuid::new_v7(uuid::Timestamp::from_unix(uuid::NoContext, secs, 500_000_000));
        let session = ShellSession { id, start };
        // The daemon reads the start back out of the id, so the two must agree.
        assert_eq!(ShellSession::parse(&id.as_simple().to_string()), Some(session));
        session
    }

    fn make_session_history(
        command: &str,
        session: Uuid,
        timestamp: OffsetDateTime,
        author_kind: AuthorKind,
    ) -> History {
        History::import()
            .timestamp(timestamp)
            .command(command)
            .cwd("/tmp")
            .session(session.as_simple().to_string())
            .author("test-user")
            .author_kind(Some(author_kind))
            .build()
            .into()
    }

    /// Search `index` for `query` the way the daemon does.
    ///
    /// If `filter` needs the database, this queries `db` and finishes the search with the result.
    /// The ids are sorted, because these tests only check which ids are present.
    async fn search_with_db(
        index: &SearchIndex,
        query: &str,
        filter: &IndexFilterMode,
        db: &Sqlite,
    ) -> Vec<HistoryId> {
        let mut results: Vec<_> = match index.search(query, filter, 10) {
            Ok(results) => results.collect(),
            Err(pending) => index.finish_search(pending.load(db).await).collect(),
        };
        results.sort_unstable();
        results
    }

    /// The most recent ids of `commands` in `index`, sorted.
    fn ids(index: &SearchIndex, commands: &[&str]) -> Vec<HistoryId> {
        let mut ids: Vec<_> = commands.iter().map(|c| command(index, c).most_recent_id).collect();
        ids.sort_unstable();
        ids
    }

    /// `session-preload` matches the session's own commands plus anything a user ran, in any
    /// session, before the session started -- not just the session's own commands.
    ///
    /// Entries that aren't saved to the db stand in for entries that were deleted from it. The
    /// index hasn't been rebuilt since those deletions. A command that was only run before the
    /// session started still matches until the rebuild. Any other filter behaves the same way. A
    /// command that was only run after the session started never matches. That holds even though
    /// the db no longer has its runs.
    #[rstest]
    #[case::v7_session(true, &[
        "before",
        "mine",
        "before_and_after",
        "same_second_before",
        "deleted_before",
    ])]
    #[case::session_without_timestamp(false, &["mine"])]
    #[tokio::test]
    async fn session_preload_includes_history_before_session_start(
        #[case] v7: bool,
        #[case] expected: &[&str],
        #[future(awt)] db: Sqlite,
        session: ShellSession,
    ) {
        let start = session.start;
        let session = if v7 {
            session.id
        } else {
            Uuid::new_v4()
        };
        let other = Uuid::new_v4();
        let (ms, minutes) = (time::Duration::milliseconds, time::Duration::minutes);

        let mut index = SearchIndex::default();
        for (command, session, timestamp, author_kind, saved) in [
            ("before", other, start - time::Duration::nanoseconds(1), AuthorKind::User, true),
            ("mine", session, start + minutes(5), AuthorKind::User, true),
            ("after", other, start, AuthorKind::User, true),
            ("before_and_after", other, start + minutes(1), AuthorKind::User, true),
            ("before_and_after", other, start - minutes(1), AuthorKind::User, true),
            // These runs are in the same second as the start. Only the db can tell them apart.
            ("same_second_before", other, start - ms(100), AuthorKind::User, true),
            ("same_second_after", other, start + ms(100), AuthorKind::User, true),
            // An agent's earlier run doesn't count: the index only holds user history.
            ("agent_before", other, start - minutes(1), AuthorKind::Agent, true),
            ("agent_before", other, start + minutes(1), AuthorKind::User, true),
            ("deleted_before", other, start - minutes(1), AuthorKind::User, false),
            ("deleted_after", other, start + minutes(1), AuthorKind::User, false),
        ] {
            let history = make_session_history(command, session, timestamp, author_kind);
            if saved {
                db.save(&history).await.unwrap();
            }
            index.add_history([&history]);
        }

        let filter = IndexFilterMode::SessionPreload(session.as_simple().to_string());
        assert_eq!(search_with_db(&index, "", &filter, &db).await, ids(&index, expected));
    }

    /// The first `session-preload` search builds a cache. The cache stays correct as history
    /// arrives later, and the caller doesn't need to clear it.
    ///
    /// A command that was run after the session started stays out. A command that was run before
    /// the session started is picked up when it arrives. For example, a sync or the rest of the
    /// initial load can bring such a command. An earlier command that is run again is also picked
    /// up.
    #[rstest]
    #[tokio::test]
    async fn session_preload_follows_history_added_after_the_cache_is_built(
        #[future(awt)] db: Sqlite,
        session: ShellSession,
    ) {
        let start = session.start;
        let other = Uuid::new_v4();
        let filter = IndexFilterMode::SessionPreload(session.id.as_simple().to_string());
        let minutes = time::Duration::minutes;

        let mut index = SearchIndex::default();
        let old = make_session_history("old", other, start - minutes(1), AuthorKind::User);
        db.save(&old).await.unwrap();
        index.add_history([&old]);
        assert_eq!(search_with_db(&index, "", &filter, &db).await, ids(&index, &["old"]));

        for (command, timestamp, expected) in [
            ("old", start + minutes(1), &["old"][..]),
            ("new_since_start", start + minutes(1), &["old"]),
            ("synced_from_before", start - minutes(2), &["old", "synced_from_before"]),
            ("new_since_start", start - minutes(3), &[
                "new_since_start",
                "old",
                "synced_from_before",
            ]),
        ] {
            let history = make_session_history(command, other, timestamp, AuthorKind::User);
            db.save(&history).await.unwrap();
            index.add_history([&history]);
            let mut results: Vec<_> =
                index.search("", &filter, 10).expect("still cached").collect();
            results.sort_unstable();
            assert_eq!(results, ids(&index, expected), "after adding {command} at {timestamp}");
        }
    }

    /// History that arrives while the preload query runs isn't in the query's result. So that
    /// result must not be cached. The next search has to run the query again.
    #[rstest]
    #[tokio::test]
    async fn session_preload_is_not_cached_when_history_arrives_during_the_query(
        #[future(awt)] db: Sqlite,
        session: ShellSession,
    ) {
        let start = session.start;
        let other = Uuid::new_v4();
        let filter = IndexFilterMode::SessionPreload(session.id.as_simple().to_string());
        let minutes = time::Duration::minutes;

        let mut index = SearchIndex::default();
        let old = make_session_history("old", other, start - minutes(1), AuthorKind::User);
        db.save(&old).await.unwrap();
        index.add_history([&old]);

        // This search begins, and then "old" is run again. The new run is saved after the query.
        let Err(pending) = index.search("", &filter, 10) else {
            panic!("nothing is cached yet");
        };
        let rerun = make_session_history("old", other, start + minutes(1), AuthorKind::User);
        index.add_history([&rerun]);
        let loaded = pending.load(&db).await;
        db.save(&rerun).await.unwrap();
        assert_eq!(index.finish_search(loaded).count(), 0, "the query missed the run again");

        assert!(index.search("", &filter, 10).is_err(), "the stale result isn't cached");
        assert_eq!(search_with_db(&index, "", &filter, &db).await, ids(&index, &["old"]));
        assert!(index.search("", &filter, 10).is_ok(), "that one is cached");
    }

    /// A pending session-preload search can finish on a different index than the one it began on.
    /// For example, a rebuild may install a new index while the search queries the database. The
    /// search then uses the new index's commands.
    ///
    /// If the query failed, the search still finds some commands. These are the commands that the
    /// index alone knows were run before the session started. Neither result is cached.
    #[rstest]
    #[case::another_index(true, false, &["mine", "mixed", "old"])]
    #[case::query_failed(false, true, &["mine", "old"])]
    #[tokio::test]
    async fn pending_session_preload_completes_without_its_index_or_its_query(
        #[case] another_index: bool,
        #[case] query_fails: bool,
        #[case] expected: &[&str],
        #[future(awt)] db: Sqlite,
        session: ShellSession,
    ) {
        let filter = IndexFilterMode::SessionPreload(session.id.as_simple().to_string());
        let other = Uuid::new_v4();
        let minutes = time::Duration::minutes;
        let mut index = SearchIndex::default();
        for (command, session, timestamp) in [
            ("old", other, session.start - minutes(1)),
            ("mine", session.id, session.start + minutes(1)),
            ("mixed", other, session.start - minutes(1)),
            ("mixed", other, session.start + minutes(1)),
        ] {
            let history = make_session_history(command, session, timestamp, AuthorKind::User);
            db.save(&history).await.unwrap();
            index.add_history([&history]);
        }
        let other_index = if another_index {
            Some(SearchIndex::from_db(index.settings().clone(), &db).await.unwrap())
        } else {
            None
        };

        let Err(pending) = index.search("", &filter, 10) else {
            panic!("nothing is cached yet");
        };
        let mut loaded = pending.load(&db).await;
        if query_fails {
            loaded.rerun = None;
        }
        let finishing = other_index.as_ref().unwrap_or(&index);
        let mut results: Vec<_> = finishing.finish_search(loaded).collect();
        results.sort_unstable();
        assert_eq!(results, ids(finishing, expected));
        assert!(finishing.search("", &filter, 10).is_err(), "nothing is cached");
    }
}
