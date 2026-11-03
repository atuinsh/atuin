pub mod pb;

use std::pin::Pin;
use std::sync::Arc;

use atuin_client::ai_session::{Appended, HarnessSession, SearchTerms, SessionFilter};
use atuin_common::harnesstools::session::SessionId;
use atuin_common::harnesstools::session::pushed::PushedMessage;
use futures::StreamExt;
use tokio_stream::Stream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::metadata::MetadataValue;
use tonic::{Code, Request, Response, Status, Streaming};

use crate::grpc::ai::agent::pb as agent;
use crate::grpc::ai::agent::pb::ParseError;
use crate::grpc::ai::session::pb::ai_session_server::AiSession as GrpcService;
use crate::grpc::ai::session::pb::{
    GetSessionEvent, GetSessionRequest, HarnessFilterRequest, ImportSessionsEvent,
    ImportSessionsProgress, ImportSessionsRequest, ImportSessionsSummary, ListSessionsRequest,
    ProjectSyncedReply, ProjectSyncedRequest, PushMessagesReply, PushMessagesRequest,
    RebuildSessionsReply, RebuildSessionsRequest, SearchSessionsMatch, SearchSessionsRequest,
    SessionRefRequest, TailSessionsEvent, TailSessionsRequest, get_session_event,
    import_sessions_event, tail_sessions_event,
};
use crate::grpc::common::pb as common;
use crate::grpc::common::pb::Lagged;
use crate::session_capture::{
    AiHarnessSessionCapture, AppendError, ImportProgress, RebuildError, SessionTailEvent,
};

#[derive(Clone)]
pub struct Service {
    capture: Arc<AiHarnessSessionCapture>,
}

impl Service {
    #[must_use]
    pub fn new(capture: Arc<AiHarnessSessionCapture>) -> Self {
        Self { capture }
    }

    /// Refuse while startup recovery or a rebuild is still restoring sessions: a read would
    /// succeed with sessions or messages silently missing.
    fn ensure_recovered(&self) -> Result<(), Status> {
        if self.capture.is_recovering() {
            return Err(rebuilding_status(self.capture.recovery_progress()));
        }
        Ok(())
    }
}

/// Metadata key marking the `Unavailable` status returned while startup recovery rebuilds AI
/// sessions, carrying how far the rebuild has got as `<replayed>/<to replay>` records. A dropped
/// connection is `Unavailable` too; the marker is what tells a client this one is worth waiting
/// out.
const REBUILDING_METADATA: &str = "atuin-ai-sessions-rebuilding";

fn rebuilding_status((replayed, pending): (u64, u64)) -> Status {
    let mut status = Status::unavailable(format!(
        "Threads are being rebuilt from the record store ({replayed} of {pending} records); try \
         again shortly"
    ));
    let progress = MetadataValue::try_from(format!("{replayed}/{pending}"))
        .expect("digits and a slash are valid metadata");
    status.metadata_mut().insert(REBUILDING_METADATA, progress);
    status
}

/// How far the daemon has got rebuilding AI sessions after starting, when `status` says it still
/// is: records replayed, and roughly how many there are to replay. `None` for any other status.
#[must_use]
pub fn rebuild_progress(status: &Status) -> Option<(u64, u64)> {
    if status.code() != Code::Unavailable {
        return None;
    }
    let value = status.metadata().get(REBUILDING_METADATA)?.to_str().ok()?;
    let (replayed, pending) = value.split_once('/')?;
    Some((replayed.parse().ok()?, pending.parse().ok()?))
}

/// Whether `status` says the daemon is still rebuilding AI sessions after starting.
#[must_use]
pub fn is_rebuilding(status: &Status) -> bool {
    rebuild_progress(status).is_some()
}

#[tonic::async_trait]
impl GrpcService for Service {
    type ListSessionsStream = Pin<Box<dyn Stream<Item = Result<agent::Session, Status>> + Send>>;
    type GetSessionStream = Pin<Box<dyn Stream<Item = Result<GetSessionEvent, Status>> + Send>>;
    type SearchSessionsStream =
        Pin<Box<dyn Stream<Item = Result<SearchSessionsMatch, Status>> + Send>>;
    type TailSessionsStream = Pin<Box<dyn Stream<Item = Result<TailSessionsEvent, Status>> + Send>>;
    type ImportSessionsStream =
        Pin<Box<dyn Stream<Item = Result<ImportSessionsEvent, Status>> + Send>>;

    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<Self::ListSessionsStream>, Status> {
        self.ensure_recovered()?;
        let filter = SessionFilter::try_from(request.into_inner().filter.unwrap_or_default())?;

        let sessions = self
            .capture
            .list_sessions(&filter)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Stream one session per message so a long list never exceeds the gRPC message size limit.
        let stream = futures::stream::iter(
            sessions.into_iter().map(|session| Ok::<_, Status>(agent::Session::from(session))),
        );

        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<Self::GetSessionStream>, Status> {
        self.ensure_recovered()?;
        let handle = request.into_inner().session()?;

        let session = self
            .capture
            .get_session(&handle)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("session not found"))?;

        let leading = GetSessionEvent {
            event: Some(get_session_event::Event::Session(session.into())),
        };

        let messages = self.capture.messages(&handle).map(|message| {
            message
                .map(|message| GetSessionEvent {
                    event: Some(get_session_event::Event::Message(message.into())),
                })
                .map_err(|e| Status::internal(e.to_string()))
        });

        let stream = futures::stream::once(async move { Ok(leading) }).chain(messages);

        Ok(Response::new(Box::pin(stream)))
    }

