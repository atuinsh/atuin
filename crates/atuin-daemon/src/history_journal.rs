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
//! Commands-in-flight are commands which have been started but have just started running. These
//! commands are uniquely identified by their [`HistoryId`].
//!
//! Commands in flight can be terminated in one of two ways:
//!
//!   - [`HistoryJournal::finish`] marks the command as finished, which will create and store a new
//!     history entry.
//!   - [`HistoryJournal::cancel`] cancels the command, disposing of any in-memory resources, but
//!     without the logic of persisting the history entry.
//!
//! ## Streaming
//!
//! It is possible to stream events out of [`HistoryJournal`] via [`HistoryJournal::subscribe`]
//! which returns a new [`futures::Stream`] of [`CmdEvent`] events.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{CommandCapture, History, HistoryId};
use atuin_client::packfile;
use atuin_client::settings::Search;
use atuin_common::sync::StripedMutex;
use atuin_domain::caps::{CapClient, PackfileCap};
use atuin_domain::record::{RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::output_capture::{CaptureError, DeleteOutputError, GetOutputError, OutputCapture};
use crate::search::SearchIndex;

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
    ///   2. history_db.save(x).await
    ///      ^--- store the command into the history database (new row)
    ///   3. history_store.push(create(X)).await
    ///      ^--- append a creation event to the history store.
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

/// The ids a running [`HistoryJournal::delete`] has claimed, released on drop.
///
/// Reference-counted in [`HistoryJournal::deleting`] so that two concurrent deletes of the same id
/// keep it claimed until *both* are done: a delete that fails part-way must not unmark an id
/// another delete is still tearing down.
struct DeletingMarks<'a> {
    deleting: &'a DashMap<HistoryId, usize>,
    ids: Vec<HistoryId>,
}

impl Drop for DeletingMarks<'_> {
    fn drop(&mut self) {
        for id in &self.ids {
            // Drop the entry once this was the last delete holding the id.
            self.deleting.remove_if_mut(id, |_, count| {
                *count -= 1;
                *count == 0
            });
        }
    }
}

/// How many stripes back [`HistoryJournal::liveness_mutex`]. History ids are UUIDv7 and hash
/// evenly, so collisions only cost a little waiting.
const LIVENESS_STRIPES: NonZeroUsize = NonZeroUsize::new(64).unwrap();

/// Registry of in-flight commands which performs output capture, management, storage and
/// retrieval.
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

    /// We hold a reference to the search index which allows us to add a new history record into it.
    search_index: Arc<tokio::sync::RwLock<SearchIndex>>,

    /// Channel used to broadcast command events to other threads. See [`CmdEvent`] and
    /// [`Self::subscribe`].
    broadcast: broadcast::Sender<CmdEvent>,

    /// Durable store for captured command output.
    output_capture: OutputCapture,

    /// Ids a [`Self::delete`] is currently tearing down, reference-counted across concurrent
    /// deletes. [`Self::register_command_output`] refuses these, so no capture can land between a
    /// delete's output removal and its record removal.
    deleting: DashMap<HistoryId, usize>,

    /// Serialises the liveness check + write in [`Self::register_command_output`] against the
    /// marking in [`Self::delete`] and the teardown in [`Self::cancel`].
    liveness_mutex: StripedMutex<HistoryId, ()>,
}

/// Errors returned by [`HistoryJournal::finish`].
#[derive(Debug, thiserror::Error)]
pub enum CmdFinishError {
    #[error("command {0} is not in flight")]
    NotFound(HistoryId),
    #[error("storing into history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
    #[error("storing into history db failed: {0}")]
    HistoryDbFailed(eyre::Report),
}

