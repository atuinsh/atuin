pub mod model;

use std::pin::Pin;
use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use futures::StreamExt;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::cmd_registry::{CmdCancelError, CmdEvent, CmdFinishError, CmdRegistry};
use crate::history::history_server::History as GrpcService;
use crate::history::{
    AuthorKind, CancelHistoryReply, CancelHistoryRequest, EndHistoryReply, EndHistoryRequest,
    HistoryEntry, HistoryEventKind, ShutdownReply, ShutdownRequest, StartHistoryReply,
    StartHistoryRequest, StatusReply, StatusRequest, TailHistoryReply, TailHistoryRequest,
};

const DAEMON_PROTOCOL_VERSION: u32 = 1;

/// The History gRPC service.
///
/// This is a thin adapter over [`CmdRegistry`]: it translates gRPC requests into registry calls
/// and registry state/events back into gRPC replies. All command-lifecycle logic lives in the
/// registry.
#[derive(Clone)]
pub struct HistoryService {
    cmd_registry: Arc<CmdRegistry>,
}

impl HistoryService {
    pub fn new(cmd_registry: Arc<CmdRegistry>) -> Self {
        Self { cmd_registry }
    }
}

/// Build a [`TailHistoryReply`] from a lifecycle event and its history entry.
fn history_to_tail_reply(kind: HistoryEventKind, history: History) -> TailHistoryReply {
    TailHistoryReply {
        kind: kind as i32,
        history: Some(HistoryEntry {
            timestamp: history.timestamp.unix_timestamp_nanos() as u64,
            id: history.id.to_string(),
            command: history.command,
            cwd: history.cwd,
            session: history.session,
            hostname: history.cmd_origin.into_string(),
            author: history.author,
            intent: history.intent.unwrap_or_default(),
            exit: history.exit,
            duration: history.duration,
            shell: history.shell.unwrap_or_default(),
            author_kind: AuthorKind::from(history.author_kind) as i32,
        }),
    }
}

#[tonic::async_trait]
impl GrpcService for HistoryService {
    type TailHistoryStream = Pin<Box<dyn Stream<Item = Result<TailHistoryReply, Status>> + Send>>;

    #[instrument(skip_all, level = Level::INFO)]
    async fn start_history(
        &self,
        request: Request<StartHistoryRequest>,
    ) -> Result<Response<StartHistoryReply>, Status> {
        let history: History = request.into_inner().try_into()?;

        let id = self.cmd_registry.start_cmd(history).await;
        tracing::info!(id = %id, "start history");

        Ok(Response::new(StartHistoryReply {
            id: id.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn end_history(
        &self,
        request: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
        let req = request.into_inner();
        let id: HistoryId = req
            .id
            .parse()
            .map_err(|_| Status::invalid_argument(format!("invalid history id: {}", req.id)))?;

        let duration = i64::try_from(req.duration).unwrap_or(i64::MAX);
        self.cmd_registry.finish(id.clone().into(), req.exit, duration).await.map_err(
            |e| match e {
                CmdFinishError::NotFound(_) => {
                    Status::not_found(format!("could not find history with id: {id}"))
                }
                other => Status::internal(other.to_string()),
            },
        )?;

        tracing::info!(id = %id, "end history");

        Ok(Response::new(EndHistoryReply {
            // TODO(markovejnovic): return the record store's real record id and idx once
            // CmdRegistry::finish surfaces them.
            id: id.to_string(),
            idx: 0,
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn cancel_history(
        &self,
        request: Request<CancelHistoryRequest>,
    ) -> Result<Response<CancelHistoryReply>, Status> {
        let req = request.into_inner();
        let id: HistoryId = req
            .id
            .parse()
            .map_err(|_| Status::invalid_argument(format!("invalid history id: {}", req.id)))?;

        self.cmd_registry.cancel(id.clone().into()).await.map_err(|e| match e {
            CmdCancelError::NotFound(_) => {
                Status::not_found(format!("could not find history with id: {id}"))
            }
        })?;

        Ok(Response::new(CancelHistoryReply {
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn tail_history(
        &self,
        _request: Request<TailHistoryRequest>,
    ) -> Result<Response<Self::TailHistoryStream>, Status> {
        let mut events = self.cmd_registry.subscribe();
        let (tx, out_rx) = tokio::sync::mpsc::channel::<Result<TailHistoryReply, Status>>(128);

        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let reply = match event {
                    Ok(CmdEvent::Started(history)) => {
                        Some(history_to_tail_reply(HistoryEventKind::Started, history))
                    }
                    Ok(CmdEvent::Finished(history)) => {
                        Some(history_to_tail_reply(HistoryEventKind::Ended, history))
                    }
                    Ok(CmdEvent::Cancelled(_)) => None,
                    Err(BroadcastStreamRecvError::Lagged(skipped)) => {
                        let _ = tx
                            .send(Err(Status::resource_exhausted(format!(
                                "tail stream lagged behind and dropped {skipped} events"
                            ))))
                            .await;
                        break;
                    }
                };

                if let Some(reply) = reply
                    && tx.send(Ok(reply)).await.is_err()
                {
                    break;
                }
            }
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(out_rx);
        Ok(Response::new(Box::pin(stream)))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusReply>, Status> {
        Ok(Response::new(StatusReply {
            healthy: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownReply>, Status> {
        // TODO(markovejnovic): wire a real shutdown signal through to the daemon. HistoryService
        // currently only holds the CmdRegistry, which has no way to request daemon shutdown.
        Ok(Response::new(ShutdownReply { accepted: true }))
    }
}