    async fn search_sessions(
        &self,
        request: Request<SearchSessionsRequest>,
    ) -> Result<Response<Self::SearchSessionsStream>, Status> {
        self.ensure_recovered()?;
        let SearchSessionsRequest {
            terms,
            limit,
            any_term,
            filter,
        } = request.into_inner();
        let filter = SessionFilter::try_from(filter.unwrap_or_default())?;

        let terms = if any_term {
            SearchTerms::Any(terms)
        } else {
            SearchTerms::All(terms)
        };

        let stream = self.capture.search(terms, &filter, limit).map(|result| {
            result.map(SearchSessionsMatch::from).map_err(|e| Status::internal(e.to_string()))
        });

        Ok(Response::new(Box::pin(stream)))
    }

    async fn tail_sessions(
        &self,
        request: Request<TailSessionsRequest>,
    ) -> Result<Response<Self::TailSessionsStream>, Status> {
        let harness = HarnessFilterRequest::harness(&request.into_inner())?;

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

    async fn rebuild_sessions(
        &self,
        _request: Request<RebuildSessionsRequest>,
    ) -> Result<Response<RebuildSessionsReply>, Status> {
        match self.capture.rebuild().await {
            Ok(()) => Ok(Response::new(RebuildSessionsReply {})),
            Err(err @ RebuildError::Unavailable) => {
                Err(Status::failed_precondition(err.to_string()))
            }
            Err(err @ (RebuildError::Sidecar(_) | RebuildError::Aborted)) => {
                Err(Status::internal(err.to_string()))
            }
        }
    }

    async fn project_synced(
        &self,
        _request: Request<ProjectSyncedRequest>,
    ) -> Result<Response<ProjectSyncedReply>, Status> {
        self.capture.project_synced().await;
        Ok(Response::new(ProjectSyncedReply {}))
    }

    async fn push_messages(
        &self,
        request: Request<Streaming<PushMessagesRequest>>,
    ) -> Result<Response<PushMessagesReply>, Status> {
        self.ensure_recovered()?;
        let mut batches = request.into_inner();
        let (mut push, mut read, mut pushed, mut duplicates) = (None, 0, 0, 0);
        let failed = |e: AppendError| match e {
            AppendError::Unavailable => Status::failed_precondition(e.to_string()),
            e => Status::internal(e.to_string()),
        };
        let mut count = |appended: Vec<Appended>| {
            let new = appended.iter().filter(|a| **a == Appended::New).count() as u64;
            (pushed, duplicates) = (pushed + new, duplicates + appended.len() as u64 - new);
        };
        // Each batch is captured as it comes, so the daemon never holds a whole push, and read as
        // the next lines of the one push.
        let streamed = async {
            while let Some(PushMessagesRequest {
                session,
                lines: batch,
            }) = batches.message().await?
            {
                let session =
                    HarnessSession::try_from(session.ok_or(ParseError::Missing("session"))?)?;
                let (handle, capture) = match &mut push {
                    Some((handle, capture)) => (handle, capture),
                    None => {
                        let id = SessionId::from(String::from(session.session.clone()));
                        let capture =
                            self.capture.push(session.harness, &id).await.map_err(failed)?;
                        let (handle, capture) = push.insert((session.clone(), capture));
                        (handle, capture)
                    }
                };
                if *handle != session {
                    return Err(Status::invalid_argument(
                        "every batch of a push names one session",
                    ));
                }
                let mut lines = Vec::with_capacity(batch.len());
                for line in batch {
                    read += 1;
                    lines.push(
                        serde_json::from_str::<PushedMessage>(&line)
                            .map_err(|e| Status::invalid_argument(format!("line {read}: {e}")))?,
                    );
                }
                count(capture.lines(lines).await.map_err(failed)?);
            }
            Ok::<_, Status>(())
        }
        .await;
        // However the push ended (a bad line, the client gone), what its earlier batches left
        // waiting for a timed line is stored.
        let Some((_, capture)) = push else {
            streamed?;
            return Err(ParseError::Missing("session").into());
        };
        let finished = capture.finish().await.map_err(failed);
        streamed?;
        count(finished?);
        Ok(Response::new(PushMessagesReply { pushed, duplicates }))
    }

    async fn import_sessions(
        &self,
        request: Request<ImportSessionsRequest>,
    ) -> Result<Response<Self::ImportSessionsStream>, Status> {
        self.ensure_recovered()?;
        // A degraded store would otherwise stream an all-zero "success" summary; refuse instead so
        // the caller sees it is unavailable.
        if !self.capture.is_available() {
            return Err(Status::failed_precondition(
                "Thread capture is unavailable: the threads store failed to open or recover",
            ));
        }

        let harness = HarnessFilterRequest::harness(&request.into_inner())?;

        let stream = self.capture.import(harness).filter_map(|progress| {
            let event = match progress {
                ImportProgress::Session {
                    harness,
                    session,
                    imported,
                    skipped,
                    failed: _,
                } => import_sessions_event::Event::Progress(ImportSessionsProgress {
                    harness: harness as i32,
                    session_id: session.into(),
                    imported,
                    skipped,
                }),
                ImportProgress::Finished {
                    sessions,
                    imported,
                    skipped,
                    failed,
                } => import_sessions_event::Event::Summary(ImportSessionsSummary {
                    sessions,
                    imported,
                    skipped,
                    failed,
                }),
                // Folded into the summary's failed count by SessionImporter::run; never streamed.
                ImportProgress::ScanFailed { .. } => return std::future::ready(None),
            };
            std::future::ready(Some(Ok::<_, Status>(ImportSessionsEvent { event: Some(event) })))
        });

        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::session_capture::StoreState;

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
    async fn search_sessions_returns_a_stream() {
        let cap = Arc::new(AiHarnessSessionCapture::nop().await);
        let svc = Service::new(cap);

        let response = svc
            .search_sessions(Request::new(SearchSessionsRequest {
                terms: vec!["anything".to_owned()],
                limit: 0,
                any_term: false,
                filter: None,
            }))
            .await
            .expect("search over an empty sidecar succeeds");

        let matches: Vec<_> = response.into_inner().collect().await;
        assert!(matches.is_empty(), "an empty sidecar yields no matches");
    }

    #[rstest]
    #[tokio::test]
    async fn search_sessions_rejects_an_unknown_harness() {
        let cap = Arc::new(AiHarnessSessionCapture::nop().await);
        let svc = Service::new(cap);

        let result = svc
            .search_sessions(Request::new(SearchSessionsRequest {
                terms: vec!["x".to_owned()],
                limit: 0,
                any_term: false,
                filter: Some(pb::SessionFilter {
                    harness: Some(9999),
                    ..pb::SessionFilter::default()
                }),
            }))
            .await;

        assert!(matches!(result, Err(ref e) if e.code() == tonic::Code::InvalidArgument));
    }

    #[rstest]
    #[tokio::test]
    async fn reads_are_refused_until_recovery_finishes() {
        let (cap, state) = AiHarnessSessionCapture::with_state(StoreState::Recovering).await;
        let svc = Service::new(Arc::new(cap));
        let list = || svc.list_sessions(Request::new(ListSessionsRequest { filter: None }));
        let search = || {
            svc.search_sessions(Request::new(SearchSessionsRequest {
                terms: vec!["x".to_owned()],
                limit: 0,
                any_term: false,
                filter: None,
            }))
        };
        let import = || svc.import_sessions(Request::new(ImportSessionsRequest { harness: None }));

        let rebuilding = |result: Result<(), Status>| result.is_err_and(|e| is_rebuilding(&e));
        assert!(rebuilding(list().await.map(drop)));
        assert!(rebuilding(search().await.map(drop)));
        assert!(rebuilding(import().await.map(drop)));

        state.send_replace(StoreState::Ready);
        assert!(list().await.is_ok());
        assert!(search().await.is_ok());
    }

    #[rstest]
    #[tokio::test]
    async fn a_failed_recovery_still_serves_reads() {
        let (cap, state) = AiHarnessSessionCapture::with_state(StoreState::Recovering).await;
        let svc = Service::new(Arc::new(cap));
        // Recovery ending without reporting (a panic) must not leave reads refused forever.
        drop(state);

        assert!(
            svc.list_sessions(Request::new(ListSessionsRequest { filter: None })).await.is_ok()
        );
    }

    #[rstest]
    #[tokio::test]
    async fn import_sessions_errors_when_capture_is_unavailable() {
        // The nop facade stands in for a failed session store: import must report unavailable
        // rather than stream an all-zero "success" summary.
        let cap = Arc::new(AiHarnessSessionCapture::nop().await);
        let svc = Service::new(cap);

        let result =
            svc.import_sessions(Request::new(ImportSessionsRequest { harness: None })).await;

        assert!(matches!(result, Err(ref e) if e.code() == tonic::Code::FailedPrecondition));
    }

    #[rstest]
    fn only_the_marked_status_is_rebuilding() {
        assert!(is_rebuilding(&rebuilding_status((0, 0))));
        // What a dropped connection looks like: must not be waited on.
        assert!(!is_rebuilding(&Status::unavailable("transport error")));
    }

    #[rstest]
    fn the_rebuilding_status_carries_its_progress() {
        assert_eq!(rebuild_progress(&rebuilding_status((12, 340))), Some((12, 340)));
        assert_eq!(rebuild_progress(&Status::unavailable("transport error")), None);
    }
}
