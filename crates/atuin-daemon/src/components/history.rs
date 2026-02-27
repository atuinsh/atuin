//! History component.
//!
//! Handles command history lifecycle (start/end) and provides the History gRPC service.

use std::sync::Arc;

use atuin_client::{
    database::Database,
    history::{History, HistoryId, store::HistoryStore},
    settings::Settings,
};
use dashmap::DashMap;
use eyre::Result;
use time::OffsetDateTime;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
    history::{
        EndHistoryReply, EndHistoryRequest, ShutdownReply, ShutdownRequest, StartHistoryReply,
        StartHistoryRequest, StatusReply, StatusRequest,
        history_server::{History as HistorySvc, HistoryServer},
    },
};

const DAEMON_PROTOCOL_VERSION: u32 = 1;

/// History component - manages command history lifecycle.
///
/// This component:
/// - Tracks currently running commands (stored in memory)
/// - Saves completed commands to the database and record store
/// - Emits history events for other components (e.g., search indexing)
/// - Provides the History gRPC service
pub struct HistoryComponent {
    inner: Arc<HistoryComponentInner>,
}

struct HistoryComponentInner {
    /// Commands currently running (not yet completed).
    running: DashMap<HistoryId, History>,

    /// Handle to the daemon (set during start).
    handle: tokio::sync::RwLock<Option<DaemonHandle>>,

    /// History store for pushing records (set during start).
    history_store: tokio::sync::RwLock<Option<HistoryStore>>,
}

impl HistoryComponent {
    /// Create a new history component.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(HistoryComponentInner {
                running: DashMap::new(),
                handle: tokio::sync::RwLock::new(None),
                history_store: tokio::sync::RwLock::new(None),
            }),
        }
    }

    /// Get the gRPC service for this component.
    ///
    /// This returns a tonic service that can be added to a gRPC server.
    pub fn grpc_service(&self) -> HistoryServer<HistoryGrpcService> {
        HistoryServer::new(HistoryGrpcService {
            inner: self.inner.clone(),
        })
    }
}

impl Default for HistoryComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl Component for HistoryComponent {
    fn name(&self) -> &'static str {
        "history"
    }

    async fn start(&mut self, handle: DaemonHandle) -> Result<()> {
        // Create the history store
        let host_id = Settings::host_id().await?;
        let history_store =
            HistoryStore::new(handle.store().clone(), host_id, *handle.encryption_key());

        *self.inner.history_store.write().await = Some(history_store);
        *self.inner.handle.write().await = Some(handle);

        tracing::info!("history component started");
        Ok(())
    }

    async fn handle_event(&mut self, _event: &DaemonEvent) -> Result<()> {
        // History component produces events but doesn't need to react to them
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        tracing::info!("history component stopped");
        Ok(())
    }
}

/// The gRPC service implementation.
///
/// This is a thin wrapper that delegates to the component's shared state.
pub struct HistoryGrpcService {
    inner: Arc<HistoryComponentInner>,
}

#[tonic::async_trait]
impl HistorySvc for HistoryGrpcService {
    #[instrument(skip_all, level = Level::INFO)]
    async fn start_history(
        &self,
        request: Request<StartHistoryRequest>,
    ) -> Result<Response<StartHistoryReply>, Status> {
        let req = request.into_inner();

        let timestamp =
            OffsetDateTime::from_unix_timestamp_nanos(req.timestamp as i128).map_err(|_| {
                Status::invalid_argument(
                    "failed to parse timestamp as unix time (expected nanos since epoch)",
                )
            })?;

        let h: History = History::daemon()
            .timestamp(timestamp)
            .command(req.command)
            .cwd(req.cwd)
            .session(req.session)
            .hostname(req.hostname)
            .build()
            .into();

        // Emit the event
        if let Some(handle) = self.inner.handle.read().await.as_ref() {
            handle.emit(DaemonEvent::HistoryStarted(h.clone()));
        }

        let id = h.id.clone();
        tracing::info!(id = id.to_string(), "start history");
        self.inner.running.insert(id.clone(), h);

        let reply = StartHistoryReply {
            id: id.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        };

        Ok(Response::new(reply))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn end_history(
        &self,
        request: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
        let req = request.into_inner();
        let id = HistoryId(req.id);

        if let Some((_, mut history)) = self.inner.running.remove(&id) {
            history.exit = req.exit;
            history.duration = match req.duration {
                0 => i64::try_from(
                    (OffsetDateTime::now_utc() - history.timestamp).whole_nanoseconds(),
                )
                .expect("failed to convert calculated duration to i64"),
                value => i64::try_from(value).expect("failed to get i64 duration"),
            };

            // Get the handle and store to save the history
            let handle_guard = self.inner.handle.read().await;
            let handle = handle_guard
                .as_ref()
                .ok_or_else(|| Status::internal("component not initialized"))?;

            let store_guard = self.inner.history_store.read().await;
            let history_store = store_guard
                .as_ref()
                .ok_or_else(|| Status::internal("component not initialized"))?;

            // Save to database
            handle
                .history_db()
                .save(&history)
                .await
                .map_err(|e| Status::internal(format!("failed to write to db: {e:?}")))?;

            tracing::info!(
                id = id.0.to_string(),
                duration = history.duration,
                "end history"
            );

            // Push to record store
            let (record_id, idx) = history_store
                .push(history.clone())
                .await
                .map_err(|e| Status::internal(format!("failed to push record to store: {e:?}")))?;

            // Emit the event
            handle.emit(DaemonEvent::HistoryEnded(history));

            let reply = EndHistoryReply {
                id: record_id.0.to_string(),
                idx,
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol: DAEMON_PROTOCOL_VERSION,
            };

            return Ok(Response::new(reply));
        }

        Err(Status::not_found(format!(
            "could not find history with id: {id}"
        )))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusReply>, Status> {
        let reply = StatusReply {
            healthy: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            protocol: DAEMON_PROTOCOL_VERSION,
        };

        Ok(Response::new(reply))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownReply>, Status> {
        // Use the daemon handle to request shutdown
        if let Some(handle) = self.inner.handle.read().await.as_ref() {
            handle.shutdown();
        }
        Ok(Response::new(ShutdownReply { accepted: true }))
    }
}
