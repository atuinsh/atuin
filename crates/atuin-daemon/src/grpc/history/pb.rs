//! Model conversion utilities for the `history` gRPC protobuf.
mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    tonic::include_proto!("history");
}

use std::ops::Range;
use std::time::Duration;

use atuin_client::history::{
    CommandCapture as DomainCommandCapture, History, HistoryId as DomainHistoryId,
};
use atuin_common::range::PyStyleIdxRange;
use atuin_common::time::OffsetDateTimeExt;
use atuin_domain::record::{CmdOrigin, CmdOriginParseError};
pub use codegen::*;
use easy_cast::Conv;
pub use tail_history_reply::Event as TailHistoryEvent;
use thiserror::Error;
use time::OffsetDateTime;
use tonic::Status;

use crate::grpc::common::pb::{self as common, Uuid};
use crate::grpc::common::{CollectCappedError, TryCollectResultsCappedExt};
use crate::history_journal::{
    CmdCancelError, CmdDeleteError, CmdEvent, CmdFinishError, CmdRebuildError, GetCmdInFlightError,
    RegisterOutputError,
};
use crate::output_capture::{CaptureError, GetOutputError};

impl From<DomainHistoryId> for HistoryId {
    fn from(value: DomainHistoryId) -> Self {
        Self {
            uuid: Some(Uuid {
                value: value.into_bytes().to_vec(),
            }),
        }
    }
}

/// Errors thrown parsing the [`HistoryId`].
#[derive(Debug, Error)]
pub enum IdParseError {
    #[error("history id is missing its uuid")]
    MissingUuid,
    #[error("history id must be exactly 16 bytes, got {0}")]
    BadLength(usize),
}

impl TryFrom<HistoryId> for DomainHistoryId {
    type Error = IdParseError;

    fn try_from(value: HistoryId) -> Result<Self, Self::Error> {
        let uuid = value.uuid.ok_or(IdParseError::MissingUuid)?;
        let len = uuid.value.len();
        let bytes: [u8; 16] = uuid.value.try_into().map_err(|_| IdParseError::BadLength(len))?;
        Ok(Self::from_bytes(bytes))
    }
}

impl From<Option<atuin_client::history::AuthorKind>> for AuthorKind {
    fn from(kind: Option<atuin_client::history::AuthorKind>) -> Self {
        match kind {
            None => Self::Unspecified,
            Some(atuin_client::history::AuthorKind::User) => Self::User,
            Some(atuin_client::history::AuthorKind::Agent) => Self::Agent,
        }
    }
}

impl From<AuthorKind> for Option<atuin_client::history::AuthorKind> {
    fn from(kind: AuthorKind) -> Self {
        match kind {
            AuthorKind::Unspecified => None,
            AuthorKind::User => Some(atuin_client::history::AuthorKind::User),
            AuthorKind::Agent => Some(atuin_client::history::AuthorKind::Agent),
        }
    }
}

impl From<History> for HistoryEntry {
    fn from(history: History) -> Self {
        Self {
            timestamp: i64::conv(history.timestamp.unix_timestamp_nanos()),
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
        }
    }
}

/// Errors thrown parsing the [`StartHistoryRequest`].
#[derive(Debug, Error)]
pub enum StartHistoryRequestParseError {
    #[error("the given cmd origin is malformed: {0}")]
    BadCmdOrigin(#[from] CmdOriginParseError),
}

impl TryFrom<StartHistoryRequest> for History {
    type Error = StartHistoryRequestParseError;

