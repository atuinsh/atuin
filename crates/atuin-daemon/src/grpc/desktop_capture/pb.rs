//! Model conversion utilities for the `desktop_capture` gRPC protobuf.
mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    tonic::include_proto!("desktop_capture");
}

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use atuin_common::time::SystemTimeExt as _;
pub use codegen::*;
use tonic::Status;
use uuid::Uuid;

use crate::desktop_capture::index::IndexError;
use crate::desktop_capture::store::{
    AppTally as StoredAppTally, DialogText, Kinds, RecordText, SegmentFilter,
    SegmentKind as StoredKind, SegmentSummary, SnapshotSummary, StoreError, Tally, TextBudget,
    Which as StoredWhich,
};
use crate::desktop_capture::{CaptureState, DeleteError, RequestError, StoreNotOpen};

internal_errors!(IndexError);

impl From<StoreError> for Status {
    fn from(error: StoreError) -> Self {
        match error {
            // A store that closes during a shutdown, or a write that conflicts with a concurrent
            // one, refuses the request only for the moment.
            StoreError::Closed | StoreError::Conflict => Self::unavailable(error.to_string()),
            // The other errors can last. For example, a rewritten store that fails to open fails
            // every request until a later attempt opens the store.
            _ => Self::internal(error.to_string()),
        }
    }
}

impl From<StoreNotOpen> for Status {
    fn from(error: StoreNotOpen) -> Self {
        Self::unavailable(error.to_string())
    }
}

impl From<RequestError> for Status {
    fn from(error: RequestError) -> Self {
        match error {
            RequestError::SupervisorEnded => Self::unavailable(error.to_string()),
            RequestError::StopFile(_) => Self::internal(error.to_string()),
        }
    }
}

impl From<DeleteError> for Status {
    fn from(error: DeleteError) -> Self {
        match error {
            DeleteError::NotOpen(_) | DeleteError::SupervisorEnded => {
                Self::unavailable(error.to_string())
            }
            DeleteError::Storage(_) => Self::internal(error.to_string()),
        }
    }
}

impl From<&CaptureState> for State {
    fn from(state: &CaptureState) -> Self {
        match state {
            CaptureState::Disabled => Self::Disabled,
            CaptureState::NoApps => Self::NoApps,
            CaptureState::Stopped => Self::Stopped,
            CaptureState::Starting => Self::Starting,
            CaptureState::Running => Self::Running,
            CaptureState::NeedsPermission => Self::NeedsPermission,
            CaptureState::NoDesktop => Self::NoDesktop,
            CaptureState::ScreenLocked => Self::ScreenLocked,
            CaptureState::Failed { .. } => Self::Failed,
        }
    }
}

impl From<StoredKind> for SegmentKind {
    fn from(kind: StoredKind) -> Self {
        match kind {
            StoredKind::Active => Self::Active,
            StoredKind::Dialog => Self::Dialog,
            StoredKind::Background => Self::Background,
            StoredKind::Notification => Self::Notification,
        }
    }
}

/// The stored kind of a request's segment kind.
pub fn stored_kind(kind: i32) -> Result<StoredKind, Status> {
    match SegmentKind::try_from(kind) {
        Ok(SegmentKind::Active) => Ok(StoredKind::Active),
        Ok(SegmentKind::Dialog) => Ok(StoredKind::Dialog),
        Ok(SegmentKind::Background) => Ok(StoredKind::Background),
        Ok(SegmentKind::Notification) => Ok(StoredKind::Notification),
        Ok(SegmentKind::Unspecified) | Err(_) => {
            Err(Status::invalid_argument(format!("{kind} is not a segment kind")))
        }
    }
}

impl From<SegmentSummary> for Segment {
    fn from(segment: SegmentSummary) -> Self {
        Self {
            id: segment.key.0.to_string(),
            kind: SegmentKind::from(segment.kind).into(),
            app: segment.app,
            title: segment.title,
            url: segment.url,
            started_ms: segment.started.saturating_unix_millis(),
            ended_ms: segment.ended.map(SystemTime::saturating_unix_millis),
            snapshots: segment.snapshots,
        }
    }
}

