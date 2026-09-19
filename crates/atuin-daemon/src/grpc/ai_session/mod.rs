pub mod pb;

use std::pin::Pin;
use std::sync::Arc;

use atuin_client::ai_session::{HarnessKind, HarnessSession, Page, PageToken};
use futures::StreamExt;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};

use crate::grpc::ai_agent::pb as agent_pb;
use crate::grpc::ai_session::pb::ai_session_server::AiSession as GrpcService;
use crate::grpc::ai_session::pb::{
    GetSessionEvent, GetSessionRequest, GetTranscriptChunk, GetTranscriptRequest,
    ListSessionsRequest, ListSessionsResponse, SearchSessionsMatch, SearchSessionsRequest,
    TailSessionsEvent, TailSessionsRequest, get_session_event, tail_sessions_event,
};
use crate::grpc::common::pb::Lagged;
use crate::session_capture::{AiHarnessSessionCapture, SessionTailEvent};

#[derive(Clone)]
pub struct Service {
    capture: Arc<AiHarnessSessionCapture>,
}

impl Service {
    #[must_use]
    pub fn new(capture: Arc<AiHarnessSessionCapture>) -> Self {
        Self { capture }
    }

    fn parse_harness(harness: Option<i32>) -> Result<Option<HarnessKind>, Status> {
        harness
            .map(|h| {
                agent_pb::HarnessKind::try_from(h)
                    .map(HarnessKind::from)
                    .map_err(|_| Status::invalid_argument("unrecognized harness kind"))
            })
            .transpose()
    }

    fn parse_session(session: Option<agent_pb::HarnessSession>) -> Result<HarnessSession, Status> {
        session
            .ok_or_else(|| Status::invalid_argument("missing session"))?
            .try_into()
            .map_err(|_| Status::invalid_argument("invalid harness session"))
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
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let request = request.into_inner();
        let harness = Self::parse_harness(request.harness)?;
        let page = Page {
            size: request.page_size,
            token: (!request.page_token.is_empty()).then(|| PageToken::from(request.page_token)),
        };

        let (sessions, token) = self
            .capture
            .list_sessions(page, harness)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ListSessionsResponse {
            sessions: sessions.into_iter().map(Into::into).collect(),
            next_page_token: token.map(Into::into).unwrap_or_default(),
        }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<Self::GetSessionStream>, Status> {
        let handle = Self::parse_session(request.into_inner().session)?;

        let session = self
            .capture
            .get_session(&handle)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("session not found"))?;

        let messages: Vec<Result<GetSessionEvent, Status>> = self
            .capture
            .messages(&handle)
            .map(|message| {
                message
                    .map(|message| GetSessionEvent {
                        event: Some(get_session_event::Event::Message(message.into())),
                    })
                    .map_err(|e| Status::internal(e.to_string()))
            })
            .collect()
            .await;

        let events = std::iter::once(Ok(GetSessionEvent {
            event: Some(get_session_event::Event::Session(session.into())),
        }))
        .chain(messages);

        Ok(Response::new(Box::pin(futures::stream::iter(events))))
    }

    async fn get_transcript(
        &self,
        request: Request<GetTranscriptRequest>,
    ) -> Result<Response<Self::GetTranscriptStream>, Status> {
        let handle = Self::parse_session(request.into_inner().session)?;

        let chunks: Vec<Result<GetTranscriptChunk, Status>> = self
            .capture
            .transcript(&handle)
            .map(|chunk| {
                chunk
                    .map(|chunk| GetTranscriptChunk { chunk })
                    .map_err(|e| Status::internal(e.to_string()))
            })
            .collect()
            .await;

        Ok(Response::new(Box::pin(futures::stream::iter(chunks))))
    }

    async fn search_sessions(
        &self,
        _request: Request<SearchSessionsRequest>,
    ) -> Result<Response<Self::SearchSessionsStream>, Status> {
        Err(Status::unimplemented("session search is not supported"))
    }

    async fn tail_sessions(
        &self,
        request: Request<TailSessionsRequest>,
    ) -> Result<Response<Self::TailSessionsStream>, Status> {
        let harness = Self::parse_harness(request.into_inner().harness)?;

        let stream = self
            .capture
            .subscribe()
            .filter(move |event| {
                let keep = match (harness, event) {
                    (_, Err(BroadcastStreamRecvError::Lagged(_))) => true,
                    (None, Ok(_)) => true,
                    (Some(harness), Ok(SessionTailEvent::SessionStarted(s))) => {
                        s.handle.harness == harness
                    }
                    (Some(harness), Ok(SessionTailEvent::SessionUpdated(s))) => {
                        s.handle.harness == harness
                    }
                    (Some(harness), Ok(SessionTailEvent::Message(m))) => {
                        m.session.harness == harness
                    }
                };
                std::future::ready(keep)
            })
            .map(|event| {
                Ok::<_, Status>(TailSessionsEvent {
                    event: Some(match event {
                        Ok(SessionTailEvent::SessionStarted(s)) => {
                            tail_sessions_event::Event::SessionStarted(s.into())
                        }
                        Ok(SessionTailEvent::SessionUpdated(s)) => {
                            tail_sessions_event::Event::SessionUpdated(s.into())
                        }
                        Ok(SessionTailEvent::Message(m)) => {
                            tail_sessions_event::Event::Message(m.into())
                        }
                        Err(BroadcastStreamRecvError::Lagged(n)) => {
                            tail_sessions_event::Event::Lagged(Lagged { dropped: n })
                        }
                    }),
                })
            });

        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[tokio::test]
    async fn tail_sessions_returns_a_stream() {
        let cap = Arc::new(AiHarnessSessionCapture::nop().await);
        let svc = Service::new(cap);

        let result = svc.tail_sessions(Request::new(TailSessionsRequest { harness: None })).await;

        assert!(result.is_ok());
    }

    #[rstest]
    #[tokio::test]
    async fn search_sessions_is_unimplemented() {
        let cap = Arc::new(AiHarnessSessionCapture::nop().await);
        let svc = Service::new(cap);

        let err = svc
            .search_sessions(Request::new(SearchSessionsRequest {
                query: "x".to_owned(),
                limit: 0,
                harness: None,
            }))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }
}