    fn try_from(req: StartHistoryRequest) -> Result<Self, Self::Error> {
        // `author_kind()` borrows `req`, so read it before moving fields out.
        let author_kind = req.author_kind();
        Ok(Self::daemon()
            .timestamp(OffsetDateTime::from_unix_nanos_i64(req.timestamp))
            .command(req.command)
            .cwd(req.cwd)
            .session(req.session)
            .cmd_origin(CmdOrigin::try_from(req.hostname)?)
            .author(req.author)
            .intent(req.intent)
            .shell(req.shell)
            .author_kind(author_kind.into())
            .build()
            .into())
    }
}

/// Errors thrown parsing the [`EndHistoryRequest`].
#[derive(Debug, Error)]
pub enum EndHistoryRequestParseError {
    #[error("invalid history id")]
    MissingHistory,
    #[error("invalid id field: {0}")]
    InvalidId(#[from] IdParseError),
    #[error("invalid duration: {0}")]
    InvalidDuration(#[from] prost_types::DurationError),
}

/// A deserialized view into the [`EndHistoryRequest`] request.
///
/// [`EndHistoryRequest`] does not cleanly map into any particular domain type, so we create a new
/// "view" type for it here.
#[derive(Debug)]
pub struct EndHistoryRequestView {
    /// The ID of the history entry.
    pub history_id: DomainHistoryId,
    /// The exit code of the command.
    pub exit_code: i64,
    /// The duration the command took. [`None`] means the daemon will perform a best-guess estimate
    /// of the length of the command.
    pub duration: Option<Duration>,
}

impl EndHistoryRequest {
    pub fn view(&self) -> Result<EndHistoryRequestView, EndHistoryRequestParseError> {
        Ok(EndHistoryRequestView {
            history_id: self
                .id
                .clone()
                .ok_or(EndHistoryRequestParseError::MissingHistory)?
                .try_into()?,
            exit_code: self.exit,
            duration: self.duration.map(Duration::try_from).transpose()?,
        })
    }
}

/// Errors thrown parsing the [`CancelHistoryRequest`].
#[derive(Debug, Error)]
pub enum CancelHistoryRequestParseError {
    #[error("missing history id")]
    MissingHistory,
    #[error("invalid id field: {0}")]
    InvalidId(#[from] IdParseError),
}

impl TryFrom<CancelHistoryRequest> for DomainHistoryId {
    type Error = CancelHistoryRequestParseError;

    fn try_from(value: CancelHistoryRequest) -> Result<Self, Self::Error> {
        Ok(value.id.ok_or(CancelHistoryRequestParseError::MissingHistory)?.try_into()?)
    }
}

impl IntoIterator for DeleteHistoryRequest {
    type Item = Result<DomainHistoryId, IdParseError>;
    type IntoIter = std::iter::Map<std::vec::IntoIter<HistoryId>, fn(HistoryId) -> Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.ids.into_iter().map(DomainHistoryId::try_from as fn(HistoryId) -> Self::Item)
    }
}

pub trait DeleteHistoryStreamExt {
    fn collect_history_ids(
        self,
    ) -> impl Future<Output = Result<Vec<DomainHistoryId>, CollectCappedError<IdParseError>>> + Send;
}

impl DeleteHistoryStreamExt for tonic::Streaming<DeleteHistoryRequest> {
    fn collect_history_ids(
        self,
    ) -> impl Future<Output = Result<Vec<DomainHistoryId>, CollectCappedError<IdParseError>>> + Send
    {
        const MAX_IDS: usize = 10_000_000;
        self.try_collect_capped(MAX_IDS)
    }
}

/// Map a single journal event to its tail-stream reply.
impl From<CmdEvent> for TailHistoryEvent {
    fn from(event: CmdEvent) -> Self {
        match event {
            CmdEvent::Started(history) => Self::Started(history.into()),
            CmdEvent::Finished(history) => Self::Ended(history.into()),
            CmdEvent::Cancelled(history) => Self::Cancelled(history.into()),
        }
    }
}

impl From<CmdFinishError> for Status {
    fn from(value: CmdFinishError) -> Self {
        match value {
            CmdFinishError::NotFound(_) => Self::not_found(value.to_string()),
            CmdFinishError::HistoryStoreFailed(_) => Self::internal(value.to_string()),
            CmdFinishError::HistoryDbFailed(_) => Self::internal(value.to_string()),
        }
    }
}

impl From<CmdCancelError> for Status {
    fn from(value: CmdCancelError) -> Self {
        match value {
            CmdCancelError::NotFound(_) => Self::not_found(value.to_string()),
        }
    }
}

impl From<CmdDeleteError> for Status {
    fn from(value: CmdDeleteError) -> Self {
        match value {
            CmdDeleteError::HistoryStoreFailed(_) | CmdDeleteError::HistoryDbFailed(_) => {
                Self::internal(value.to_string())
            }
        }
    }
}

impl From<CmdRebuildError> for Status {
    fn from(value: CmdRebuildError) -> Self {
        match value {
            CmdRebuildError::HistoryStoreFailed(_) => Self::internal(value.to_string()),
        }
    }
}

impl From<GetCmdInFlightError> for Status {
    fn from(value: GetCmdInFlightError) -> Self {
        match value {
            GetCmdInFlightError::NotFound(_) => Self::not_found(value.to_string()),
        }
    }
}

impl From<CaptureError> for Status {
    fn from(value: CaptureError) -> Self {
        match value {
            CaptureError::AlreadyExists => Self::already_exists(value.to_string()),
            CaptureError::Storage(_) | CaptureError::Serialize(_) => {
                Self::internal(value.to_string())
            }
        }
    }
}

impl From<RegisterOutputError> for Status {
    fn from(value: RegisterOutputError) -> Self {
        match value {
            RegisterOutputError::NotLive(_) => Self::not_found(value.to_string()),
            RegisterOutputError::HistoryDbFailed(_) => Self::internal(value.to_string()),
            RegisterOutputError::Capture(err) => Self::from(err),
        }
    }
}

/// Errors thrown parsing the [`RegisterCommandOutputRequest`].
#[derive(Debug, Error)]
pub enum RegisterCommandOutputRequestParseError {
    #[error("missing capture")]
    MissingCapture,
    #[error("missing capture metadata")]
    MissingCaptureMeta,
    #[error("missing history id")]
    MissingHistory,
    #[error("invalid id field: {0}")]
    InvalidId(#[from] IdParseError),
}

impl RegisterCommandOutputRequest {
    pub fn history_id(&self) -> Result<DomainHistoryId, RegisterCommandOutputRequestParseError> {
        Ok(self
            .history_id
            .clone()
            .ok_or(RegisterCommandOutputRequestParseError::MissingHistory)?
            .try_into()?)
    }

    /// The capture to store, rejected unless it carries its [`CommandCaptureMeta`].
    pub fn capture(&self) -> Result<CommandCapture, RegisterCommandOutputRequestParseError> {
        let capture =
            self.capture.clone().ok_or(RegisterCommandOutputRequestParseError::MissingCapture)?;
        if capture.meta.is_none() {
            return Err(RegisterCommandOutputRequestParseError::MissingCaptureMeta);
        }
        Ok(capture)
    }
}

impl From<DomainCommandCapture> for CommandCapture {
    fn from(capture: DomainCommandCapture) -> Self {
        Self {
            output_start: capture.output_start,
            output_end: capture.output_end,
            meta: Some(CommandCaptureMeta {
                output_observed_bytes: capture.output_observed_bytes,
                terminal_width: u32::from(capture.terminal_width),
                terminal_height: u32::from(capture.terminal_height),
            }),
        }
    }
}

impl From<CommandCapture> for DomainCommandCapture {
    fn from(capture: CommandCapture) -> Self {
        // The history id travels separately in the request envelope and is applied by the caller
        // as the storage key.
        let meta = capture.meta.unwrap_or_default();
        Self {
            output_start: capture.output_start,
            output_end: capture.output_end,
            output_observed_bytes: meta.output_observed_bytes,
            terminal_width: u16::try_from(meta.terminal_width).unwrap_or(u16::MAX),
            terminal_height: u16::try_from(meta.terminal_height).unwrap_or(u16::MAX),
        }
    }
}

/// Errors thrown parsing a command-output request.
#[derive(Debug, Error)]
pub enum GetOutputRequestParseError {
    #[error("missing history id")]
    MissingHistory,
    #[error("invalid id field: {0}")]
    InvalidId(#[from] IdParseError),
}

impl GetCommandOutputRequest {
    /// Fetch the history ID whose output is being requested.
    pub fn history_id(&self) -> Result<DomainHistoryId, GetOutputRequestParseError> {
        Ok(self.id.clone().ok_or(GetOutputRequestParseError::MissingHistory)?.try_into()?)
    }

    /// The requested line ranges.
    #[must_use]
    pub fn output_ranges(&self) -> &[PyStyleIdxRange] {
        &self.line_ranges
    }
}

#[derive(Clone, Copy)]
pub struct ChunkedOutputLineView<'a> {
    /// The line's position in the output.
    ///
    /// Non-negative numbers are 0-based offsets from the start of the output. Negative numbers
    /// count backward from the end (`-1` is the last line); these indicate lines taken from the end
    /// chunk of an output whose middle was discarded.
    pub line: i64,
    pub content: &'a str,
}

impl GetCommandOutputResponse {
    /// Build a chunked output from an output and a set of signed line ranges.
    #[must_use]
    pub fn build(capture: &CommandCapture, ranges: &[PyStyleIdxRange]) -> Self {
        let lines_start: Vec<&str> = capture.output_start.lines().collect();
        let lines_end: Option<Vec<&str>> =
            capture.output_end.as_ref().map(|end| end.lines().collect());

        /// How to represent a line number.
        enum Anchor {
            /// Use a positive number, indicating an offset from the start.
            Start,
            /// Use a negative number, indicating an offset from the end.
            End,
        }

        let to_output_chunk = |range: Range<usize>, lines: &[&str], anchor| {
            if range.is_empty() {
                return None;
            }
            let offset = match anchor {
                Anchor::Start => 0,
                Anchor::End => i64::conv(lines.len()),
            };
            Some(OutputChunk {
                line_range: Some(PyStyleIdxRange::new(
                    i64::conv(range.start) - offset,
                    i64::conv(range.end) - 1 - offset,
                )),
                content: lines[range].join("\n"),
            })
        };

        let mut truncated = false;
        let chunks = if let Some(lines_end) = &lines_end {
            ranges
                .iter()
                .flat_map(|range| {
                    let ranges = range.resolve_for_split(&lines_start, lines_end);
                    truncated |= ranges.truncated;
                    [
                        to_output_chunk(ranges.start, &lines_start, Anchor::Start),
                        to_output_chunk(ranges.end, lines_end, Anchor::End),
                    ]
                })
                .flatten()
                .collect()
        } else {
            ranges
                .iter()
                .map(|range| range.resolve_for(&lines_start))
                .filter_map(|range| to_output_chunk(range, &lines_start, Anchor::Start))
                .collect()
        };

        Self {
            chunks,
            total_bytes: u64::try_from(capture.output_start.len().saturating_add(
                capture.output_end.as_ref().map(|end| end.len()).unwrap_or_default(),
            ))
            .unwrap_or(u64::MAX),
            total_lines: u64::try_from(
                lines_start
                    .len()
                    .saturating_add(lines_end.map(|lines| lines.len()).unwrap_or_default()),
            )
            .unwrap_or(u64::MAX),
            truncated,
            meta: capture.meta,
        }
    }

    /// Every chunk's lines, each tagged with its position in the output.
    pub fn lines(&self) -> impl Iterator<Item = ChunkedOutputLineView<'_>> + Clone {
        self.chunks.iter().flat_map(|chunk| {
            let (start, count) = chunk.line_range.map_or((0, 0), |range| {
                // Both ends are inclusive, so a one-line chunk has `start == end`.
                let count = range.end.saturating_sub(range.start).saturating_add(1);
                (range.start, usize::try_from(count).unwrap_or(0))
            });

            chunk.content.split('\n').take(count).enumerate().map(move |(offset, line)| {
                ChunkedOutputLineView {
                    line: start.saturating_add(i64::try_from(offset).unwrap_or(i64::MAX)),
                    content: line,
                }
            })
        })
    }
}

invalid_argument_errors!(
    IdParseError,
    StartHistoryRequestParseError,
    EndHistoryRequestParseError,
    CancelHistoryRequestParseError,
    RegisterCommandOutputRequestParseError,
    GetOutputRequestParseError,
);

versioned_messages!(
    StartHistoryReply,
    EndHistoryReply,
    CancelHistoryReply,
    DeleteHistoryReply,
    RebuildHistoryReply,
);

internal_errors!(GetOutputError);

#[cfg(test)]
mod tests {
    use atuin_client::history::AuthorKind as ClientAuthorKind;
    use proptest::prelude::*;
    use rstest::rstest;
    use time::OffsetDateTime;
    use tonic::Code;

    use super::*;
    use crate::grpc::common::TooManyItemsError;

    fn good_id_proto() -> HistoryId {
        HistoryId::from(DomainHistoryId::from_bytes([1u8; 16]))
    }

    fn id_proto(byte: u8) -> HistoryId {
        HistoryId::from(DomainHistoryId::from_bytes([byte; 16]))
    }

    /// A proto id that fails to parse: no uuid at all.
    fn bad_id_proto() -> HistoryId {
        HistoryId { uuid: None }
    }

    fn delete_req(ids: &[HistoryId]) -> DeleteHistoryRequest {
        DeleteHistoryRequest { ids: ids.to_vec() }
    }

    // These exercise the `DeleteHistoryRequest` wiring -- its `IntoIterator` glue and the
    // `IdParseError -> Status` mapping -- through the generic collector. The collector's own
    // behavior (cap, item/stream errors) is covered in `grpc::common::pb`.

    #[rstest]
    #[tokio::test]
    async fn delete_stream_collects_ids_across_chunks_in_order() {
        let stream = futures::stream::iter(vec![
            Ok(delete_req(&[id_proto(1), id_proto(2)])),
            Ok(delete_req(&[id_proto(3)])),
        ]);
        let ids = stream.try_collect_capped(100).await.unwrap();
        assert_eq!(ids, vec![
            DomainHistoryId::from_bytes([1; 16]),
            DomainHistoryId::from_bytes([2; 16]),
            DomainHistoryId::from_bytes([3; 16]),
        ]);
    }

    #[rstest]
    #[tokio::test]
    async fn delete_stream_maps_a_malformed_id_to_invalid_argument() {
        let stream =
            futures::stream::iter(vec![Ok(delete_req(&[good_id_proto(), bad_id_proto()]))]);
        let err = stream.try_collect_capped(100).await.unwrap_err();
        assert!(matches!(err, CollectCappedError::Item(IdParseError::MissingUuid)));
        assert_eq!(Status::from(err).code(), Code::InvalidArgument);
    }

    #[rstest]
    #[tokio::test]
    async fn delete_stream_maps_over_cap_to_invalid_argument() {
        let stream =
            futures::stream::iter(vec![Ok(delete_req(&[id_proto(1), id_proto(2), id_proto(3)]))]);
        let err = stream.try_collect_capped(2).await.unwrap_err();
        assert!(matches!(err, CollectCappedError::TooMany(TooManyItemsError(2))));
        assert_eq!(Status::from(err).code(), Code::InvalidArgument);
    }

    fn capture_of(output: &str) -> CommandCapture {
        CommandCapture {
            output_start: output.to_string(),
            output_end: None,
            meta: Some(CommandCaptureMeta {
                output_observed_bytes: u64::conv(output.len()),
                terminal_width: 80,
                terminal_height: 24,
            }),
        }
    }

    /// A capture that lost its middle: `start` and `end` were kept, everything between them was
    /// discarded, and `observed` bytes went past the terminal in total.
    fn split_capture_of(start: &str, end: &str, observed: u64) -> CommandCapture {
        CommandCapture {
            output_start: start.to_string(),
            output_end: Some(end.to_string()),
            meta: Some(CommandCaptureMeta {
                output_observed_bytes: observed,
                terminal_width: 80,
                terminal_height: 24,
            }),
        }
    }

    /// A requested range, Python-slice style (inclusive ends, negatives from the end).
    fn py_range(start: i64, end: i64) -> PyStyleIdxRange {
        PyStyleIdxRange::new(start, end)
    }

    fn register_req(capture: Option<CommandCapture>) -> RegisterCommandOutputRequest {
        RegisterCommandOutputRequest {
            history_id: Some(good_id_proto()),
            capture,
        }
    }

    #[rstest]
    fn register_command_output_accepts_a_capture_carrying_its_meta() {
        let capture = register_req(Some(capture_of("hello"))).capture().expect("capture");
        assert_eq!(capture.output_start, "hello");
        assert!(capture.meta.is_some());
    }

    #[rstest]
    // A capture with no `meta` would read back with a zeroed `output_observed_bytes`, so output
    // that overflowed the limit would look complete. Both omissions are rejected instead.
    #[case::no_meta(Some(CommandCapture {
        output_start: "hi".to_string(),
        output_end: None,
        meta: None,
    }))]
    #[case::no_capture(None)]
    fn register_command_output_rejects_an_incomplete_capture(
        #[case] capture: Option<CommandCapture>,
    ) {
        let err = register_req(capture).capture().expect_err("should be rejected");
        assert_eq!(Status::from(err).code(), Code::InvalidArgument);
    }

    #[rstest]
    fn command_output_whole_output_via_full_range() {
        let chunked = GetCommandOutputResponse::build(&capture_of("a\nb\nc"), &[py_range(0, -1)]);
        assert!(chunked.meta.is_some());
        assert_eq!(chunked.total_lines, 3);
        assert_eq!(chunked.chunks.len(), 1);
        assert_eq!(chunked.chunks[0].content, "a\nb\nc");
        assert_eq!(chunked.chunks[0].line_range, Some(py_range(0, 2)));
    }

    #[rstest]
    fn command_output_ranges_are_inclusive_with_negative_offsets() {
        // [1, 2] inclusive -> "one", "two"; [-1, -1] -> the last line, "four" (no sentinel needed).
        let chunked =
            GetCommandOutputResponse::build(&capture_of("zero\none\ntwo\nthree\nfour"), &[
                py_range(1, 2),
                py_range(-1, -1),
            ]);
        assert!(chunked.meta.is_some());
        assert_eq!(chunked.total_lines, 5);
        let contents: Vec<&str> = chunked.chunks.iter().map(|c| c.content.as_str()).collect();
        assert_eq!(contents, vec!["one\ntwo", "four"]);
        // The reported `line_range` is the resolved, concrete half-open span.
        assert_eq!(chunked.chunks[0].line_range, Some(py_range(1, 2)));
        assert_eq!(chunked.chunks[1].line_range, Some(py_range(4, 4)));
    }

    #[rstest]
    fn command_output_drops_ranges_that_select_nothing() {
        // A range that selects nothing yields no chunk at all: [2, 1] is backwards and [10, 20] is
        // past the end, so only [0, 0] survives, selecting "a".
        //
        // A chunk's `line_range` is inclusive at both ends, which cannot encode an empty span --
        // the natural candidate, [0, -1], already means "the whole output". So an empty range has
        // to be dropped rather than reported as a zero-length chunk. Callers already had to treat
        // empty chunks as contributing nothing, so no line is lost by leaving them out.
        let chunked = GetCommandOutputResponse::build(&capture_of("a\nb\nc"), &[
            py_range(2, 1),
            py_range(10, 20),
            py_range(0, 0),
        ]);
        let contents: Vec<&str> = chunked.chunks.iter().map(|c| c.content.as_str()).collect();
        assert_eq!(contents, vec!["a"]);
        assert_eq!(chunked.chunks[0].line_range, Some(py_range(0, 0)));
    }

    // -- Captures that lost their middle --------------------------------------

    /// Six lines of output whose middle was discarded: `a b c` were kept from the front and
    /// `x y z` from the back, with an unknown number of lines gone between them.
    fn split_abc_xyz() -> CommandCapture {
        split_capture_of("a\nb\nc", "x\ny\nz", 1_000_000)
    }

    #[rstest]
    fn split_capture_reports_both_halves_for_a_full_range() {
        let chunked = GetCommandOutputResponse::build(&split_abc_xyz(), &[py_range(0, -1)]);

        // One chunk per half, in order, each carrying only the lines it really holds.
        let contents: Vec<&str> = chunked.chunks.iter().map(|c| c.content.as_str()).collect();
        assert_eq!(contents, vec!["a\nb\nc", "x\ny\nz"]);
        // The kept start is numbered from the front and the kept tail from the back, so the two
        // halves can never be mistaken for one contiguous run.
        assert_eq!(chunked.chunks[0].line_range, Some(py_range(0, 2)));
        assert_eq!(chunked.chunks[1].line_range, Some(py_range(-3, -1)));
        // The request spanned the gap, so the caller is told the output is incomplete.
        assert!(chunked.truncated);
    }

    #[rstest]
    fn split_capture_totals_count_only_what_was_kept() {
        let chunked = GetCommandOutputResponse::build(&split_abc_xyz(), &[py_range(0, -1)]);
        assert_eq!(chunked.total_lines, 6, "three lines either side of the gap");
        assert_eq!(chunked.total_bytes, u64::conv("a\nb\nc".len() + "x\ny\nz".len()));
        // The bytes the terminal actually saw are reported separately, and are far larger.
        assert_eq!(chunked.meta.expect("meta").output_observed_bytes, 1_000_000);
    }

    #[rstest]
    // Wholly inside the kept start: answered in full, nothing missing.
    #[case::within_the_start(py_range(0, 1), vec![(0, "a"), (1, "b")], false)]
    #[case::exactly_the_start(py_range(0, 2), vec![(0, "a"), (1, "b"), (2, "c")], false)]
    // Wholly inside the kept tail: likewise, and numbered from the end.
    #[case::within_the_tail(py_range(-2, -1), vec![(-2, "y"), (-1, "z")], false)]
    #[case::exactly_the_tail(py_range(-3, -1), vec![(-3, "x"), (-2, "y"), (-1, "z")], false)]
    // Reaching past either kept half runs into the gap, which must be reported.
    #[case::past_the_start(py_range(0, 5), vec![(0, "a"), (1, "b"), (2, "c")], true)]
    #[case::past_the_tail(py_range(-9, -1), vec![(-3, "x"), (-2, "y"), (-1, "z")], true)]
    // Entirely inside the gap: nothing to hand back, but the caller must not read that as an
    // empty output.
    #[case::inside_the_gap_from_the_front(py_range(8, 9), vec![], true)]
    #[case::inside_the_gap_from_the_back(py_range(-99, -90), vec![], true)]
    // Spanning it: both halves, and the gap between them.
    #[case::spanning(py_range(1, -2), vec![(1, "b"), (2, "c"), (-3, "x"), (-2, "y")], true)]
    fn split_capture_answers_a_range_from_the_half_that_holds_it(
        #[case] range: PyStyleIdxRange,
        #[case] expected: Vec<(i64, &str)>,
        #[case] truncated: bool,
    ) {
        let chunked = GetCommandOutputResponse::build(&split_abc_xyz(), &[range]);
        assert_eq!(views_of(&chunked), expected);
        assert_eq!(chunked.truncated, truncated);
    }

    #[rstest]
    fn split_capture_truncation_is_sticky_across_ranges() {
        // One range inside the kept start, one spanning the gap. The response carries a single
        // flag, so it has to report the incompleteness of the request as a whole.
        let chunked =
            GetCommandOutputResponse::build(&split_abc_xyz(), &[py_range(0, 0), py_range(0, -1)]);
        assert!(chunked.truncated);
    }

    #[rstest]
    fn a_whole_capture_never_reports_truncation() {
        // Nothing was discarded, so no request can reach a gap -- not even one running well past
        // both ends of the output.
        let chunked = GetCommandOutputResponse::build(&capture_of("a\nb\nc"), &[
            py_range(0, -1),
            py_range(0, 99),
            py_range(-99, -1),
        ]);
        assert!(!chunked.truncated);
    }

    #[rstest]
    fn an_empty_tail_still_marks_the_capture_as_split() {
        // The limit left no room for a tail. There is nothing to hand back from it, but the
        // output is still incomplete and a full-range request has to say so.
        let capture = split_capture_of("a\nb\nc", "", 1_000_000);
        let chunked = GetCommandOutputResponse::build(&capture, &[py_range(0, -1)]);

        assert!(chunked.truncated);
        assert_eq!(views_of(&chunked), vec![(0, "a"), (1, "b"), (2, "c")]);
        assert_eq!(chunked.total_lines, 3);
    }

    /// The lines a chunked output actually hands back, as `(line number, content)` pairs.
    fn views_of(chunked: &GetCommandOutputResponse) -> Vec<(i64, &str)> {
        chunked.lines().map(|view| (view.line, view.content)).collect()
    }

    #[rstest]
    // A trailing newline terminates the last line instead of starting an empty one, so "a\nb\nc"
    // and "a\nb\nc\n" are both three lines. The capture path already trims trailing newlines, but
    // the count must not quietly depend on that having happened.
    #[case::no_trailing_newline("a\nb\nc", 3)]
    #[case::one_trailing_newline("a\nb\nc\n", 3)]
    #[case::two_trailing_newlines("a\nb\nc\n\n", 4)]
    #[case::blank_line_at_the_end("a\nb\n\n", 3)]
    #[case::single_line_no_newline("a", 1)]
    #[case::just_a_newline("\n", 1)]
    #[case::empty("", 0)]
    fn command_output_counts_lines_without_a_trailing_newline_sentinel(
        #[case] output: &str,
        #[case] expected: u64,
    ) {
        let chunked = GetCommandOutputResponse::build(&capture_of(output), &[py_range(0, -1)]);
        assert_eq!(chunked.total_lines, expected);
        // Whatever `total_lines` claims, asking for everything hands back exactly that many lines.
        assert_eq!(chunked.lines().count(), usize::try_from(expected).unwrap());
    }

    #[rstest]
    // A chunk is stored as its lines joined by "\n", and that join is not reversible on its own:
    // `str::lines` reads both "a" and "a\n" as a single line. So a blank line at the end of a
    // chunk is exactly where a line goes missing and every later line number looks like a gap.
    #[case::blank_line_ends_a_chunk("a\n\nb", vec![py_range(0, 1)], vec![(0, "a"), (1, "")])]
    // A lone blank line joins to "", which must still read back as one selected line rather than
    // vanishing and leaving the caller believing nothing matched.
    #[case::chunk_is_only_a_blank_line("a\n\nb", vec![py_range(1, 1)], vec![(1, "")])]
    // Blank lines away from a chunk boundary were never at risk, but pin them so a future fix
    // cannot trade one failure for the other.
    #[case::interior_blank_lines_survive(
        "a\n\n\nb",
        vec![py_range(0, -1)],
        vec![(0, "a"), (1, ""), (2, ""), (3, "b")]
    )]
    // Two chunks, the first ending on blank line 1. Readers infer skipped lines from jumps in
    // these numbers, so losing line 1 would report a three-line gap where only lines 2 and 3 are
    // genuinely missing.
    #[case::gap_is_not_widened_by_a_blank_line(
        "alpha\n\ncharlie\ndelta\necho\nfoxtrot",
        vec![py_range(0, 1), py_range(4, 5)],
        vec![(0, "alpha"), (1, ""), (4, "echo"), (5, "foxtrot")]
    )]
    // Backwards and past-the-end ranges resolve to an empty span, which must contribute nothing
    // at all rather than one phantom blank line.
    #[case::empty_ranges_contribute_nothing(
        "a\nb\nc",
        vec![py_range(2, 1), py_range(10, 20)],
        vec![]
    )]
    fn command_output_lines_survive_the_chunk_round_trip(
        #[case] output: &str,
        #[case] ranges: Vec<PyStyleIdxRange>,
        #[case] expected: Vec<(i64, &str)>,
    ) {
        let chunked = GetCommandOutputResponse::build(&capture_of(output), &ranges);
        assert_eq!(views_of(&chunked), expected);

        // Content and line numbers stay in step: every chunk hands back exactly as many lines as
        // the range it reports covers.
        let declared: usize = chunked
            .chunks
            .iter()
            .map(|chunk| {
                let resolved = chunk.line_range.expect("build always sets the resolved range");
                usize::try_from(resolved.end - resolved.start + 1).unwrap()
            })
            .sum();
        assert_eq!(declared, expected.len());
    }

    proptest! {
        /// However the output is shaped and whatever ranges are asked for, each chunk hands back
        /// exactly the lines its `line_range` claims, numbered consecutively from that start. This
        /// is the contract a reader depends on to tell a real gap in the output from a lost line.
        #[test]
        fn chunk_line_views_match_the_declared_ranges(
            output in "[ab\r\n]{0,40}",
            ranges in proptest::collection::vec((-8i64..8, -8i64..8), 0..4),
        ) {
            let ranges: Vec<PyStyleIdxRange> =
                ranges.into_iter().map(|(start, end)| PyStyleIdxRange::new(start, end)).collect();
            let chunked = GetCommandOutputResponse::build(&capture_of(&output), &ranges);

            let mut expected: Vec<i64> = Vec::new();
            for chunk in &chunked.chunks {
                let resolved = chunk.line_range.expect("build always sets the resolved range");
                expected.extend(resolved.start..=resolved.end);
            }

            let actual: Vec<i64> = chunked.lines().map(|view| view.line).collect();
            prop_assert_eq!(actual, expected);
        }
    }

    fn end_req(
        id: Option<HistoryId>,
        exit: i64,
        duration: Option<prost_types::Duration>,
    ) -> EndHistoryRequest {
        EndHistoryRequest { id, exit, duration }
    }

    fn parse_end(
        req: &EndHistoryRequest,
    ) -> Result<EndHistoryRequestView, EndHistoryRequestParseError> {
        req.view()
    }

    proptest! {
        /// A DomainHistoryId survives the proto round trip for every possible id, and the proto carries
        /// the id's raw 16 bytes verbatim -- pinning byte order against a symmetric from/into swap.
        #[test]
        fn history_id_round_trips_and_wire_bytes(b in proptest::array::uniform16(any::<u8>())) {
            let proto = HistoryId::from(DomainHistoryId::from_bytes(b));
            prop_assert_eq!(proto.uuid.as_ref().unwrap().value.clone(), b.to_vec());
            prop_assert_eq!(DomainHistoryId::try_from(proto).unwrap().into_bytes(), b);
        }

        /// Any uuid payload whose length is not 16 is rejected as BadLength reporting the real length,
        /// never a panic.
        #[test]
        fn history_id_rejects_wrong_length(
            v in proptest::collection::vec(any::<u8>(), 0..40usize).prop_filter("not 16", |v| v.len() != 16),
        ) {
            let proto = HistoryId { uuid: Some(Uuid { value: v.clone() }) };
            match DomainHistoryId::try_from(proto) {
                Err(IdParseError::BadLength(len)) => prop_assert_eq!(len, v.len()),
                other => prop_assert!(false, "expected BadLength, got {:?}", other),
            }
        }
    }

    #[rstest]
    #[case::missing(None, "missing its uuid")]
    #[case::short(Some(vec![0u8; 15]), "got 15")]
    #[case::long(Some(vec![0u8; 17]), "got 17")]
    fn history_id_parse_errors(#[case] value: Option<Vec<u8>>, #[case] fragment: &str) {
        let proto = HistoryId {
            uuid: value.map(|value| Uuid { value }),
        };
        let err = DomainHistoryId::try_from(proto).unwrap_err();
        assert!(err.to_string().contains(fragment), "{err}");
    }

    #[rstest]
    fn history_entry_field_routing() {
        let h: History = History::from_db()
            .id(DomainHistoryId::from_bytes([7u8; 16]))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .command("CMD".into())
            .cwd("CWD".into())
            .exit(3)
            .duration(42)
            .session("SES".into())
            .hostname("hostx:usery".into())
            .author("AUTH".into())
            .intent(None)
            .deleted_at(None)
            .shell(None)
            .author_kind(Some(ClientAuthorKind::Agent))
            .build()
            .into();

        let e = HistoryEntry::from(h);

        assert_eq!(e.command, "CMD");
        assert_eq!(e.cwd, "CWD");
        assert_eq!(e.session, "SES");
        assert_eq!(e.author, "AUTH");
        assert_eq!(e.hostname, "hostx:usery");
        assert_eq!((e.exit, e.duration), (3, 42));
        assert_eq!((e.intent.as_str(), e.shell.as_str()), ("", ""));
        assert_eq!(e.id.unwrap().uuid.unwrap().value, [7u8; 16].to_vec());
        assert_eq!(e.author_kind, AuthorKind::Agent as i32);
    }

    fn start_req(hostname: &str) -> StartHistoryRequest {
        StartHistoryRequest {
            timestamp: 0,
            command: "cmd".into(),
            cwd: "/".into(),
            session: "ses".into(),
            hostname: hostname.into(),
            author: "auth".into(),
            intent: "intent".into(),
            shell: "bash".into(),
            author_kind: 0,
        }
    }

    #[rstest]
    fn start_request_rejects_colonless_hostname() {
        let err = History::try_from(start_req("nocolon")).unwrap_err();
        assert!(matches!(err, StartHistoryRequestParseError::BadCmdOrigin(_)));
    }

    #[rstest]
    fn start_request_defaults_exit_and_duration_to_unmeasured() {
        let h = History::try_from(start_req("host:user")).unwrap();
        assert_eq!((h.exit, h.duration), (-1, -1));
    }

    #[rstest]
    fn end_request_none_duration_is_preserved() {
        let view = parse_end(&end_req(Some(good_id_proto()), 5, None)).unwrap();
        assert_eq!(view.exit_code, 5);
        assert!(view.duration.is_none());
    }

    #[rstest]
    fn end_request_some_duration_is_parsed() {
        let d = prost_types::Duration {
            seconds: 0,
            nanos: 3,
        };
        let view = parse_end(&end_req(Some(good_id_proto()), 0, Some(d))).unwrap();
        assert_eq!(view.duration, Some(Duration::from_nanos(3)));
    }

    #[rstest]
    fn end_request_negative_duration_errs_without_panic() {
        let d = prost_types::Duration {
            seconds: -1,
            nanos: 0,
        };
        let err = parse_end(&end_req(Some(good_id_proto()), 0, Some(d))).unwrap_err();
        assert!(matches!(err, EndHistoryRequestParseError::InvalidDuration(_)));
    }

    #[rstest]
    fn end_request_missing_id_errs() {
        let err = parse_end(&end_req(None, 0, None)).unwrap_err();
        assert!(matches!(err, EndHistoryRequestParseError::MissingHistory));
    }

    #[rstest]
    #[case::missing(None)]
    #[case::bad_len(Some(HistoryId { uuid: Some(Uuid { value: vec![0u8; 15] }) }))]
    fn cancel_request_rejects_bad_id(#[case] id: Option<HistoryId>) {
        assert!(DomainHistoryId::try_from(CancelHistoryRequest { id }).is_err());
    }

    #[rstest]
    fn cancel_request_good_id_ok() {
        assert!(
            DomainHistoryId::try_from(CancelHistoryRequest {
                id: Some(good_id_proto())
            })
            .is_ok()
        );
    }

    #[rstest]
    #[case(CmdFinishError::NotFound(DomainHistoryId::from_bytes([0u8; 16])), Code::NotFound)]
    #[case(CmdFinishError::HistoryStoreFailed(eyre::eyre!("x")), Code::Internal)]
    #[case(CmdFinishError::HistoryDbFailed(eyre::eyre!("x")), Code::Internal)]
    fn finish_error_status_codes(#[case] err: CmdFinishError, #[case] code: Code) {
        assert_eq!(Status::from(err).code(), code);
    }

    #[rstest]
    #[case(RegisterOutputError::NotLive(DomainHistoryId::from_bytes([0u8; 16])), Code::NotFound)]
    #[case(RegisterOutputError::HistoryDbFailed(eyre::eyre!("x")), Code::Internal)]
    #[case(RegisterOutputError::Capture(CaptureError::AlreadyExists), Code::AlreadyExists)]
    fn register_output_error_status_codes(#[case] err: RegisterOutputError, #[case] code: Code) {
        assert_eq!(Status::from(err).code(), code);
    }

    #[rstest]
    fn id_parse_error_maps_to_invalid_argument() {
        assert_eq!(Status::from(IdParseError::MissingUuid).code(), Code::InvalidArgument);
    }

    #[rstest]
    #[case::cancel(Status::from(CmdCancelError::NotFound(DomainHistoryId::from_bytes([0u8; 16]))))]
    #[case::get(Status::from(GetCmdInFlightError::NotFound(DomainHistoryId::from_bytes([0u8; 16]))))]
    fn journal_not_found_maps_to_not_found(#[case] status: Status) {
        assert_eq!(status.code(), Code::NotFound);
    }
}