impl From<SnapshotSummary> for Snapshot {
    fn from(snapshot: SnapshotSummary) -> Self {
        Self {
            record_id: snapshot.record.to_string(),
            at_ms: snapshot.at.saturating_unix_millis(),
            trigger: snapshot
                .trigger
                .map(|trigger| trigger.as_str().to_owned())
                .unwrap_or_default(),
            nodes: snapshot.nodes,
            same_as_previous: snapshot.same_as_previous,
        }
    }
}

impl From<DialogText> for Dialog {
    fn from(dialog: DialogText) -> Self {
        Self {
            segment: Some(dialog.segment.into()),
            text: dialog.text,
        }
    }
}

/// The time of a request's `millis` field named `field`.
pub fn time(field: &str, millis: i64) -> Result<SystemTime, Status> {
    SystemTime::checked_from_unix_millis(millis)
        .ok_or_else(|| Status::invalid_argument(format!("{field} {millis} is out of range")))
}

/// The time of a request's `millis` field named `field`, where 0 means now.
pub fn time_or_now(field: &str, millis: i64) -> Result<SystemTime, Status> {
    match millis {
        0 => Ok(SystemTime::now()),
        millis => time(field, millis),
    }
}

/// The fields of a request that select segments: a time range, an application, and segment
/// kinds. A `to_ms` of 0 means now.
struct SegmentQuery {
    from_ms: i64,
    to_ms: i64,
    app: Option<String>,
    kinds: Vec<i32>,
}

impl TryFrom<SegmentQuery> for SegmentFilter {
    type Error = Status;

    fn try_from(query: SegmentQuery) -> Result<Self, Status> {
        let kinds = if query.kinds.is_empty() {
            Kinds::All
        } else {
            Kinds::Only(query.kinds.into_iter().map(stored_kind).collect::<Result<_, _>>()?)
        };
        Ok(Self {
            from: time("from_ms", query.from_ms)?,
            to: time_or_now("to_ms", query.to_ms)?,
            app: query.app,
            kinds,
        })
    }
}

/// The filter of the request's segments. The request's `limit` is left for the caller.
impl TryFrom<ListSegmentsRequest> for SegmentFilter {
    type Error = Status;

    fn try_from(request: ListSegmentsRequest) -> Result<Self, Status> {
        let ListSegmentsRequest {
            from_ms,
            to_ms,
            app,
            kinds,
            limit: _,
        } = request;
        Self::try_from(SegmentQuery {
            from_ms,
            to_ms,
            app,
            kinds,
        })
    }
}

/// The filter of the request's hits. The request's query and limit are left for the caller.
impl TryFrom<SearchRequest> for SegmentFilter {
    type Error = Status;

    fn try_from(request: SearchRequest) -> Result<Self, Status> {
        let SearchRequest {
            from_ms,
            to_ms,
            app,
            kinds,
            query: _,
            limit: _,
        } = request;
        Self::try_from(SegmentQuery {
            from_ms,
            to_ms,
            app,
            kinds,
        })
    }
}

/// The filter of the segments that the request tallies.
impl TryFrom<TallySegmentsRequest> for SegmentFilter {
    type Error = Status;

    fn try_from(request: TallySegmentsRequest) -> Result<Self, Status> {
        let TallySegmentsRequest {
            from_ms,
            to_ms,
            app,
        } = request;
        Self::try_from(SegmentQuery {
            from_ms,
            to_ms,
            app,
            kinds: Vec::new(),
        })
    }
}

impl From<Tally> for TallySegmentsReply {
    fn from(tally: Tally) -> Self {
        Self {
            apps: tally.apps.into_iter().map(Into::into).collect(),
            from_ms: tally.span.map(|span| span.from.saturating_unix_millis()),
            to_ms: tally.span.map(|span| span.to.saturating_unix_millis()),
            damaged: tally.damaged,
        }
    }
}

impl From<StoredAppTally> for AppTally {
    fn from(tally: StoredAppTally) -> Self {
        let StoredAppTally { app, totals } = tally;
        Self {
            app,
            windows: totals.windows,
            notifications: totals.notifications,
            snapshots: totals.snapshots,
            duration_ms: i64::try_from(totals.duration.as_millis()).unwrap_or(i64::MAX),
        }
    }
}

