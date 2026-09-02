pub mod pb;

use std::pin::Pin;
use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use atuin_common::time::OffsetDateTimeExt;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::DaemonHandle;
use crate::grpc::history::pb::history_server::History as GrpcService;
use crate::grpc::history::pb::{
    CancelHistoryReply, CancelHistoryRequest, EndHistoryReply, EndHistoryRequest, Lagged,
    ShutdownReply, ShutdownRequest, StartHistoryReply, StartHistoryRequest, StatusReply,
    StatusRequest, TailHistoryEvent, TailHistoryReply, TailHistoryRequest,
};
use crate::history_journal::HistoryJournal;

const DAEMON_PROTOCOL_VERSION: u32 = 2;

/// The History gRPC service.
///
/// Clients request operations on history via this service.
#[derive(Clone)]
pub struct Service {
    journal: Arc<HistoryJournal>,
    /// TODO(markovejnovic): Revisit whether we need to hold this handle. At the moment, the only
    /// reason why this exists is to be able to service the [`GrpcService::shutdown`] request, but
    /// perhaps that function does not belong in the history service -- perhaps we should have a
    /// Control service.
    daemon_handle: DaemonHandle,
}

impl Service {
    #[must_use]
    pub fn new(journal: Arc<HistoryJournal>, daemon_handle: DaemonHandle) -> Self {
        Self {
            journal,
            daemon_handle,
        }
    }
}

#[tonic::async_trait]
impl GrpcService for Service {
    type TailHistoryStream = Pin<Box<dyn Stream<Item = Result<TailHistoryReply, Status>> + Send>>;

    #[instrument(skip_all, level = Level::TRACE)]
    async fn start_history(
        &self,
        request: Request<StartHistoryRequest>,
    ) -> Result<Response<StartHistoryReply>, Status> {
        let history: History = request.into_inner().try_into()?;

        let id = self.journal.start_cmd(history);

        Ok(Response::new(StartHistoryReply {
            id: Some(id.into()),
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn end_history(
        &self,
        req: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
        let req = req.into_inner().view()?;

        // The client may omit the duration, in which case we measure it from the command's start
        // timestamp, which the journal tracks for us.
        let duration = match req.duration {
            Some(duration) => duration,
            None => OffsetDateTime::now_utc()
                .saturating_duration_since(self.journal.get(req.history_id)?.timestamp),
        };

        let finished_cmd = self.journal.finish(req.history_id, req.exit_code, duration).await?;

        Ok(Response::new(EndHistoryReply {
            record_id: Some(finished_cmd.history_record_id.into()),
            record_idx: finished_cmd.history_record_idx,
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
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

        self.journal.cancel(id)?;

        Ok(Response::new(CancelHistoryReply {
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn tail_history(
        &self,
        _request: Request<TailHistoryRequest>,
    ) -> Result<Response<Self::TailHistoryStream>, Status> {
        // Every journal event (started, ended, cancelled) and any lag notice becomes a reply on the
        // tail stream.
        let stream = self.journal.subscribe().map(|event| {
            Ok::<_, Status>(TailHistoryReply {
                event: Some(match event {
                    Ok(event) => event.into(),
                    Err(BroadcastStreamRecvError::Lagged(dropped)) => {
                        TailHistoryEvent::Lagged(Lagged { dropped })
                    }
                }),
            })
        });

        Ok(Response::new(Box::pin(stream)))
    }

    /// Returns the active status of the daemon. Has nothing to do with history.
    ///
    /// TODO(markovejnovic): This probably doesn't belong in this service.
    #[instrument(skip_all, level = Level::TRACE)]
    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusReply>, Status> {
        Ok(Response::new(StatusReply {
            healthy: true,
            // TODO(markovejnovic): Pull this from one constant, well-defined spot.
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            protocol: DAEMON_PROTOCOL_VERSION,
        }))
    }

    /// Requests the daemon shut down. Has nothing to do with history.
    ///
    /// Note:
    ///  - A misbehaving daemon will likely not respect this request.
    ///  - The shutdown request is sent asynchronously, but this RPC immediately returns.
    ///
    /// TODO(markovejnovic): This probably doesn't belong in this service.
    #[instrument(skip_all, level = Level::TRACE)]
    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownReply>, Status> {
        self.daemon_handle.shutdown();
        Ok(Response::new(ShutdownReply { accepted: true }))
    }
}
