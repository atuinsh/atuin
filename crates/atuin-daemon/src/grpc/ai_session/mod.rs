pub mod pb;

use std::pin::Pin;

use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use crate::grpc::ai_session::pb::ai_session_server::AiSession as GrpcService;
use crate::grpc::ai_session::pb::{
    GetSessionRequest, GetSessionResponse, GetTranscriptRequest, GetTranscriptResponse,
    ListSessionsRequest, ListSessionsResponse, SearchSessionsMatch, SearchSessionsRequest,
    TailSessionsEvent, TailSessionsRequest,
};

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
    ) -> Result<Response<GetSessionResponse>, Status> {
        Err(Status::unimplemented(""))
    }

    async fn get_transcript(
        &self,
        _request: Request<GetTranscriptRequest>,
    ) -> Result<Response<GetTranscriptResponse>, Status> {
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