impl From<RecordText> for SnapshotText {
    fn from(text: RecordText) -> Self {
        Self {
            record_id: text.record.to_string(),
            text: text.text,
            truncated: text.truncated,
        }
    }
}

impl From<SegmentSummary> for LookupReply {
    fn from(segment: SegmentSummary) -> Self {
        Self {
            segment: Some(segment.into()),
        }
    }
}

/// What a deletion request selects.
pub enum DeleteSelection {
    /// The segments that match the filter.
    Filter(SegmentFilter),
    /// The id of the segment or record to look up before the deletion.
    Id(Uuid),
}

/// The selection of what the request deletes. A request without a selection, and a filter that
/// doesn't set any field, are refused.
impl TryFrom<DeleteRequest> for DeleteSelection {
    type Error = Status;

    fn try_from(request: DeleteRequest) -> Result<Self, Status> {
        match request.selection {
            None => Err(Status::invalid_argument("a deletion must say what it deletes")),
            Some(delete_request::Selection::Id(id)) => parse_id("id", &id).map(Self::Id),
            Some(delete_request::Selection::Everything(DeleteEverything {})) => {
                Ok(Self::Filter(SegmentFilter {
                    from: SystemTime::UNIX_EPOCH,
                    to: end_of_time(),
                    app: None,
                    kinds: Kinds::All,
                }))
            }
            Some(delete_request::Selection::Filter(filter)) => filter.try_into().map(Self::Filter),
        }
    }
}

/// The segments that a deletion filter selects. A `to_ms` that is absent selects everything up
/// to the moment of the deletion.
impl TryFrom<DeleteFilter> for SegmentFilter {
    type Error = Status;

    fn try_from(filter: DeleteFilter) -> Result<Self, Status> {
        let DeleteFilter {
            app,
            from_ms,
            to_ms,
        } = filter;
        if app.is_none() && from_ms == 0 && to_ms.is_none() {
            return Err(Status::invalid_argument(
                "a deletion filter must name an application or a time",
            ));
        }
        Ok(Self {
            from: time("from_ms", from_ms)?,
            to: match to_ms {
                None => end_of_time(),
                Some(to_ms) => time("to_ms", to_ms)?,
            },
            app,
            kinds: Kinds::All,
        })
    }
}

/// The most records that one request to read several snapshots can name.
pub const MAX_SNAPSHOT_RECORDS: usize = 256;

/// The most bytes that the texts of one reply to a read of several snapshots hold together.
pub const MAX_SNAPSHOTS_TOTAL_BYTES: usize = 16 * 1024 * 1024;

/// What a request to read several snapshots reads.
pub struct SnapshotsTarget {
    /// The id of a segment, or of one of its snapshots.
    pub id: Uuid,
    /// The records of the snapshots to read.
    pub records: HashSet<Uuid>,
    pub budget: TextBudget,
}

impl TryFrom<ReadSnapshotsRequest> for SnapshotsTarget {
    type Error = Status;

    fn try_from(request: ReadSnapshotsRequest) -> Result<Self, Status> {
        if request.record_ids.len() > MAX_SNAPSHOT_RECORDS {
            return Err(Status::invalid_argument(format!(
                "a read of snapshots names {} records, and it can name at most \
                 {MAX_SNAPSHOT_RECORDS}",
                request.record_ids.len()
            )));
        }
        let records = request.record_ids.iter().map(|id| parse_id("record id", id));
        let total = max_bytes(request.max_total_bytes).min(MAX_SNAPSHOTS_TOTAL_BYTES);
        Ok(Self {
            id: parse_id("id", &request.id)?,
            records: records.collect::<Result<_, _>>()?,
            // The read always returns its first text, so the total also limits each text.
            budget: TextBudget {
                per_text: max_bytes(request.max_bytes).min(total),
                total,
            },
        })
    }
}

/// The longest text for a request's `max_bytes`, where 0 means the whole text.
fn max_bytes(max_bytes: u64) -> usize {
    match max_bytes {
        0 => usize::MAX,
        max => usize::try_from(max).unwrap_or(usize::MAX),
    }
}

