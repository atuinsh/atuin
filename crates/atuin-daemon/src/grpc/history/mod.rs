pub mod model;

use atuin_client::history::History;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::cmd_registry::CmdRegistry;
use crate::history::history_server::History as GrpcService;
use crate::history::{
    CancelHistoryReply, CancelHistoryRequest, EndHistoryReply, EndHistoryRequest, ShutdownReply,
    ShutdownRequest, StartHistoryReply, StartHistoryRequest, StatusReply, StatusRequest,
    TailHistoryReply, TailHistoryRequest,
};

pub struct HistoryService {
    cmd_registry: &'static CmdRegistry,
}

impl HistoryService {
    pub fn new(cmd_registry: &'static CmdRegistry) -> Self {
        Self { cmd_registry }
    }
}

impl GrpcService for HistoryService {
    #[instrument(skip_all, level = Level::TRACE)]
    async fn start_history(
        &self,
        request: Request<StartHistoryRequest>,
    ) -> Result<Response<StartHistoryReply>, Status> {
        let history: History = request.into_inner().into()?;
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn end_history(
        &self,
        request: Request<EndHistoryRequest>,
    ) -> Result<Response<EndHistoryReply>, Status> {
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn cancel_history(
        &self,
        request: Request<CancelHistoryRequest>,
    ) -> Result<Response<CancelHistoryReply>, Status> {
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn shutdown(
        &self,
        request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownReply>, Status> {
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<StatusReply>, Status> {
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn tail_history(
        &self,
        request: Request<TailHistoryRequest>,
    ) -> Result<Response<TailHistoryReply>, Status> {
    }
}
