//! The `DesktopCapture` gRPC service, which serves what desktop capture stored.

pub mod pb;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atuin_common::string::excerpt_around;
use atuin_common::time::SystemTimeExt as _;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument, warn};
use uuid::Uuid;

use self::pb::desktop_capture_server::DesktopCapture as GrpcService;
use self::pb::{
    DeleteReply, DeleteRequest, DeleteSelection, ListSegmentsReply, ListSegmentsRequest,
    ListSnapshotsReply, ListSnapshotsRequest, LookupReply, LookupRequest, ReadSegmentReply,
    ReadSegmentRequest, ReadSnapshotsReply, ReadSnapshotsRequest, ReadTarget, RecentActivityReply,
    RecentActivityRequest, SearchHit, SearchReply, SearchRequest, SessionStartedReply,
    SessionStartedRequest, SnapshotsTarget, StartReply, StartRequest, State, StatusReply,
    StatusRequest, StopReply, StopRequest, TallySegmentsReply, TallySegmentsRequest, parse_id,
};
use crate::desktop_capture::store::{
    DesktopStore, MatchedContent, RecentActivity, SegmentFilter, Selection, Which,
};
use crate::desktop_capture::{CaptureState, DesktopCapture, Opened, StoreNotOpen, search};

/// The length of a search hit's snippet, in characters.
const SNIPPET_CHARS: usize = 200;

/// The most bytes of a dialog's text that recent activity sends.
const DIALOG_TEXT_BYTES: usize = 2048;

/// The longest wait of a request for the store to open.
const STORE_OPEN_WAIT: Duration = Duration::from_secs(30);

/// The longest wait for the outcome of a start that a request can ask for.
const MAX_OUTCOME_WAIT: Duration = Duration::from_secs(30);

/// The `DesktopCapture` gRPC service.
#[derive(Clone)]
pub struct Service {
    capture: Arc<DesktopCapture>,
}

impl Service {
    #[must_use]
    pub const fn new(capture: Arc<DesktopCapture>) -> Self {
        Self { capture }
    }

    /// The open store and index. A request that arrives while the store opens, as when the
    /// request started the daemon, waits for the open.
    async fn opened(&self) -> Result<&Opened, Status> {
        self.capture.opened_within(STORE_OPEN_WAIT).await.ok_or_else(|| StoreNotOpen.into())
    }

    async fn store(&self) -> Result<&DesktopStore, Status> {
        Ok(self.opened().await?.store())
    }
}

/// What a status reply says about the state of capture.
struct StateReply {
    state: State,
    error: String,
}

impl From<CaptureState> for StateReply {
    fn from(state: CaptureState) -> Self {
        match state {
            CaptureState::Failed { error } => Self {
                state: State::Failed,
                error,
            },
            state => Self {
                state: State::from(&state),
                error: String::new(),
            },
        }
    }
}

