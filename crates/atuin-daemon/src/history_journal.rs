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

use std::sync::Arc;
use std::time::Duration;

use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{History, HistoryId};
use atuin_client::packfile;
use atuin_client::settings::Search;
use atuin_domain::caps::{CapClient, PackfileCap};
use atuin_domain::record::{RecordId, RecordIdx, RecordSeriesKey, RecordTag};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::field::Empty;
use tracing::{Instrument, Span};

use crate::grpc::history::pb::CommandCapture;
use crate::output_capture::{CaptureError, GetOutputError, OutputCapture};
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

    /// Cancel a command, discarding its in-memory state without persisting a history entry.
    pub async fn cancel(&self, history_id: HistoryId) -> Result<(), CmdCancelError> {
        let lock = self
            .active_cmds
            .get(&history_id)
            .map(|cmd| cmd.finalization_mutex.clone())
            .ok_or(CmdCancelError::NotFound(history_id))?;
        let _guard = lock.lock().await;

        let Some((_id, cmd)) = self.active_cmds.remove(&history_id) else {
            return Err(CmdCancelError::NotFound(history_id));
        };

        let _ = self.broadcast.send(CmdEvent::Cancelled(cmd.history));

        Ok(())
    }

    /// Delete the given history entries from Atuin's memory completely.
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

    /// Store a command's captured output. Errors if an output already exists for this id.
    pub async fn register_command_output(
        &self,
        id: HistoryId,
        capture: CommandCapture,
    ) -> Result<(), CaptureError> {
        self.output_capture.capture(id, capture).await
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