/// The UUID in a request's `id` field named `what`.
pub fn parse_id(what: &str, id: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(id).map_err(|err| Status::invalid_argument(format!("{what} {id:?}: {err}")))
}

/// A time later than the time of any record: the last millisecond that the 48-bit time of a
/// UUIDv7 can hold, in the year 10889.
fn end_of_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis((1 << 48) - 1)
}

/// What a request to read a segment reads.
pub struct ReadTarget {
    /// The id of a segment, or of one of its snapshots.
    pub id: Uuid,
    /// The snapshot to read, among the snapshots of the segment that `id` names. `None` reads the
    /// snapshot that `id` names, or the latest snapshot when `id` names a segment.
    pub which: Option<StoredWhich>,
    /// The longest text to return, in bytes.
    pub max_bytes: usize,
}

impl TryFrom<ReadSegmentRequest> for ReadTarget {
    type Error = Status;

    fn try_from(request: ReadSegmentRequest) -> Result<Self, Status> {
        let id = parse_id("id", &request.id)?;
        let which = match request.which() {
            Which::Unspecified => None,
            Which::First => Some(StoredWhich::First),
            Which::Latest => Some(StoredWhich::Latest),
            Which::Record => Some(StoredWhich::Record(parse_id("record id", &request.record_id)?)),
        };
        Ok(Self {
            id,
            which,
            max_bytes: max_bytes(request.max_bytes),
        })
    }
}

/// The number of items for a request's `limit`.
#[must_use]
pub fn limit(limit: u32) -> usize {
    usize::try_from(limit).unwrap_or(usize::MAX)
}

/// The number of hits that a search returns when the request's `limit` is 0.
const DEFAULT_SEARCH_LIMIT: u32 = 20;

/// The most hits that a search returns.
const MAX_SEARCH_LIMIT: u32 = 100;

/// The number of hits for a search request's `limit`.
#[must_use]
pub fn search_limit(limit: u32) -> u32 {
    match limit {
        0 => DEFAULT_SEARCH_LIMIT,
        limit => limit.min(MAX_SEARCH_LIMIT),
    }
}

impl SegmentKind {
    /// The kind's name in machine-readable output, such as JSON and the text of the MCP tools,
    /// which stays the same in every language.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unspecified => "unknown",
            Self::Active => "active",
            Self::Dialog => "dialog",
            Self::Background => "background",
            Self::Notification => "notification",
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const MAX: usize = MAX_SNAPSHOTS_TOTAL_BYTES;

    /// A read of snapshots keeps each text within the read's total, which is at most 16 MiB.
    #[rstest]
    #[case::whole_texts(0, 0, TextBudget { per_text: MAX, total: MAX })]
    #[case::whole_texts_within_a_total(0, 100, TextBudget { per_text: 100, total: 100 })]
    #[case::shorter_texts_than_the_total(10, 100, TextBudget { per_text: 10, total: 100 })]
    #[case::longer_texts_than_the_total(1000, 100, TextBudget { per_text: 100, total: 100 })]
    #[case::a_total_above_the_limit(0, u64::MAX, TextBudget { per_text: MAX, total: MAX })]
    fn read_snapshots_budget(
        #[case] max_bytes: u64,
        #[case] max_total_bytes: u64,
        #[case] expected: TextBudget,
    ) {
        let request = ReadSnapshotsRequest {
            id: Uuid::nil().to_string(),
            record_ids: Vec::new(),
            max_bytes,
            max_total_bytes,
        };

        let target = SnapshotsTarget::try_from(request).expect("a valid request");

        assert_eq!(target.budget, expected);
    }

    #[rstest]
    #[case::closed(StoreError::Closed, tonic::Code::Unavailable)]
    #[case::conflict(StoreError::Conflict, tonic::Code::Unavailable)]
    #[case::damaged(StoreError::Corrupt("a test"), tonic::Code::Internal)]
    #[case::unopened_after_a_rewrite(
        StoreError::Reopen(std::sync::Arc::new(StoreError::Locked)),
        tonic::Code::Internal
    )]
    fn store_errors_keep_their_kind_across_the_boundary(
        #[case] error: StoreError,
        #[case] code: tonic::Code,
    ) {
        assert_eq!(Status::from(error).code(), code);
    }
}
