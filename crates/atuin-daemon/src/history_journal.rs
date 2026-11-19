//! Exposes logic for managing the lifecycle of commands.
//!
//! The core structure is [`HistoryJournal`]. The [`HistoryJournal`] handles the creation and
//! termination of new commands.
//!
//! When users run new commands in the client, the client sends requests to the GRPC server. The
//! [`crate::grpc::HistoryService`] then "forwards" the request down into [`HistoryJournal`] --
//! requesting the start of a new shell command.
//!
//! The [`HistoryJournal`] is responsible for managing the lifecycle of this command. The main
//! entrypoint is [`HistoryJournal::start_cmd`] which marks the beginning of a command. This returns
//! the [`HistoryId`] of the command which has been started, but not finished.
//!
//! ## Commands in flight
//!
//! Commands-in-flight are commands which have been started but are already running. These commands
//! are uniquely identified by their [`HistoryId`].
//!
//! Commands in flight can be terminated in one of three ways:
//!
//!   - [`HistoryJournal::finish`] marks the command as finished, which will create and store a new
//!     history entry.
//!   - [`HistoryJournal::cancel`] cancels the command, disposing of any in-memory resources, but
//!     without the logic of persisting the history entry. This will discard any captured output
//!     too.
//!   - [`HistoryJournal::delete`] deletes a history entry which will dispose of a command, if it is
//!     in flight, announcing the command as "cancelled". This will discard any captured output
//!     too.
//!
//! ## Streaming
//!
//! It is possible to stream events out of [`HistoryJournal`] via [`HistoryJournal::subscribe`]
//! which returns a new [`futures::Stream`] of [`Result<CmdEvent, _>`] events.
//!
//! ## Output Capture
//!
//! When deleting entries from the history journal, we have to be careful to gracefully handle
//! output command capture.
//!
//! A command can be in one of many states:
//!
//!   1. The command is considered "in flight", ie. it has been started but not stopped. In this
//!      case, it will be a member of the `active_cmds` hashmap.
//!   2. The command is considered "settled", ie. it has been stopped and committed into the
//!      database.
//!   3. A command is deleted, ie. it has been added at some point, and has been subsequently
//!      deleted through [`HistoryJournal::delete`].
//!
//! In all of these states, the note that a caller can request a command can have associated output
//! capture enter out-of-band, via [`HistoryJournal::register_command_output`]. This **must** be
//! rejected if the command is deleted.
//!
//! ## Deletion
//!
//! Deletion is handled through [`HistoryJournal::delete`]. This is a potentially long process and
//! requires the following invariants be upheld:
//!
//!   - During deletion, [`HistoryJournal::register_command_output`] requests going in parallel must
//!     be rejected.
//!
//! ## Concurrency
//!
//! Concurrency in this module is quite difficult. Here are the important things to know about:
//!
//!   - [`HistoryJournal::deleting`] is a hashmap which refcounts each history ID which is currently
//!     under the process of deletion. This enables us to reject any command output capture requests
//!     which run in parallel to a deletion.
//!   - [`HistoryJournal::lifecycle_mutex`] is a [sharded
//!     mutex](http://quinnftw.com/sharding-to-reduce-mutex-contention/) which guards [`HistoryId`]s
//!     as they transition between states. _**Note to maintainers**: This sharded mutex **can
//!     deadlock** if you are awaiting a [`HistoryId`], while holding the same or another
//!     [`HistoryId`]. **Always** avoid nesting [`HistoryJournal::lifecycle_mutex`] accesses._
//!
//!     This utility is critical as it prevents parallel [`HistoryJournal::cancel`],
//!     [`HistoryJournal::delete`], [`HistoryJournal::register_command_output`], etc. requests from
//!     stomping over each other and putting us in an inconsistent state.
//!   - [`InFlightCmd::finalization_mutex`] is used to prevent the cancellation of a command as it
//!     is being finalized. It is acquired at the start of [`HistoryJournal::finish`],
//!     [`HistoryJournal::cancel`] and for each in-flight entry deleted during
//!     [`HistoryJournal::delete`].
//!

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::{DeletedLiveRows, Sqlite as HistoryDatabase};
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{CommandCapture, History, HistoryId};
use atuin_client::packfile;
use atuin_client::settings::{OutputCapture, Search};
use atuin_common::secrets::files::SensitiveFiles;
use atuin_common::sync::AsyncShardedMutex;
use atuin_domain::caps::{CapClient, PackfileCap};
use atuin_domain::record::{RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::field::Empty;
use tracing::{Instrument, Span};

#[cfg(feature = "octavo")]
use crate::octavo::Octavo;
use crate::output_capture::{CaptureError, GetOutputError, OutputCaptureEngine};
use crate::search::{FrecencyMultipliers, RebuildSettings, SearchIndexSettings, SharedSearchIndex};

/// History-db rows removed per transaction by [`HistoryJournal::delete`]; matches the chunking of
/// `HistoryStore::build_all`, which this path used to go through.
const DELETE_ROWS_BATCH_SIZE: usize = 5000;

/// An event describing a change in the lifecycle of a command.
#[derive(Debug, Clone)]
pub enum CmdEvent {
    /// A command has been started. See [`HistoryJournal::start_cmd`].
    Started(History),
    /// A command has been successfully finished. See [`HistoryJournal::finish`].
    Finished(History),
    /// A command has been cancelled. See [`HistoryJournal::cancel`].
    Cancelled(History),
}

/// Structure returned by [`HistoryJournal::finish`] which encodes the stored record identifier
/// and index.
pub struct FinishedCmd {
    pub history_record_id: RecordId,
    pub history_record_idx: RecordIdx,
}

/// In-flight command state held in [`HistoryJournal::active_cmds`].
#[derive(Debug)]
struct InFlightCmd {
    history: History,
    span: Span,
    /// Suppose that a command is in flight -- `active_cmds` holds an entry for it.
    /// Suppose two requests come concurrently -- finish the command and delete the command.
    ///
    /// ```text
    /// finish is supposed to:
    ///   1. x := read(active_cmds, cmd)
    ///      ^--- BORROW (not pop) the command from the shared active_cmds map into the stack.
    ///   2. history_store.push(create(X)).await
    ///      ^--- append a creation event to the history store.
    ///   3. history_db.save(x).await
    ///      ^--- store the command into the history database (new row)
    ///   4. pop(active_cmds, cmd)
    ///      ^--- remove the entry from the active_cmds
    ///
    /// delete is supposed to:
    ///   1. x := pop(active_cmds, cmd)
    ///      ^--- remove the command from the active cmds
    ///       -> Some(x) means that the command was in-flight, in which case we're good to go
    ///       -> None means that the command was already persisted by finish:4
    ///          which means we need to do history_store.push(delete(x))
    /// ```
    ///
    /// BUT! What might happen is that _while_ we're finishing a command (ie. between finish:1 and
    /// finish:4), we get a delete. The delete sees that pop(active_cmds, cmd) is Some, and it pops
    /// it and then exits, thinking that there is nothing else to handle there. But guess what --
    /// the data is just about to be inserted.
    ///
    /// Really, we need a critical section between finish:1-4 and delete:1.
    finalization_mutex: Arc<tokio::sync::Mutex<()>>,
}

/// Takes a command [`HistoryJournal::finish`] has stored the record of out of
/// [`HistoryJournal::active_cmds`] when dropped, whether `finish` ran to the end or was dropped
/// partway. A command whose history.db write didn't succeed (`unsaved`) goes into
/// [`HistoryJournal::unsaved_cmds`] first, so output arriving meanwhile always finds it.
struct Recorded<'j> {
    journal: &'j HistoryJournal,
    id: HistoryId,
    unsaved: Option<(String, String)>,
}

