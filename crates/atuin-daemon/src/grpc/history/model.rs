use std::time::Duration;

use atuin_client::history::{History, HistoryId};
use atuin_common::time::OffsetDateTimeExt;
use atuin_domain::record::{CmdOrigin, CmdOriginParseError};
use thiserror::Error;
use time::OffsetDateTime;
use tonic::Status;

use crate::{
    command_journal::{CmdCancelError, CmdFinishError},
    history::{CancelHistoryRequest, EndHistoryRequest, Id, StartHistoryRequest},
};

macro_rules! grpc_invalid_argument {
    ($err:ty) => {
        impl From<$err> for tonic::Status {
            fn from(value: $err) -> Self {
                Self::invalid_argument(value.to_string())
            }
        }
    };
}

impl From<HistoryId> for Id {
    fn from(value: HistoryId) -> Self {
        Self {
            uuid: value.into_bytes().to_vec(),
        }
    }
}

#[derive(Debug, Error)]
pub enum IdParseError {
    #[error("history id must be exactly 16 bytes, got {0}")]
    BadLength(usize),
}

impl TryFrom<Id> for HistoryId {
    type Error = IdParseError;

    fn try_from(value: Id) -> Result<Self, Self::Error> {
        let len = value.uuid.len();
        let bytes: [u8; 16] = value.uuid.try_into().map_err(|_| IdParseError::BadLength(len))?;
        Ok(HistoryId::from_bytes(bytes))
    }
}

grpc_invalid_argument!(IdParseError);

#[derive(Debug, Error)]
pub enum StartHistoryRequestParseError {
    #[error("the given cmd origin is malformed: {0}")]
    BadCmdOrigin(#[from] CmdOriginParseError),
}

grpc_invalid_argument!(StartHistoryRequestParseError);

impl TryFrom<StartHistoryRequest> for History {
    type Error = StartHistoryRequestParseError;

    fn try_from(req: StartHistoryRequest) -> Result<Self, Self::Error> {
        // `author_kind()` borrows `req`, so read it before moving fields out.
        let author_kind = req.author_kind();
        Ok(History::daemon()
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

#[derive(Debug, Error)]
pub enum EndHistoryRequestParseError {
    #[error("missing history id")]
    MissingHistory,
    #[error("invalid id field: {0}")]
    InvalidId(#[from] IdParseError),
}

grpc_invalid_argument!(EndHistoryRequestParseError);

impl TryFrom<EndHistoryRequest> for (HistoryId, i64, Option<Duration>) {
    type Error = EndHistoryRequestParseError;

    fn try_from(value: EndHistoryRequest) -> Result<Self, Self::Error> {
        let id: HistoryId =
            value.id.ok_or(EndHistoryRequestParseError::MissingHistory)?.try_into()?;
        let exit_code = value.exit;
        let duration = (value.duration != 0).then(|| Duration::from_nanos(value.duration));
        Ok((id, exit_code, duration))
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

#[derive(Debug, Error)]
pub enum CancelHistoryRequestParseError {
    #[error("missing history id")]
    MissingHistory,
    #[error("invalid id field: {0}")]
    InvalidId(#[from] IdParseError),
}

grpc_invalid_argument!(CancelHistoryRequestParseError);

impl TryFrom<CancelHistoryRequest> for HistoryId {
    type Error = CancelHistoryRequestParseError;

    fn try_from(value: CancelHistoryRequest) -> Result<Self, Self::Error> {
        Ok(value.id.ok_or(CancelHistoryRequestParseError::MissingHistory)?.try_into()?)
    }
}

impl From<CmdCancelError> for Status {
    fn from(value: CmdCancelError) -> Self {
        match value {
            CmdCancelError::NotFound(_) => Self::not_found(value.to_string()),
        }
    }
}