/// Errors returned by [`HistoryJournal::delete`].
#[derive(Debug, thiserror::Error)]
pub enum CmdDeleteError {
    #[error("deleting captured output failed: {0}")]
    OutputCaptureFailed(#[from] DeleteOutputError),
    #[error("deleting from history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
    #[error("applying deletion to history db failed: {0}")]
    HistoryDbFailed(eyre::Report),
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
        search_index: Arc<tokio::sync::RwLock<SearchIndex>>,
        output_capture: OutputCapture,
    ) -> Self {
        let (broadcast, _) = broadcast::channel(128);
        Self {
            caps,
            history_store,
            history_db,
            active_cmds: DashMap::new(),
            search_index,
            broadcast,
            output_capture,
            deleting: DashMap::new(),
            liveness_mutex: StripedMutex::new(LIVENESS_STRIPES),
        }
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

        self.active_cmds.insert(id, InFlightCmd {
            history: history.clone(),
            span,
            finalization_mutex: Arc::new(tokio::sync::Mutex::new(())),
        });
        let _ = self.broadcast.send(CmdEvent::Started(history));
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

        self.history_db
            .save(&history)
            .instrument(span.clone())
            .await
            .map_err(|e| CmdFinishError::HistoryDbFailed(e.into()))?;

        let (history_record_id, history_record_idx) = self
            .history_store
            .push(history.clone())
            .instrument(span.clone())
            .await
            .map_err(CmdFinishError::HistoryStoreFailed)?;

        self.active_cmds.remove(&history_id);

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

        // TODO(#4052): This is inherently racy -- any add_history operations added between this
        //              .read() and the subsequent .write() are completely discarded from the new
        //              index.
        self.search_index.read().await.add_history(&history);

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
        // There is a really nasty concurrency issue we have to handle...
        //
        // Firstly, note that it is possible for **multiple** cancels/finishes to through at the
        // same time. Additionally, while we're cancelling, we might receive a call to
        // `register_command_output`.
        //
        // This puts us in a nasty position -- the `register_command_output` can regiser
        let _liveness = self.liveness_mutex.lock(&history_id).await;

        let lock = self
            .active_cmds
            .get(&history_id)
            .map(|cmd| cmd.finalization_mutex.clone())
            .ok_or(CmdCancelError::NotFound(history_id))?;
        let _guard = lock.lock().await;

        if !self.active_cmds.contains_key(&history_id) {
            return Err(CmdCancelError::NotFound(history_id));
        }

        // Nothing durable refers to a cancelled command, so a buffered removal is enough. The
        // shell fires cancels and forgets them, so a storage failure here is retried by nobody:
        // log it and cancel anyway rather than leave the command in flight forever. Any output
        // that stays behind is the retention sweep's to reclaim.
        if let Err(err) = self.output_capture.discard([history_id]).await {
            tracing::warn!(
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
    /// `search_settings` is needed to rebuild the search index's frecency map after the deletion,
    /// so the swapped-in index has correct rankings immediately rather than after the next refresh.
    ///
    /// Returns how many history entries Atuin forgot.
    pub async fn delete(
        &self,
        ids: impl IntoIterator<Item = HistoryId>,
        search_settings: &Search,
    ) -> Result<usize, CmdDeleteError> {
        let ids: Vec<HistoryId> = ids.into_iter().collect();

        // Claim every id before touching anything: from here on `register_command_output` refuses
        // them, so no capture can land between the output removal below and the record removal
        // after it. Marking takes the same per-id gate the register path holds across its
        // check-and-write, so a capture that passed its check is stored by the time its id is
        // marked, and the removal below sees it. The marks are released when `_marks` drops, on
        // success or on any early return.
        let _marks = self.mark_deleting(&ids).await;

        // Forget captured output before the history records. This order means a failed call can
        // leave an entry without its output but never output without its entry.
        //
        // Eh, it's not great, but without some sort of STM, we can't do better.
        //
        // TODO(markovejnovic): Implement STM
        self.output_capture.delete(ids.iter().copied()).await?;

        // Remove records from the record store.
        //
        // This returns a tuple where the first element is the total number of history elements that
        // were erased from Atuin's memory, and the second element is a vector of [`RecordId`]s that
        // must be subsequently removed from the history database via [`HistoryStore::build_all`].
        // Note the passed database argument.
        //
        // Furthermore, note that `.0 != .1.len()`, because there may very well be history entries
        // that atuin has forgotten about that were never in the record store.
        //
        // This happens as a result of the fact that [`HistoryJournal`] might be tracking started,
        // but not finished commands. These get cancelled via [`HistoryJournal::cancel`].
        let delete_records = async || {
            let mut deleted: usize = 0;
            let mut record_ids = Vec::new();
            for id in ids {
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
                    continue;
                }

                match self.history_store.delete(id).await {
                    Ok((record_id, _)) => {
                        record_ids.push(record_id);
                        deleted += 1;
                    }
                    Err(e) => {
                        return Err(CmdDeleteError::HistoryStoreFailed(e));
                    }
                }
            }

            Ok((deleted, record_ids))
        };

        let (deleted, record_ids) = delete_records().await?;
        if record_ids.is_empty() {
            return Ok(deleted);
        }

        self.history_store
            .build_all(&self.history_db, &record_ids)
            .await
            .map_err(CmdDeleteError::HistoryDbFailed)?;

        self.reload_search_index(search_settings).await;

        Ok(deleted)
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
    async fn reload_search_index(&self, search_settings: &Search) {
        // Clone the shell filter and drop the read guard before the (full) reload, so the scan
        // doesn't hold the search-index lock across the database load.
        let shells = self.search_index.read().await.shells.clone();
        let rebuilt = SearchIndex::from_db(shells, &self.history_db, search_settings).await;
        match rebuilt {
            Ok(new_index) => *self.search_index.write().await = new_index,
            Err(e) => {
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
    }

    /// Claim `ids` for a running delete. See [`DeletingMarks`].
    ///
    /// The guard exists before the first id is marked, so a future dropped part-way through still
    /// releases every mark it took.
    async fn mark_deleting(&self, ids: &[HistoryId]) -> DeletingMarks<'_> {
        let mut marks = DeletingMarks {
            deleting: &self.deleting,
            ids: Vec::with_capacity(ids.len()),
        };
        for id in ids {
            let _liveness = self.liveness_mutex.lock(id).await;
            *self.deleting.entry(*id).or_insert(0) += 1;
            marks.ids.push(*id);
        }
        marks
    }

    /// Store a command's captured output.
    ///
    /// If the output is received for an unknown command, this returns a
    /// [`RegisterOutputError::NotLive`].
    pub async fn register_command_output(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), RegisterOutputError> {
        let _liveness = self.liveness_mutex.lock(&id).await;

        if self.deleting.contains_key(&id) {
            return Err(RegisterOutputError::NotLive(id));
        }

        let live = self.active_cmds.contains_key(&id)
            || self
                .history_db
                .load(id)
                .await
                .map_err(|e| RegisterOutputError::HistoryDbFailed(e.into()))?
                .is_some_and(|h| h.deleted_at.is_none());

        if !live {
            return Err(RegisterOutputError::NotLive(id));
        }

        self.output_capture.capture(id, capture).await?;
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
