//! Exposes logic for managing the lifecycle of commands.
//!
//! The core structure is [`CmdRegistry`]. The [`CmdRegistry`] handles the creation and termination
//! of new commands.
//!
//! When users run new commands in the client, the client sends requests to the GPRC server. The
//! `HistoryService` then "forwards" the request down into [`CmdRegistry`] -- requesting the
//! start of a new shell command.
//!
//! The [`CmdRegistry`] is responsible for managing the lifecycle of this command. The main
//! entrypoint is [`CmdRegistry::start_cmd`] which marks the beginning of a command. This returns a
//! new object [`CmdInFlightId`] which represents a command which has been started, but not finished.
//!
//! ## Commands in flight
//!
//! Commands-in-flight are commands which ahve been started but have just started running. These
//! commands are uniquely identified by a [`CmdInFlightId`].
//!
//! Commands in flight can be terminated in one of two ways:
//!
//!   - [`CmdRegistry::finish`] marks the command as finished, which will create and store a new
//!     history entry.
//!   - [`CmdRegistry::cancel`] cancels the command, disposing of any in-memory resources, but
//!     without the logic of persisting the history entry.
//!
//! ## Streaming
//!
//! It is possible to stream events out of [`CmdRegistry`] via [`CmdRegistry::subscribe`] which
//! returns a new [`futures::Stream`] of [`CmdEvent`] events.
//!
//! ```mermaid
//! sequenceDiagram
//!   actor C as Client
//!
//!   box Daemon Process
//!     participant D as GRPC Server
//!     participant R as CmdRegistry
//!   end
//!   participant DB@{ "type" : "database" }
//!   participant P as PTY Proxy
//!
//!   note over C,P: Finishing the command means notifying the<br/>daemon that a command has been completed.
//!   C->>+D: Finish Command
//!   D-->>+R: finish_cmd
//!   note over R,P: Get the command output from the pty proxy.
//!   R->>+P: Request(CommandOutput)
//!   P->>-R:
//!   note over R,DB: Storing data into the database<br/>involves multiple database stores:<br/>one for RecordStore, HistoryDb,<br/>and CommandDb.
//!   R->>+DB: Store Command Output
//!   DB->>-R:
//!   R->>+DB: Store History Entry
//!   DB->>-R:
//!   R->>+DB: Store Record
//!   DB->>-R:
//!
//!   R-->>-D:
//!
//!   D->>-C:
//! ```

use std::sync::Arc;

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

/// Uniquely identifies a command that has been started but not yet terminated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CmdInFlightId {
    history_id: HistoryId,
}

impl From<HistoryId> for CmdInFlightId {
    fn from(value: HistoryId) -> Self {
        CmdInFlightId { history_id: value }
    }
}

impl std::fmt::Display for CmdInFlightId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.history_id)
    }
}

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
pub struct CmdRegistry {
    caps: Arc<CapClient>,
    history_store: HistoryStore,
    history_db: HistoryDatabase,

    active_sessions: DashMap<CmdInFlightId, CmdInFlightOwned>,

    semantic_component: SemanticComponent,
    search_index: Arc<tokio::sync::RwLock<SearchIndex>>,

    broadcast: broadcast::Sender<CmdEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum CmdFinishError {
    #[error("command {0} is not in flight")]
    NotFound(CmdInFlightId),
    #[error("storing into history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
    #[error("storing into history db failed: {0}")]
    HistoryDbFailed(eyre::Report),
}

#[derive(Debug, thiserror::Error)]
pub enum CmdCancelError {
    #[error("command {0} is not in flight")]
    NotFound(CmdInFlightId),
}

impl CmdRegistry {
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
    /// Returns the [`CmdInFlightId`] identifying the in-flight command, which is later used to
    /// [`CmdRegistry::finish`] or [`CmdRegistry::cancel`] it.
    pub async fn start_cmd(&self, history: History) -> CmdInFlightId {
        let id = CmdInFlightId::from(history.id.clone());
        self.active_sessions.insert(id.clone(), CmdInFlightOwned {
            history: history.clone(),
        });
        let _ = self.broadcast.send(CmdEvent::Started(history));
        id
    }

    /// Mark a command as finished, persisting it to the history store and database.
    ///
    /// `duration` is in nanoseconds; a value of `0` means "compute it from the command's start
    /// timestamp".
    pub async fn finish(
        &self,
        session_id: CmdInFlightId,
        exit_code: i64,
        duration: i64,
    ) -> Result<(), CmdFinishError> {
        let (_sess_id, mut session) = match self.active_sessions.remove(&session_id) {
            Some(s) => s,
            None => return Err(CmdFinishError::NotFound(session_id)),
        };

        session.history.exit = exit_code;
        session.history.duration = if duration == 0 {
            i64::try_from(
                OffsetDateTime::now_utc()
                    .saturating_duration_since(session.history.timestamp)
                    .as_nanos(),
            )
            .unwrap_or(i64::MAX)
        } else {
            duration
        };

        let history = session.history;

        // TODO(markovejnovic): The following DB operations can be parallelized.
        // They're on different DBs.
        self.history_db
            .save(&history)
            .await
            .map_err(|e| CmdFinishError::HistoryDbFailed(e.into()))?;
        // TODO(markovejnovic): surface the returned (RecordId, RecordIdx) so end_history can report
        // the real record id and idx rather than the placeholder history id.
        self.history_store
            .push(history.clone())
            .await
            .map_err(|e| CmdFinishError::HistoryStoreFailed(e.into()))?;

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
    pub async fn cancel(&self, session_id: CmdInFlightId) -> Result<(), CmdCancelError> {
        let (_sess_id, session) = match self.active_sessions.remove(&session_id) {
            Some(s) => s,
            None => return Err(CmdCancelError::NotFound(session_id)),
        };

        let _ = self.broadcast.send(CmdEvent::Cancelled(session.history));

        Ok(())
    }

    /// Create a new stream of [`CmdEvent`] objects.
    pub fn subscribe(&self) -> BroadcastStream<CmdEvent> {
        BroadcastStream::new(self.broadcast.subscribe())
    }
}
