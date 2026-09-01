//! Exposes logic for managing the lifecycle of commands.
//!
//! The core structure is [`HistoryJournal`]. The [`HistoryJournal`] handles the creation and
//! termination of new commands.
//!
//! When users run new commands in the client, the client sends requests to the GPRC server. The
//! [`crate::grpc::history::Service`] then "forwards" the request down into [`HistoryJournal`] --
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
use atuin_common::time::OffsetDateTimeExt;
use atuin_domain::caps::{CapClient, PackfileCap};
use atuin_domain::record::{RecordSeriesKey, RecordTag};
use dashmap::DashMap;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::SemanticComponent;
use crate::search::SearchIndex;

#[derive(Debug)]
struct CmdInFlightOwned {
    history: History,
}

/// An event describing a change in the lifecycle of a command.
#[derive(Debug, Clone)]
pub enum CmdEvent {
    Started(History),
    Finished(History),
    Cancelled(History),
}

/// Registry of in-flight commands which performs output capture, management, storage and
/// retrieval.
#[derive(Debug)]
pub struct HistoryJournal {
    caps: Arc<CapClient>,
    history_store: HistoryStore,
    history_db: HistoryDatabase,

    active_sessions: DashMap<HistoryId, CmdInFlightOwned>,

    /// We hold a reference to the search index which allows us to add a new history record into it.
    ///
    /// Do note that the [`tokio::sync::RwLock`] is only ever used in reader mode.
    search_index: Arc<tokio::sync::RwLock<SearchIndex>>,

    /// Just like the search_index, we need to notify the SemanticComponent of added history.
    semantic_component: SemanticComponent,

    /// Channel used to broadcast command events to other threads. See [`CmdEvent`] and
    /// [`Self::subscribe`].
    broadcast: broadcast::Sender<CmdEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum CmdFinishError {
    #[error("command {0} is not in flight")]
    NotFound(HistoryId),
    #[error("storing into history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
    #[error("storing into history db failed: {0}")]
    HistoryDbFailed(eyre::Report),
}

#[derive(Debug, thiserror::Error)]
pub enum CmdCancelError {
    #[error("command {0} is not in flight")]
    NotFound(HistoryId),
}

impl HistoryJournal {
    /// Create a new command registry.
    pub fn new(
        caps: Arc<CapClient>,
        history_store: HistoryStore,
        history_db: HistoryDatabase,
        semantic_component: SemanticComponent,
        search_index: Arc<tokio::sync::RwLock<SearchIndex>>,
    ) -> Self {
        let (broadcast, _) = broadcast::channel(128);
        Self {
            caps,
            history_store,
            history_db,
            active_sessions: DashMap::new(),
            semantic_component,
            search_index,
            broadcast,
        }
    }

    /// Notify the registry that a command has been started.
    ///
    /// Returns the [`HistoryId`] identifying the in-flight command, which is later used to
    /// [`HistoryJournal::finish`] or [`HistoryJournal::cancel`] it.
    #[must_use]
    pub fn start_cmd(&self, history: History) -> HistoryId {
        let id = history.id;
        self.active_sessions.insert(
            id,
            CmdInFlightOwned {
                history: history.clone(),
            },
        );
        let _ = self.broadcast.send(CmdEvent::Started(history));
        id
    }

    /// Mark a command as finished, persisting it to the history store and database.
    ///
    /// Pass the measured command `duration`, or `None` to measure it from the command's start
    /// timestamp.
    pub async fn finish(
        &self,
        history_id: HistoryId,
        exit_code: i64,
        duration: Option<Duration>,
    ) -> Result<(), CmdFinishError> {
        let Some((_id, mut session)) = self.active_sessions.remove(&history_id) else {
            return Err(CmdFinishError::NotFound(history_id));
        };

        let duration = duration.unwrap_or_else(|| {
            OffsetDateTime::now_utc().saturating_duration_since(session.history.timestamp)
        });

        session.history.exit = exit_code;
        session.history.duration = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);

        let history = session.history;

        self.history_db
            .save(&history)
            .await
            .map_err(|e| CmdFinishError::HistoryDbFailed(e.into()))?;
        // TODO(markovejnovic): surface the returned (RecordId, RecordIdx) so end_history can report
        // the real record id and idx rather than the placeholder history id.
        self.history_store
            .push(history.clone())
            .await
            .map_err(CmdFinishError::HistoryStoreFailed)?;

        // TODO(markovejnovic): This is a little bit hacked-together. I'm thinking it would be good
        // to have a Packer type for this kind of logic. It can wraps the Caps.
        if let Err(e) = packfile::try_pack(
            &self.history_store.store,
            &RecordSeriesKey::new(self.history_store.host_id, RecordTag::History),
            self.caps.get_server::<PackfileCap>().await.ok().flatten(),
        )
        .await
        {
            tracing::warn!("packing failed: {e}");
        }

        self.search_index.read().await.add_history(&history);
        self.semantic_component.record_history(history.clone()).await;

        let _ = self.broadcast.send(CmdEvent::Finished(history));

        Ok(())
    }

    /// Cancel a command, discarding its in-memory state without persisting a history entry.
    pub fn cancel(&self, history_id: HistoryId) -> Result<(), CmdCancelError> {
        let Some((_id, session)) = self.active_sessions.remove(&history_id) else {
            return Err(CmdCancelError::NotFound(history_id));
        };

        let _ = self.broadcast.send(CmdEvent::Cancelled(session.history));

        Ok(())
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
