pub mod model;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::history::{History, HistoryId};
use futures::StreamExt;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::command_journal::{CmdEvent, CommandJournal};
use crate::history::history_server::History as GrpcService;
use crate::history::{
    AuthorKind, CancelHistoryReply, CancelHistoryRequest, EndHistoryReply, EndHistoryRequest,
    HistoryEntry, HistoryEventKind, ShutdownReply, ShutdownRequest, StartHistoryReply,
    StartHistoryRequest, StatusReply, StatusRequest, TailHistoryReply, TailHistoryRequest,
};

const DAEMON_PROTOCOL_VERSION: u32 = 1;

/// The History gRPC service.
///
/// This is a thin adapter over [`CommandJournal`]: it translates gRPC requests into journal calls
/// and journal state/events back into gRPC replies. All command-lifecycle logic lives in the
/// journal.
#[derive(Clone)]
pub struct HistoryService {
    journal: Arc<CommandJournal>,
}

impl HistoryService {
    pub fn new(journal: Arc<CommandJournal>) -> Self {
        Self { journal }
    }
}

/// Build a [`TailHistoryReply`] from a lifecycle event and its history entry.
fn history_to_tail_reply(kind: HistoryEventKind, history: History) -> TailHistoryReply {
    TailHistoryReply {
        kind: kind as i32,
        history: Some(HistoryEntry {
            timestamp: history.timestamp.unix_timestamp_nanos() as u64,
            id: Some(history.id.into()),
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

    #[instrument(skip_all, level = Level::TRACE)]
    async fn start_history(
        &self,
        request: Request<StartHistoryRequest>,
    ) -> Result<Response<StartHistoryReply>, Status> {
        let history: History = request.into_inner().try_into()?;

        let id = self.journal.start_cmd(history).await;

        Ok(Response::new(StartHistoryReply {
            id: Some(id.history_id().into()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn end_history(
        &self,
        req: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
        let (id, exit, duration): (HistoryId, i64, Option<Duration>) =
            req.into_inner().try_into()?;

        self.journal.finish(id.into(), exit, duration).await?;

        Ok(Response::new(EndHistoryReply {
            // TODO(markovejnovic): return the record store's real record id and idx once
            // CommandJournal::finish surfaces them.
            id: Some(id.into()),
            idx: 0,
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn cancel_history(
        &self,
        request: Request<CancelHistoryRequest>,
    ) -> Result<Response<CancelHistoryReply>, Status> {
        let id: HistoryId = request.into_inner().try_into()?;

        self.journal.cancel(id.into()).await?;

        Ok(Response::new(CancelHistoryReply {
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn tail_history(
        &self,
        _request: Request<TailHistoryRequest>,
    ) -> Result<Response<Self::TailHistoryStream>, Status> {
        let mut events = self.journal.subscribe();
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

    #[instrument(skip_all, level = Level::TRACE)]
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

    #[instrument(skip_all, level = Level::TRACE)]
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownReply>, Status> {
        // TODO(markovejnovic): wire a real shutdown signal through to the daemon. HistoryService
        // currently only holds the CommandJournal, which has no way to request daemon shutdown.
        unimplemented!("daemon shutdown via the History service is not wired up yet")
    }
}
