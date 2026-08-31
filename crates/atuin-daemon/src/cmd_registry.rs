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
//! new object [`CmdInFlight`] which represents a command which has been started, but not finished.
//!
//! ## [`CmdInFlight`]
//!
//! [`CmdInFlight`]s can be terminated in one of two ways:
//!
//!   - [`CmdInFlight::finish`] marks the command as finished, which will create and store a new
//!     history entry.
//!   - [`CmdInFlight::cancel`] cancels the command, disposing of any in-memory resources, but
//!     without the logic of persisting the history entry.
//!
//! The user of this library is responsible for keeping [`CmdInFlight`] alive. [`Drop`] of
//! [`CmdInFlight`] is equivalent to calling [`CmdInFlight::cancel`].
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
use std::time::Instant;

use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{History, HistoryId};
use atuin_client::packfile;
use atuin_domain::caps::{CapClient, CapServer, PackfileCap};
use atuin_domain::record::{RecordSeriesKey, RecordTag};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::SemanticComponent;
use crate::search::SearchIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CmdInFlightId {
    history_id: HistoryId,
}

/// Represents an active session for a command.
#[derive(Debug, Clone, Copy)]
pub struct CmdInFlight<'eng> {
    id: CmdInFlightId,
    engine: &'eng CmdRegistry,
}

impl<'eng> CmdInFlight<'eng> {
    /// Consume the command session, marking the command as complete and storing it into the
    /// long-term storage.
    ///
    /// Failing to call this will result in the command data never being persisted into storage. It
    /// is not considered terminal.
    pub async fn finish(
        self,
        timestamp: Instant,
        exit_code: i64,
    ) -> Result<(), CmdFinishError<'eng>> {
        self.engine.finish_cmd(self, timestamp, exit_code).await
    }

    pub async fn cancel(self) -> Result<(), CmdCancelError<'eng>> {
        self.engine.cancel_cmd(self).await
    }
}

#[derive(Debug)]
struct CmdInFlightOwned {
    /// TODO(markovejnovic): Why do we need this?
    history: History,
}

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

#[derive(Debug, Clone, thiserror::Error)]
pub enum CmdFinishError<'eng> {
    #[error("command {0} is not in flight")]
    NotFound(CmdInFlight<'eng>),
    #[error("storing into history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
    #[error("storing into history db failed: {0}")]
    HistoryDbFailed(sqlx::Error),
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum CmdCancelError<'eng> {
    #[error("command {0} is not in flight")]
    NotFound(CmdInFlight<'eng>),
}

impl CmdRegistry {
    /// Create a new output capture engine.
    pub fn new(history_store: HistoryStore) -> Self {
        Self {
            history_store,
            active_sessions: DashMap::new(),
        }
    }

    /// Notify the output capture engine that a command has been started.
    ///
    /// It is intended that this be called by a client.
    ///
    /// TODO(markovejnovic): Docs suck.
    pub async fn start_cmd(&self, history: History) -> CmdInFlight {}

    async fn finish_cmd<'s>(
        &self,
        session: CmdInFlight<'s>,
        timestamp: Instant,
        exit_code: i64,
    ) -> Result<(), CmdFinishError<'s>> {
        let (_sess_id, session) = match self.active_sessions.remove(&session.id) {
            Some(s) => s,
            None => return Err(CmdFinishError::NotFound(session)),
        };

        session.history.exit = exit_code;
        session.history.duration = timestamp - session.history.timestamp;

        // TODO(markovejnovic): The following DB operations can be parallelized.
        // They're on different DBs.
        self.history_store
            .push(session.history)
            .await
            .map_err(CmdFinishError::HistoryStoreFailed)?;
        self.history_db.save(&session.history).await.map_err(CmdFinishError::HistoryDbFailed)?;

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

        self.search_index.read().await.add_history(&session.history);
        self.semantic_component.record_history(session.history).await;

        self.broadcast.send(CmdEvent::Finished(session.history));

        Ok(())
    }

    /// Create a new stream of [`CmdEvent`] objects.
    pub fn subscribe(&self) -> BroadcastStream<CmdEvent> {
        BroadcastStream::new(self.broadcast.subscribe())
    }

    async fn cancel_cmd<'s>(&self, session: CmdInFlight<'s>) -> Result<(), CmdCancelError<'s>> {
        let (_sess_id, session) = match self.active_sessions.remove(&session.id) {
            Some(s) => s,
            None => return Err(CmdCancelError::NotFound(session)),
        };

        self.broadcast.send(CmdEvent::Cancelled(session.history));

        Ok(())
    }
}
