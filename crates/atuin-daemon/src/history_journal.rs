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

use std::ops::{Deref, DerefMut};
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

/// RAII lease of a temporarily checked-out [`HistoryJournal`]'s in-flight map entry.
///
/// While the lease is alive the command has been removed from the map. If it is dropped without
/// [`ActiveCmdLease::commit`], the command is returned to the map (rolled back).
struct ActiveCmdLease<'a> {
    map: &'a DashMap<HistoryId, InFlightCmd>,
    cmd: Option<InFlightCmd>,
}

impl<'a> ActiveCmdLease<'a> {
    /// Remove the command identified by `id` from `map`, returning a lease that restores it on drop
    /// unless [`Self::commit`] is called. Returns [`None`] when no such command is in flight.
    fn take(map: &'a DashMap<HistoryId, InFlightCmd>, id: HistoryId) -> Option<Self> {
        map.remove(&id).map(|(_id, cmd)| Self {
            map,
            cmd: Some(cmd),
        })
    }

    /// The command's tracing span, which traces its lifetime.
    fn span(&self) -> &Span {
        &self.cmd.as_ref().expect("command is present until commit or drop").span
    }

    /// Consume the lease, keeping the command out of the map and returning ownership of it.
    ///
    /// Call this once all fallible work has succeeded and the command should no longer be considered
    /// in flight.
    fn commit(mut self) -> InFlightCmd {
        self.cmd.take().expect("command is present until commit or drop")
    }
}

impl Deref for ActiveCmdLease<'_> {
    type Target = History;

    fn deref(&self) -> &Self::Target {
        &self.cmd.as_ref().expect("command is present until commit or drop").history
    }
}

impl DerefMut for ActiveCmdLease<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cmd.as_mut().expect("command is present until commit or drop").history
    }
}

impl Drop for ActiveCmdLease<'_> {
    fn drop(&mut self) {
        if let Some(cmd) = self.cmd.take() {
            self.map.insert(cmd.history.id, cmd);
        }
    }
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
        });
        let _ = self.broadcast.send(CmdEvent::Started(history));
        id
    }

    /// Borrows an in-flight command from [`Self::active_cmds`] with an [`ActiveCmdLease`]
    /// guard, which achieves two things:
    ///
    ///   - [`ActiveCmdLease::commit`] removes it from [`Self::active_cmds`].
    ///   - [`Drop`] "rolls the command back", placing it back into [`Self::active_cmds`].
    fn checkout(&self, history_id: HistoryId) -> Option<ActiveCmdLease<'_>> {
        ActiveCmdLease::take(&self.active_cmds, history_id)
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
        let mut session = self.checkout(history_id).ok_or(CmdFinishError::NotFound(history_id))?;

        session.exit = exit_code;
        session.duration = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);

        let span = session.span().clone();
        span.record("exit_code", exit_code);
        span.record("duration", session.duration);

        // Each `?` below drops `session`, which restores it to the in-flight map, so a failed (or
        // cancelled) persistence leaves the command in flight instead of dropping it.
        self.history_db
            .save(&session)
            .instrument(span.clone())
            .await
            .map_err(|e| CmdFinishError::HistoryDbFailed(e.into()))?;

        let (history_record_id, history_record_idx) = self
            .history_store
            .push(session.clone())
            .instrument(span.clone())
            .await
            .map_err(CmdFinishError::HistoryStoreFailed)?;

        // Persistence succeeded; take ownership and stop treating the command as in flight.
        let history = session.commit().history;

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
    pub fn cancel(&self, history_id: HistoryId) -> Result<(), CmdCancelError> {
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
                if let Some((_id, cmd)) = self.active_cmds.remove(&id) {
                    let _ = self.broadcast.send(CmdEvent::Cancelled(cmd.history));
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