impl Drop for Recorded<'_> {
    fn drop(&mut self) {
        if let Some(cmd) = self.unsaved.take() {
            self.journal.unsaved_cmds.insert(self.id, cmd);
        }
        self.journal.active_cmds.remove(&self.id);
    }
}

/// Increments each id's entry in [`HistoryJournal::deleting`] on creation and decrements it on
/// drop, so `deleting[id] > 0` for exactly as long as some [`HistoryJournal::delete`] is running
/// against `id`.
struct DeletingGuard<'s, 'i> {
    deleting: &'s DashMap<HistoryId, usize>,
    ids: &'i [HistoryId],
    marked: usize,
}

impl Drop for DeletingGuard<'_, '_> {
    fn drop(&mut self) {
        for id in &self.ids[..self.marked] {
            self.deleting.remove_if_mut(id, |_, count| {
                *count -= 1;
                *count == 0
            });
        }
    }
}

/// Registry of in-flight commands which performs output capture, management, storage and retrieval.
#[derive(Debug)]
pub struct HistoryJournal {
    /// Capabilities client used for packing.
    ///
    /// TODO(markovejnovic): This probably shouldn't be injected in [`HistoryJournal`]. Perhaps a
    /// better option is to have a type "`Packer`" which is the type we inject, rather than this
    /// `caps` field.
    caps: Arc<CapClient>,

    /// WAL-style database used to store history entries.
    history_store: HistoryStore,

    /// Database used for storing history entries. This is a rich, typed, CRUD database which
    /// manages the history.
    history_db: HistoryDatabase,

    /// Map which holds all commands which are considered to be in flight.
    ///
    /// An "in-flight" command is a command which has been started, but we're still waiting for it
    /// to be completed.
    active_cmds: DashMap<HistoryId, InFlightCmd>,

    /// Commands [`Self::finish`] stored a record for but couldn't save to the history db, with
    /// their command line and working directory. [`Self::register_command_output`] still accepts output for these, since
    /// rebuilding the history db brings their rows back. [`Self::delete`] forgets them.
    unsaved_cmds: DashMap<HistoryId, (String, String)>,

