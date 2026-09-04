//! Model conversion utilities for the `history` gRPC protobuf.
mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    tonic::include_proto!("history");
}

use std::time::Duration;

use atuin_client::history::{History, HistoryId as DomainHistoryId};
use atuin_common::time::OffsetDateTimeExt;
use atuin_domain::record::{CmdOrigin, CmdOriginParseError};
pub use codegen::*;
use easy_cast::Conv;
pub use tail_history_reply::Event as TailHistoryEvent;
use thiserror::Error;
use time::OffsetDateTime;
use tonic::Status;

use crate::grpc::common::pb as common;
use crate::grpc::common::pb::Uuid;
use crate::history_journal::{
    CmdCancelError, CmdDeleteError, CmdEvent, CmdFinishError, CmdRebuildError, GetCmdInFlightError,
};

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
            timestamp: u64::conv(history.timestamp.unix_timestamp_nanos()),
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
            .timestamp(OffsetDateTime::from_unix_nanos_u64(req.timestamp))
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

impl DeleteHistoryRequest {
    pub fn into_history_ids(self) -> impl Iterator<Item = Result<DomainHistoryId, IdParseError>> {
        self.ids.into_iter().map(DomainHistoryId::try_from)
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

invalid_argument_errors!(
    IdParseError,
    StartHistoryRequestParseError,
    EndHistoryRequestParseError,
    CancelHistoryRequestParseError,
);

versioned_messages!(
    StartHistoryReply,
    EndHistoryReply,
    CancelHistoryReply,
    DeleteHistoryReply,
    RebuildHistoryReply,
);

#[cfg(test)]
mod tests {
    use atuin_client::history::AuthorKind as ClientAuthorKind;
    use proptest::prelude::*;
    use rstest::rstest;
    use time::OffsetDateTime;
    use tonic::Code;

    use super::*;

    fn good_id_proto() -> HistoryId {
        HistoryId::from(DomainHistoryId::from_bytes([1u8; 16]))
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
    fn id_parse_error_maps_to_invalid_argument() {
        assert_eq!(Status::from(IdParseError::MissingUuid).code(), Code::InvalidArgument);
    }

    #[rstest]
    #[case::cancel(Status::from(CmdCancelError::NotFound(DomainHistoryId::from_bytes([0u8; 16]))))]
    #[case::get(Status::from(GetCmdInFlightError::NotFound(DomainHistoryId::from_bytes([0u8; 16]))))]
    fn journal_not_found_maps_to_not_found(#[case] status: Status) {
        assert_eq!(status.code(), Code::NotFound);
    }

    fn history_fixture() -> History {
        History::from_db()
            .id(DomainHistoryId::from_bytes([1u8; 16]))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .command("c".into())
            .cwd("/".into())
            .exit(0)
            .duration(0)
            .session("s".into())
            .hostname("h:u".into())
            .author("a".into())
            .intent(None)
            .deleted_at(None)
            .shell(None)
            .author_kind(None)
            .build()
            .into()
    }

    #[rstest]
    fn tail_started_maps_to_started() {
        let event = TailHistoryEvent::from(CmdEvent::Started(history_fixture()));
        assert!(matches!(event, TailHistoryEvent::Started(_)));
    }

    #[rstest]
    fn tail_finished_maps_to_ended() {
        let event = TailHistoryEvent::from(CmdEvent::Finished(history_fixture()));
        assert!(matches!(event, TailHistoryEvent::Ended(_)));
    }

    #[rstest]
    fn tail_cancelled_maps_to_cancelled() {
        let event = TailHistoryEvent::from(CmdEvent::Cancelled(history_fixture()));
        assert!(matches!(event, TailHistoryEvent::Cancelled(_)));
    }
}
