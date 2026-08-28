//! Performs command output capture and storage.
//!
//! TODO(markovejnovic): Document why this keeps using the terminology "History".

use std::sync::Arc;
use std::time::Instant;

use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::history::{History, HistoryId, store::HistoryStore};
use atuin_client::packfile;
use atuin_domain::caps::{CapClient, CapServer, PackfileCap};
use atuin_domain::record::{RecordSeriesKey, RecordTag};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::SemanticComponent;
use crate::pty_proxy::PtyProxyPool;
use crate::search::SearchIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CmdSessionId {
    history_id: HistoryId,
}

/// Represents an active session for a command.
#[derive(Debug, Clone, Copy)]
pub struct CmdSession<'p, 'eng> {
    id: CmdSessionId,
    engine: &'eng OcapEngine<'p>,
}

impl CmdSession {
    /// Consume the command session, marking the command as complete and storing it into the
    /// long-term storage.
    ///
    /// Failing to call this will result in the command data never being persisted into storage. It
    /// is not considered terminal.
    pub fn finish(self, timestamp: Instant) {}

    pub fn cancel(self);
}

#[derive(Debug)]
struct CmdSessionOwned {
    /// TODO(markovejnovic): Why do we need this?
    history: History,
}

#[derive(Debug, Clone)]
pub enum CmdEvent {
    Started(History),
    Finished(History),
    Cancelled(History),
}

/// Engine which performs output capture, management, storage and retrieval.
#[derive(Debug)]
struct OcapEngine<'p> {
    pty_proxy_pool: &'p PtyProxyPool,
    caps: Arc<CapClient>,
    history_store: HistoryStore,
    history_db: HistoryDatabase,

    active_sessions: DashMap<CmdSessionId, CmdSessionOwned>,

    semantic_component: SemanticComponent,
    search_index: Arc<tokio::sync::RwLock<SearchIndex>>,

    broadcast: broadcast::Sender<CmdEvent>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CmdFinishError<'p, 'eng> {
    #[error("command {0} is not in flight")]
    NotFound(CmdSession<'p, 'eng>),
    #[error("storing into history store failed: {0}")]
    HistoryStoreFailed(eyre::Report),
    #[error("storing into history db failed: {0}")]
    HistoryDbFailed(sqlx::Error),
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum CmdCancelError<'p, 'eng> {
    #[error("command {0} is not in flight")]
    NotFound(CmdSession<'p, 'eng>),
}

impl<'p> OcapEngine<'p> {
    /// Create a new output capture engine.
    pub fn new(pty_proxy_pool: &'p PtyProxyPool, history_store: HistoryStore) -> Self {
        Self {
            pty_proxy_pool,
            history_store,
            active_sessions: DashMap::new(),
        }
    }

    /// Notify the output capture engine that a command has been started.
    ///
    /// It is intended that this be called by a client.
    ///
    /// TODO(markovejnovic): Docs suck.
    pub async fn start_cmd(&self, history: History) -> CmdSession {}

    async fn finish_cmd<'s>(
        &self,
        session: CmdSession<'s, 'p>,
        timestamp: Instant,
        exit_code: i64,
    ) -> Result<(), CmdFinishError<'p, 's>> {
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
    pub fn stream(&self) -> BroadcastStream<CmdEvent> {
        BroadcastStream::new(self.broadcast.subscribe())
    }

    async fn cancel_cmd<'s>(
        &self,
        session: CmdSession<'s, 'p>,
        timestamp: Instant,
    ) -> Result<(), CmdCancelError<'p, 's>> {
        let (_sess_id, session) = match self.active_sessions.remove(&session.id) {
            Some(s) => s,
            None => return Err(CmdCancelError::NotFound(session)),
        };

        self.broadcast.send(CmdEvent::Cancelled(session.history));

        Ok(())
    }
}