    /// We hold a reference to the search index which allows us to add a new history record into it.
    search_index: Arc<SharedSearchIndex>,

    /// Channel used to broadcast command events to other threads. See [`CmdEvent`] and
    /// [`Self::subscribe`].
    broadcast: broadcast::Sender<CmdEvent>,

    /// Durable store for captured command output.
    output_capture: OutputCaptureEngine,

    #[cfg(feature = "octavo")]
    octavo: Arc<Octavo>,

    /// Ids a [`Self::delete`] is currently tearing down, reference-counted across concurrent
    /// deletes. [`Self::register_command_output`] refuses these, so no capture can land between a
    /// delete's output removal and its record removal.
    deleting: DashMap<HistoryId, usize>,

    /// Serialises the lifecycle check + write in [`Self::register_command_output`] against the
    /// marking in [`Self::delete`] and the teardown in [`Self::cancel`], one shard per history id.
    ///
    /// Please see the [moduledoc](self) for a (hopefully better) explanation.
    lifecycle_mutex: AsyncShardedMutex<HistoryId>,
}

/// Errors returned by [`HistoryJournal::finish`].
#[derive(Debug, thiserror::Error)]
pub enum CmdFinishError {
    #[error("command {0} is not in flight")]
    NotFound(HistoryId),
    #[error("storing into history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
}

/// Errors returned by [`HistoryJournal::delete`].
#[derive(Debug, thiserror::Error)]
pub enum CmdDeleteError {
    #[error("deleting from history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
    #[error("applying deletion to history db failed: {0}")]
    HistoryDbFailed(eyre::Report),
}

/// The `settings` callback of a search index rebuild that applies `search_settings`. See
/// [`SharedSearchIndex::rebuild`].
fn index_settings(
    search_settings: &Search,
) -> impl Fn(RebuildSettings<'_>) -> Option<SearchIndexSettings> {
    let frecency_multipliers = FrecencyMultipliers::from(search_settings);
    move |settings| {
        Some(SearchIndexSettings {
            frecency_multipliers,
            ..settings.latest.clone()
        })
    }
}

/// What [`HistoryJournal::delete`] did in the record store, before it removes rows from the
/// history db.
struct TombstonedEntries {
    /// How many entries Atuin forgot. That includes started commands that were cancelled. Those
    /// were never in the record store, so this can be more than `to_delete.len()`.
    forgotten: usize,

    /// The ids that got a tombstone. Their rows must be removed from the history db.
    to_delete: Vec<HistoryId>,

    /// The entries of `to_delete` that are in the search index but whose save to the history db
    /// failed.
    unsaved: DeletedLiveRows,
}

/// Errors returned by [`HistoryJournal::rebuild`].
#[derive(Debug, thiserror::Error)]
pub enum CmdRebuildError {
    #[error("rebuilding history db from store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
}

/// Errors returned by [`HistoryJournal::cancel`].
#[derive(Debug, thiserror::Error)]
pub enum CmdCancelError {
    #[error("command {0} is not in flight")]
    NotFound(HistoryId),
}

/// Errors returned by [`HistoryJournal::register_command_output`].
#[derive(Debug, thiserror::Error)]
pub enum RegisterOutputError {
    #[error("command {0} is neither in flight nor persisted; refusing its output")]
    NotLive(HistoryId),
    #[error("checking the history db failed: {0}")]
    HistoryDbFailed(eyre::Report),
    #[error(transparent)]
    Capture(#[from] CaptureError),
}

/// Errors returned by [`HistoryJournal::get`].
#[derive(Debug, thiserror::Error)]
pub enum GetCmdInFlightError {
    #[error("command {0} is not in flight")]
    NotFound(HistoryId),
}

impl HistoryJournal {
    /// Create a new command registry.
    pub fn new(
        caps: Arc<CapClient>,
        history_store: HistoryStore,
        history_db: HistoryDatabase,
        search_index: Arc<SharedSearchIndex>,
        output_capture: OutputCaptureEngine,
    ) -> Self {
        const DEFAULT_LIFECYCLE_SHARDS: NonZeroUsize = NonZeroUsize::new(64).unwrap();

        let (broadcast, _) = broadcast::channel(128);
        Self {
            caps,
            history_store,
            history_db,
            active_cmds: DashMap::new(),
            unsaved_cmds: DashMap::new(),
            search_index,
            broadcast,
            output_capture,
            #[cfg(feature = "octavo")]
            octavo: Arc::new(Octavo::nop()),
            deleting: DashMap::new(),
            lifecycle_mutex: AsyncShardedMutex::new(DEFAULT_LIFECYCLE_SHARDS),
        }
    }

    /// Sends what this journal records to `octavo`, which [`Self::new`] leaves as a nop.
    #[cfg(feature = "octavo")]
    pub(crate) fn with_octavo(self, octavo: Arc<Octavo>) -> Self {
        Self { octavo, ..self }
    }

