pub mod pb;

use std::pin::Pin;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use crate::grpc::ai_session::pb::ai_session_server::AiSession as GrpcService;
use crate::grpc::ai_session::pb::{
    GetSessionEvent, GetSessionRequest, GetTranscriptChunk, GetTranscriptRequest,
    ListSessionsRequest, ListSessionsResponse, SearchSessionsMatch, SearchSessionsRequest,
    TailSessionsEvent, TailSessionsRequest,
};
// Brought into scope so the generated `ai.session` code can resolve its
// cross-package references to the `common` package (e.g. `common.Uuid`,
// `common.Lagged`).
use crate::grpc::common::pb as common;

#[derive(Clone, Default)]
pub struct Service;

impl Service {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[tonic::async_trait]
impl GrpcService for Service {
    type GetSessionStream = Pin<Box<dyn Stream<Item = Result<GetSessionEvent, Status>> + Send>>;
    type GetTranscriptStream =
        Pin<Box<dyn Stream<Item = Result<GetTranscriptChunk, Status>> + Send>>;
    type SearchSessionsStream =
        Pin<Box<dyn Stream<Item = Result<SearchSessionsMatch, Status>> + Send>>;
    type TailSessionsStream = Pin<Box<dyn Stream<Item = Result<TailSessionsEvent, Status>> + Send>>;

    async fn list_sessions(
        &self,
        _request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        Err(Status::unimplemented(""))
    }

    async fn get_session(
        &self,
        _request: Request<GetSessionRequest>,
    ) -> Result<Response<Self::GetSessionStream>, Status> {
        Err(Status::unimplemented(""))
    }

    async fn get_transcript(
        &self,
        _request: Request<GetTranscriptRequest>,
    ) -> Result<Response<Self::GetTranscriptStream>, Status> {
        Err(Status::unimplemented(""))
    }

    async fn search_sessions(
        &self,
        _request: Request<SearchSessionsRequest>,
    ) -> Result<Response<Self::SearchSessionsStream>, Status> {
        Err(Status::unimplemented(""))
    }

    async fn tail_sessions(
        &self,
        _request: Request<TailSessionsRequest>,
    ) -> Result<Response<Self::TailSessionsStream>, Status> {
        Err(Status::unimplemented(""))
    }
}