#[tonic::async_trait]
impl GrpcService for Service {
    #[instrument(skip_all, level = Level::TRACE)]
    async fn status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusReply>, Status> {
        let StateReply { state, error } = self.capture.state().into();
        let counts = match self.capture.store() {
            Some(store) => store
                .counts()
                .await
                .inspect_err(|err| warn!(?err, "could not count the desktop capture store"))
                .ok(),
            None => None,
        };
        Ok(Response::new(StatusReply {
            state: state.into(),
            error,
            segments: counts.map(|counts| counts.segments),
            records: counts.map(|counts| counts.records),
            stopped: self.capture.stopped(),
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn stop(&self, _request: Request<StopRequest>) -> Result<Response<StopReply>, Status> {
        self.capture.stop().await?;
        Ok(Response::new(StopReply {}))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn start(&self, request: Request<StartRequest>) -> Result<Response<StartReply>, Status> {
        self.capture.start().await?;
        let wait = Duration::from_millis(request.into_inner().outcome_wait_ms.into());
        self.capture.settled(wait.min(MAX_OUTCOME_WAIT)).await;
        Ok(Response::new(StartReply {}))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn session_started(
        &self,
        _request: Request<SessionStartedRequest>,
    ) -> Result<Response<SessionStartedReply>, Status> {
        self.capture.session_started();
        Ok(Response::new(SessionStartedReply {}))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn recent_activity(
        &self,
        request: Request<RecentActivityRequest>,
    ) -> Result<Response<RecentActivityReply>, Status> {
        let since = pb::time("since_ms", request.into_inner().since_ms)?;
        let store = self.store().await?;
        let capture_running = self.capture.is_running();
        let RecentActivity { segments, dialogs } =
            store.recent_activity(since, SystemTime::now(), DIALOG_TEXT_BYTES).await?;
        Ok(Response::new(RecentActivityReply {
            capture_running,
            segments: segments.into_iter().map(Into::into).collect(),
            dialogs: dialogs.into_iter().map(Into::into).collect(),
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn list_segments(
        &self,
        request: Request<ListSegmentsRequest>,
    ) -> Result<Response<ListSegmentsReply>, Status> {
        let request = request.into_inner();
        let limit = request.limit;
        let filter = SegmentFilter::try_from(request)?;
        let store = self.store().await?;
        let segments = match limit.map(pb::limit) {
            None => store.segments(filter).await?,
            Some(limit) => store.newest_segments(filter, limit).await?,
        };
        Ok(Response::new(ListSegmentsReply {
            segments: segments.into_iter().map(Into::into).collect(),
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<DeleteReply>, Status> {
        let selection = match DeleteSelection::try_from(request.into_inner())? {
            DeleteSelection::Filter(filter) => Selection::from(filter),
            DeleteSelection::Id(id) => {
                let key =
                    self.store().await?.segment_of(id).await?.ok_or_else(|| no_such_segment(id))?;
                Selection::from(key)
            }
        };
        let deleted = self.capture.delete(selection).await?;
        Ok(Response::new(DeleteReply {
            segments: deleted.segments.len() as u64,
            undecided: deleted.undecided.len() as u64,
            orphans: deleted.orphans.len() as u64,
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn lookup(
        &self,
        request: Request<LookupRequest>,
    ) -> Result<Response<LookupReply>, Status> {
        let id = parse_id("id", &request.into_inner().id)?;
        let found = self.store().await?.lookup(id).await?.ok_or_else(|| no_such_segment(id))?;
        Ok(Response::new(found.into()))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn read_segment(
        &self,
        request: Request<ReadSegmentRequest>,
    ) -> Result<Response<ReadSegmentReply>, Status> {
        let ReadTarget {
            id,
            which,
            max_bytes,
        } = ReadTarget::try_from(request.into_inner())?;
        let read = self
            .store()
            .await?
            .read_text(id, which, max_bytes)
            .await?
            .ok_or_else(|| no_such_snapshot(id, which))?;
        Ok(Response::new(ReadSegmentReply {
            segment: Some(read.segment.into()),
            at_ms: read.at.saturating_unix_millis(),
            text: read.text,
            truncated: read.truncated,
            record_id: read.record.to_string(),
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn list_snapshots(
        &self,
        request: Request<ListSnapshotsRequest>,
    ) -> Result<Response<ListSnapshotsReply>, Status> {
        let id = parse_id("id", &request.into_inner().id)?;
        let listed = self.store().await?.snapshots(id).await?.ok_or_else(|| no_such_segment(id))?;
        Ok(Response::new(ListSnapshotsReply {
            segment: Some(listed.segment.into()),
            snapshots: listed.snapshots.into_iter().map(Into::into).collect(),
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn read_snapshots(
        &self,
        request: Request<ReadSnapshotsRequest>,
    ) -> Result<Response<ReadSnapshotsReply>, Status> {
        let SnapshotsTarget {
            id,
            records,
            budget,
        } = SnapshotsTarget::try_from(request.into_inner())?;
        let wanted = records.len();
        let read = self
            .store()
            .await?
            .read_snapshots(id, records, budget)
            .await?
            .ok_or_else(|| no_such_segment(id))?;
        let found = read.texts.len() + read.left_out;
        if found < wanted {
            return Err(Status::not_found(format!(
                "the segment of {id} lacks {} of the snapshots that the request names",
                wanted - found
            )));
        }
        Ok(Response::new(ReadSnapshotsReply {
            texts: read.texts.into_iter().map(Into::into).collect(),
            left_out: u64::try_from(read.left_out).unwrap_or(u64::MAX),
        }))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn tally_segments(
        &self,
        request: Request<TallySegmentsRequest>,
    ) -> Result<Response<TallySegmentsReply>, Status> {
        let filter = SegmentFilter::try_from(request.into_inner())?;
        let tally = self.store().await?.tally(filter).await?;
        Ok(Response::new(tally.into()))
    }

    #[instrument(skip_all, level = Level::TRACE)]
    async fn search(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchReply>, Status> {
        let mut request = request.into_inner();
        let limit = pb::search_limit(request.limit);
        let query = std::mem::take(&mut request.query);
        let filter = SegmentFilter::try_from(request)?;
        let opened = self.opened().await?;
        let index = opened.index().ok_or_else(|| {
            Status::unavailable("desktop capture search is unavailable: its index is not open")
        })?;
        let hits = search::search(opened.store(), index, &query, filter, limit).await?;
        let hits = hits
            .into_iter()
            .map(|MatchedContent { item, record, at }| SearchHit {
                segment: item.key.segment.0.to_string(),
                kind: pb::SegmentKind::from(item.kind).into(),
                snippet: excerpt_around(&item.text, query.split_whitespace(), SNIPPET_CHARS)
                    .to_owned(),
                app: item.app.to_string(),
                title: item.title.to_string(),
                record_id: record.to_string(),
                at_ms: at.saturating_unix_millis(),
            })
            .collect();
        Ok(Response::new(SearchReply { hits }))
    }
}

/// The error of a read of `which` snapshot of the segment that `id` names, which found no
/// snapshot.
fn no_such_snapshot(id: Uuid, which: Option<Which>) -> Status {
    Status::not_found(match which {
        Some(Which::Record(record)) => format!(
            "the segment of {id} has no snapshot {record}, or there is no segment or snapshot {id}"
        ),
        Some(Which::First | Which::Latest) | None => {
            format!("there is no segment or snapshot {id}, or the segment of {id} has no snapshots")
        }
    })
}

fn no_such_segment(id: Uuid) -> Status {
    Status::not_found(format!("there is no segment or snapshot {id}"))
}