    /// Notify the registry that a command has been started.
    ///
    /// Returns the [`HistoryId`] identifying the in-flight command, which is later used to
    /// [`HistoryJournal::finish`] or [`HistoryJournal::cancel`] it.
    #[must_use]
    pub fn start_cmd(&self, history: History) -> HistoryId {
        let id = history.id;

        let span = tracing::trace_span!(
            "command",
            history_id = %id,
            command = %history.command,
            exit_code = Empty,
            duration = Empty,
        );

        if self.broadcast.receiver_count() > 0 {
            let _ = self.broadcast.send(CmdEvent::Started(history.clone()));
        }

        self.active_cmds.insert(id, InFlightCmd {
            history,
            span,
            finalization_mutex: Arc::new(tokio::sync::Mutex::new(())),
        });
        id
    }

    /// The in-flight command recorded under `history_id`.
    ///
    /// Returns an owned clone, releasing the map's shard lock before returning, so callers never
    /// hold a borrow into the journal across [`HistoryJournal::finish`] / [`HistoryJournal::cancel`].
    pub fn get(&self, history_id: HistoryId) -> Result<History, GetCmdInFlightError> {
        self.active_cmds
            .get(&history_id)
            .map(|cmd| cmd.history.clone())
            .ok_or(GetCmdInFlightError::NotFound(history_id))
    }

    /// Mark a command as finished, persisting it to the history store and database.
    ///
    /// `duration` is the measured runtime of the command; callers that don't have one can derive it
    /// from the start timestamp via [`HistoryJournal::get`].
    #[expect(
        clippy::significant_drop_tightening,
        reason = "false positive: the index permit is consumed by add_history right after the save"
    )]
    pub async fn finish(
        &self,
        history_id: HistoryId,
        exit_code: i64,
        duration: Duration,
    ) -> Result<FinishedCmd, CmdFinishError> {
        // Careful! We need to ensure that the finalization_mutex gets guarded _while_ under the
        // dashmap lock.
        //
        // Make sure you read the docs of [`ActveCmd::finalization_mutex`].
        let mutex = self
            .active_cmds
            .get(&history_id)
            .map(|cmd| cmd.finalization_mutex.clone())
            .ok_or(CmdFinishError::NotFound(history_id))?;
        let lock = mutex.lock().await;

        let (mut history, span) = {
            let cmd =
                self.active_cmds.get(&history_id).ok_or(CmdFinishError::NotFound(history_id))?;
            (cmd.history.clone(), cmd.span.clone())
        };

        history.exit = exit_code;
        history.duration = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
        span.record("exit_code", exit_code);
        span.record("duration", history.duration);

        // The record store is the source of truth that history.db is built from, so it's written
        // first: once the record is in, the command is recorded even if the history.db write fails.
        let (history_record_id, history_record_idx) = self
            .history_store
            .push(history.clone())
            .instrument(span.clone())
            .await
            .map_err(CmdFinishError::HistoryStoreFailed)?;

        // From here on the command is recorded, so it must leave `active_cmds` even if this future
        // is dropped (the client went away) during the history.db write: a delete would otherwise
        // take it for in flight and not tombstone the record.
        let mut recorded = Recorded {
            journal: self,
            id: history_id,
            unsaved: Some((history.command.clone(), history.cwd.clone())),
        };

        // The entry is registered with any search index rebuild in progress before the row is
        // saved. The rebuilt index then has the entry even if the rebuild's load doesn't read it.
        // See `AddPermit`.
        let index_permit = self.search_index.begin_add().await;
        index_permit.register([&history]);

        // Not an error for the caller: a retry would push a second record. The row comes back the
        // next time history.db is rebuilt from the store.
        match self.history_db.save(&history).instrument(span.clone()).await {
            Ok(()) => recorded.unsaved = None,
            Err(e) => tracing::error!(
                "recorded command {history_id}, but could not save it to the history db; run \
                 `atuin store rebuild history` to restore it: {e}"
            ),
        }

        // The entry is indexed even if the save failed, because the record store has it.
        index_permit.add_history([&history]);

        drop(recorded);
        drop(lock);

        // TODO(markovejnovic): This is a little bit hacked-together. I'm thinking it would be good
        // to have a Packer type for this kind of logic. It can wraps the Caps.
        if let Err(e) = packfile::try_pack(
            &self.history_store.store,
            &RecordSeriesKey::new(self.history_store.host_id, RecordTag::History),
            self.caps.get_server::<PackfileCap>().await.ok().flatten(),
        )
        .instrument(span)
        .await
        {
            tracing::warn!("packing failed: {e}");
        }

        #[cfg(feature = "octavo")]
        self.octavo.push_history(&history, history_record_id).await;

        if self.broadcast.receiver_count() > 0 {
            let _ = self.broadcast.send(CmdEvent::Finished(history));
        }

        Ok(FinishedCmd {
            history_record_id,
            history_record_idx,
        })
    }

    /// Cancel a command, discarding its in-memory state -- and any output captured for it.
    pub async fn cancel(&self, history_id: HistoryId) -> Result<(), CmdCancelError> {
        let _lifecycle = self.lifecycle_mutex.lock(&history_id).await;

        let lock = self
            .active_cmds
            .get(&history_id)
            .map(|cmd| cmd.finalization_mutex.clone())
            .ok_or(CmdCancelError::NotFound(history_id))?;
        let _guard = lock.lock().await;

        if !self.active_cmds.contains_key(&history_id) {
            return Err(CmdCancelError::NotFound(history_id));
        }

        if let Err(err) = self.output_capture.remove([history_id]).await {
            tracing::error!(
                %history_id,
                ?err,
                "failed to discard the captured output of a cancelled command"
            );
        }

        let (_id, cmd) = self
            .active_cmds
            .remove(&history_id)
            .expect("still present: re-checked under the finalization mutex every remover holds");

        let _ = self.broadcast.send(CmdEvent::Cancelled(cmd.history));

        Ok(())
    }

    /// Delete the given history entries from Atuin's memory completely, including any captured
    /// output they have, and refuse output for them from then on.
    ///
    /// The search index rebuilds just the commands that lost a run. It reloads whole when it can't:
    /// when the delete touches too many commands or runs, when the index is being rebuilt, or when
    /// an entry's row was already gone. `search_settings` is needed to rebuild the index's frecency
    /// after the deletion. The index then has correct rankings immediately rather than after the
    /// next refresh.
    ///
    /// Returns how many history entries Atuin forgot.
    ///
    /// This function is serialized on the **bundle** of the `ids` you're passing in, eg. if you
    /// pass `ids = [1, 2]` and then in parallel another delete call with `ids = [2, 3]`, the latter
    /// will wait for the former request to completely go through.
    pub async fn delete(
        &self,
        ids: &[HistoryId],
        search_settings: &Search,
    ) -> Result<usize, CmdDeleteError> {
        // This delete guard is important since it serializes, per HistoryId, the deletion of
        // history IDs.
        //
        // In effect, if concurrent `delete`s come through here, they'll have to wait on ids.
        let _delete_guard = self.guard_deleting(ids).await;

        if let Err(err) = self.output_capture.remove(ids.iter().copied()).await {
            tracing::error!(
                ?ids,
                ?err,
                "failed to remove captured output while deleting history; deleting the entry \
                 anyway. the orphaned output will be reclaimed by the store's garbage collector",
            );
        }

        // Append delete tombstones to the record store. See `TombstonedEntries`.
        //
        // The tombstones go into the store as one batch: per-record pushes cost three sqlite
        // round-trips each, which made large deletes take minutes.
        let delete_records = async || {
            let mut deleted: usize = 0;
            let mut to_delete = Vec::with_capacity(ids.len());
            for &id in ids {
                let mutex = self.active_cmds.get(&id).map(|cmd| cmd.finalization_mutex.clone());
                let cancelled = if let Some(mutex) = mutex {
                    let _lock = mutex.lock().await;
                    match self.active_cmds.remove(&id) {
                        Some((_id, cmd)) => {
                            let _ = self.broadcast.send(CmdEvent::Cancelled(cmd.history));
                            true
                        }
                        None => false,
                    }
                } else {
                    false
                };

                if cancelled {
                    deleted += 1;
                } else {
                    to_delete.push(id);
                }
            }

            self.history_store
                .delete_batch(to_delete.iter().copied())
                .await
                .map_err(CmdDeleteError::HistoryStoreFailed)?;
            deleted += to_delete.len();
            // These entries are in the search index but not in the history db. Their commands are
            // kept, so the index update below finds them.
            let mut unsaved = DeletedLiveRows::default();
            for id in &to_delete {
                if let Some((id, (command, _cwd))) = self.unsaved_cmds.remove(id) {
                    unsaved.add_unsaved(id, command);
                }
            }

            Ok(TombstonedEntries {
                forgotten: deleted,
                to_delete,
                unsaved,
            })
        };

        let TombstonedEntries {
            forgotten: deleted,
            to_delete,
            unsaved: mut removed,
        } = delete_records().await?;
        if to_delete.is_empty() {
            return Ok(deleted);
        }

        // The tombstones commit the deletion, so Octavo hears of it even if a chunk below fails.
        #[cfg(feature = "octavo")]
        self.octavo.delete_history(&to_delete).await;

        // The tombstones were just written above, so replaying them through
        // `HistoryStore::build_all` would only read and decrypt them back into these same ids.
        // Chunked like `build_all` is: one transaction over the whole set would hold the history
        // db's write lock for seconds at 100k+ ids, past the busy timeout a concurrent
        // `finish()` is willing to wait.
        let mut result = Ok(deleted);
        let mut attempted = 0;
        for chunk in to_delete.chunks(DELETE_ROWS_BATCH_SIZE) {
            match self.history_db.delete_rows(chunk.iter().copied()).await {
                Ok(chunk_removed) => {
                    removed.extend(chunk_removed);
                    attempted += chunk.len();
                }
                Err(e) => {
                    result = Err(CmdDeleteError::HistoryDbFailed(e.into()));
                    break;
                }
            }
        }

        // The index is updated even if a chunk failed. The chunks before it removed their rows, and
        // a retry can't learn their commands any more. The failed chunk and the chunks after it
        // removed nothing. Only the ids of the chunks that ran are checked for rows that were
        // already gone.
        let missing = to_delete[..attempted].iter().filter(|&&id| !removed.contains(id)).count();
        if missing > 0 {
            // Some rows were already gone from the history db, but their commands may still be in
            // the index. Only a reload can drop them. An unknown id costs a reload too.
            tracing::info!(
                "{missing} deleted ids had no live row in the history db, reloading the whole \
                 search index"
            );
            self.reload_search_index(search_settings).await;
        } else if !removed.is_empty() {
            let settings = index_settings(search_settings);
            self.search_index
                .sync_deleted(&self.history_db, removed.into_commands(), settings)
                .await;
        }

        result
    }

    /// Rebuild the history db from the record store, then reload the search index from it.
    pub async fn rebuild(&self, search_settings: &Search) -> Result<(), CmdRebuildError> {
        self.history_store
            .build(&self.history_db)
            .await
            .map_err(CmdRebuildError::HistoryStoreFailed)?;

        self.reload_search_index(search_settings).await;

        Ok(())
    }

    /// Reload the search index from the history database.
    pub async fn reload_search_index(&self, search_settings: &Search) {
        let rebuilt = self.search_index.rebuild(&self.history_db, index_settings(search_settings));
        if let Err(e) = rebuilt.await {
            // TODO(markovejnovic): This is obviously incorrect behavior, keeping the previous
            //                      index is almost certainly wrong, however, is the legacy
            //                      behavior we had.
            //
            //                      Arguably, we could completely delete the index/crash the
            //                      daemon, since at this point, Atuin is as good as useless.
            //
            //                      We could also just mark the index as hot garbo and have the
            //                      next request attempt to rebuild it. Not sure why the next
            //                      request would succeed but a retry is always good.
            tracing::error!("failed to reload search index; keeping previous index: {e}");
        }
    }

    /// Claim `ids` for a running delete. See [`DeletingGuard`].
    async fn guard_deleting<'i>(&self, ids: &'i [HistoryId]) -> DeletingGuard<'_, 'i> {
        let mut marks = DeletingGuard {
            deleting: &self.deleting,
            ids,
            marked: 0,
        };

        for &id in ids {
            let _lifecycle = self.lifecycle_mutex.lock(&id).await;
            *self.deleting.entry(id).or_insert(0) += 1;
            marks.marked += 1;
        }

        marks
    }

    /// Store a command's captured output as `output` allows: nothing while capture is disabled,
    /// nothing for a command its `command_filter` matches, and nothing for one that may print a
    /// credential (`atuin key`, `gh auth token`) or names one of `files` (`cat .env`).
    ///
    /// If the output is received for an unknown command, this returns a
    /// [`RegisterOutputError::NotLive`].
    pub async fn register_command_output(
        &self,
        id: HistoryId,
        capture: CommandCapture,
        output: &OutputCapture,
        files: &SensitiveFiles,
    ) -> Result<(), RegisterOutputError> {
        let _lifecycle = self.lifecycle_mutex.lock(&id).await;

        if self.deleting.contains_key(&id) {
            return Err(RegisterOutputError::NotLive(id));
        }

        // Resolve the command backing this id: an in-flight entry wins, then a recorded command
        // missing from the history db, otherwise the (non-deleted) history row. `None` means the
        // command is gone or already deleted.
        let command = if let Some(cmd) = self.active_cmds.get(&id) {
            Some((cmd.history.command.clone(), cmd.history.cwd.clone()))
        } else if let Some(cmd) = self.unsaved_cmds.get(&id) {
            Some(cmd.clone())
        } else {
            self.history_db
                .load(id)
                .await
                .map_err(|e| RegisterOutputError::HistoryDbFailed(e.into()))?
                .filter(|h| h.deleted_at.is_none())
                .map(|h| (h.command, h.cwd))
        };

        let Some((command, cwd)) = command else {
            return Err(RegisterOutputError::NotLive(id));
        };

        // Capture turned off on a running daemon: proxies started before it keep sending output
        // until their shells restart.
        let OutputCapture::Enabled(limits) = output else {
            return Ok(());
        };

        // Never persist output for commands that may carry secrets, or that the user excluded.
        if atuin_common::secrets::output_unsafe(&command)
            || limits.command_filter.is_match(&command)
            || files.named_in_command(&command, Some(std::path::Path::new(&cwd)))
        {
            return Ok(());
        }

        self.output_capture.capture(id, capture).await?;
        // Only once stored, since the upload reads the output back from the store.
        #[cfg(feature = "octavo")]
        self.octavo.push_output(id).await;
        Ok(())
    }

    /// Retrieve a command's captured output, if any.
    pub async fn get_command_output(
        &self,
        id: HistoryId,
    ) -> Result<Option<CommandCapture>, GetOutputError> {
        self.output_capture.get(id).await
    }

    /// Create a new stream of [`CmdEvent`] objects.
    ///
    /// Note that the resulting channel is potentially lossy -- if there is too much backpressure on
    /// any subscriber, there is potential for loss of data.
    #[must_use]
    pub fn subscribe(&self) -> BroadcastStream<CmdEvent> {
        BroadcastStream::new(self.broadcast.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{CmdOrigin, HostId};
    use rstest::{fixture, rstest};

    use super::*;
    use crate::search::SearchIndex;

    /// A journal wired to real temp stores, so `finish`/`delete` run for real against a
    /// caller-chosen output-capture backend. The returned `TempDir` must outlive the journal.
    async fn journal(output_capture: OutputCaptureEngine) -> (HistoryJournal, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let timeout = Duration::from_secs(5);
        let history_db =
            HistoryDatabase::new(tmp.path().join("history.db"), timeout).await.unwrap();
        let store = SqliteStore::new(tmp.path().join("records.db"), timeout).await.unwrap();
        let history_store = HistoryStore::new(store, HostId(uuid_v7()), paseto_v4::Key::generate());
        let search_index = Arc::new(SearchIndex::default().into());
        let caps = CapClient::new("http://127.0.0.1:1".parse().unwrap(), reqwest::Client::new());
        let journal =
            HistoryJournal::new(caps, history_store, history_db, search_index, output_capture);
        (journal, tmp)
    }

    fn entry(cmd: &str) -> History {
        History::daemon()
            .timestamp(jiff::Timestamp::now())
            .command(cmd)
            .cwd("/tmp")
            .session(uuid_v7().as_simple().to_string())
            .cmd_origin(CmdOrigin::try_from("test-host:test-user").unwrap())
            .shell("bash")
            .author("test-user")
            .build()
            .into()
    }

    /// A broken output store must not sink a deletion: the entry the user asked to forget is still
    /// removed rather than the whole delete being refused.
    #[rstest]
    #[tokio::test]
    async fn delete_survives_a_broken_output_store() {
        let (journal, _tmp) = journal(OutputCaptureEngine::failing()).await;
        let id = journal.start_cmd(entry("echo goodbye"));
        journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();

        let deleted = journal
            .delete(&[id], &Search::default())
            .await
            .expect("a failed output removal must not fail the whole delete");
        assert_eq!(deleted, 1);
        assert!(journal.history_db.load(id).await.unwrap().is_none());
    }

    /// Run `sql` against the history db in `tmp`.
    async fn exec_on_history_db(tmp: &tempfile::TempDir, sql: &'static str) {
        let mut conn = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(tmp.path().join("history.db")),
        )
        .await
        .unwrap();
        atuin_common::db::query(sql).execute(&mut conn).await.unwrap();
        sqlx::Connection::close(conn).await.unwrap();
    }

    /// Make every save to the history db in `tmp` fail.
    async fn fail_saves(tmp: &tempfile::TempDir) {
        exec_on_history_db(
            tmp,
            "create trigger fail_saves before insert on history begin select raise(abort, 'no'); \
             end",
        )
        .await;
    }

    /// Undo [`fail_saves`].
    async fn allow_saves(tmp: &tempfile::TempDir) {
        exec_on_history_db(tmp, "drop trigger fail_saves").await;
    }

    /// A journal whose history db fails every save.
    struct FailingSaves {
        journal: HistoryJournal,
        tmp: tempfile::TempDir,
    }

    #[fixture]
    async fn failing_saves() -> FailingSaves {
        let (journal, tmp) = journal(OutputCaptureEngine::failing()).await;
        fail_saves(&tmp).await;
        FailingSaves { journal, tmp }
    }

    /// Deleting an entry whose history.db save failed updates the search index in place. The
    /// entry has no row, but it isn't mistaken for a row that is already gone, which would reload
    /// the whole index. Saved runs of the same command stay.
    #[rstest]
    #[case::only_run(0)]
    #[case::other_runs_saved(2)]
    #[tokio::test]
    async fn delete_of_an_unsaved_entry_syncs_the_index(#[case] saved_runs: u32) {
        let (journal, tmp) = journal(OutputCaptureEngine::failing()).await;
        for _ in 0..saved_runs {
            let id = journal.start_cmd(entry("echo x"));
            journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
        }
        // Only the index has this entry. A reload of the index would drop it.
        let sentinel = entry("index-only sentinel");
        {
            let permit = journal.search_index.begin_add().await;
            permit.register([&sentinel]);
            permit.add_history([&sentinel]);
        }
        fail_saves(&tmp).await;

        let unsaved = journal.start_cmd(entry("echo x"));
        journal.finish(unsaved, 0, Duration::from_millis(1)).await.unwrap();
        assert!(journal.history_db.load(unsaved).await.unwrap().is_none(), "the save failed");
        assert_eq!(journal.search_index.read().invocations("echo x"), saved_runs + 1);

        journal.delete(&[unsaved], &Search::default()).await.unwrap();

        let index = journal.search_index.read();
        assert_eq!(index.invocations("echo x"), saved_runs);
        assert_eq!(index.invocations("index-only sentinel"), 1, "the index wasn't reloaded");
    }

    /// A delete that names an id twice reports it twice, as every id that it processed counts.
    /// But it updates the index in place, because no row is missing.
    #[rstest]
    #[tokio::test]
    async fn delete_of_a_repeated_id_updates_the_index_in_place() {
        let (journal, _tmp) = journal(OutputCaptureEngine::failing()).await;
        let id = journal.start_cmd(entry("echo twice"));
        journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
        // Only the index has this entry. A reload of the index would drop it.
        let sentinel = entry("index-only sentinel");
        {
            let permit = journal.search_index.begin_add().await;
            permit.register([&sentinel]);
            permit.add_history([&sentinel]);
        }

        let deleted = journal.delete(&[id, id], &Search::default()).await.unwrap();

        assert_eq!(deleted, 2);
        let index = journal.search_index.read();
        assert_eq!(index.invocations("echo twice"), 0);
        assert_eq!(index.invocations("index-only sentinel"), 1, "the index wasn't reloaded");
    }

    /// An entry whose save failed and whose row came back later counts once in a delete. So it
    /// can't hide another deleted id whose row was already gone. That id's command is only dropped
    /// from the index by a reload.
    #[rstest]
    #[tokio::test]
    async fn an_unsaved_entry_with_a_row_counts_once(
        #[future(awt)]
        #[from(failing_saves)]
        saves: FailingSaves,
    ) {
        let FailingSaves { journal, tmp } = saves;
        let unsaved = entry("echo unsaved");
        let id = journal.start_cmd(unsaved.clone());
        journal.finish(id, 0, Duration::from_millis(1)).await.unwrap();
        assert!(journal.history_db.load(id).await.unwrap().is_none(), "the save failed");
        // A rebuild of the history db from the store brings the row back.
        allow_saves(&tmp).await;
        journal.history_db.save(&unsaved).await.unwrap();

        // An earlier delete removed this row but not its command from the index.
        let gone_id = journal.start_cmd(entry("echo gone"));
        journal.finish(gone_id, 0, Duration::from_millis(1)).await.unwrap();
        journal.history_db.delete_rows([gone_id]).await.unwrap();
        assert_eq!(journal.search_index.read().invocations("echo gone"), 1);

        journal.delete(&[id, gone_id], &Search::default()).await.unwrap();

        let index = journal.search_index.read();
        assert_eq!(index.invocations("echo gone"), 0, "the index was reloaded");
        assert_eq!(index.invocations("echo unsaved"), 0);
    }

    /// The state of the history db row of the entry in a delete's first chunk.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum FirstRow {
        Live,
        AlreadyGone,
    }

    /// A delete whose second chunk fails still updates the index for the first chunk. A row in the
    /// first chunk that was already gone reloads the whole index, as it does after a delete that
    /// succeeds.
    #[rstest]
    #[case::row_already_gone(FirstRow::AlreadyGone, 0)]
    #[case::all_rows_live(FirstRow::Live, 1)]
    #[tokio::test]
    async fn delete_with_a_failed_chunk_updates_the_index(
        #[case] first_row: FirstRow,
        #[case] sentinels_left: u32,
    ) {
        let (journal, tmp) = journal(OutputCaptureEngine::failing()).await;
        let first = journal.start_cmd(entry("echo first"));
        journal.finish(first, 0, Duration::from_millis(1)).await.unwrap();
        let poison = journal.start_cmd(entry("echo poison"));
        journal.finish(poison, 0, Duration::from_millis(1)).await.unwrap();
        // Only the index has this entry. A reload of the index would drop it.
        let sentinel = entry("index-only sentinel");
        {
            let permit = journal.search_index.begin_add().await;
            permit.register([&sentinel]);
            permit.add_history([&sentinel]);
        }
        if first_row == FirstRow::AlreadyGone {
            // An earlier delete removed this row but not its command from the index.
            journal.history_db.delete_rows([first]).await.unwrap();
        }
        exec_on_history_db(
            &tmp,
            "create trigger fail_poison before delete on history when old.command = 'echo poison' \
             begin select raise(abort, 'no'); end",
        )
        .await;
        // The repeats fill the first chunk, so `poison` lands in the second one.
        let mut ids = vec![first; DELETE_ROWS_BATCH_SIZE];
        ids.push(poison);

        journal.delete(&ids, &Search::default()).await.expect_err("the second chunk fails");

        let index = journal.search_index.read();
        assert_eq!(index.invocations("echo first"), 0);
        assert_eq!(index.invocations("echo poison"), 1, "the failed chunk removed nothing");
        assert_eq!(index.invocations("index-only sentinel"), sentinels_left);
    }
}
