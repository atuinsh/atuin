use tonic::metadata::errors::InvalidMetadataValue;
use tonic::{Code, Status};
use tonic_types::StatusExt;

const DOMAIN: &str = "hub.atuin.sh";

#[derive(Debug, thiserror::Error)]
pub enum NewHubClientError {
    #[error("the hub endpoint must be http or https, got {0}")]
    UnsupportedScheme(String),
    #[error("the hub endpoint is not a valid URI")]
    InvalidEndpoint(#[source] tonic::transport::Error),
    #[error("the hub token is not a valid header value")]
    InvalidToken(#[source] InvalidMetadataValue),
    #[error("failed to set up TLS")]
    Tls(#[source] native_tls::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("the hub rejected the token")]
    Unauthenticated(#[source] Status),
    #[error("octavo is not enabled for this account")]
    NotEnabled(#[source] Status),
    #[error("the hub is unavailable")]
    Unavailable(#[source] Status),
    #[error("the hub call failed")]
    Other(#[source] Status),
}

#[derive(Debug, thiserror::Error)]
pub enum InsertHistoryError {
    #[error("the hub rejected the history")]
    Invalid(#[source] Status),
    #[error("the hub already has this history")]
    AlreadyExists(#[source] Status),
    #[error(transparent)]
    Call(#[from] CallError),
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryStreamError {
    #[error(transparent)]
    Call(#[from] CallError),
    #[error("the hub sent a watch response without history")]
    MissingHistory,
}

impl From<Status> for CallError {
    fn from(status: Status) -> Self {
        match (status.code(), hub_reason(&status).as_deref()) {
            (Code::Unauthenticated, _) => Self::Unauthenticated(status),
            (Code::FailedPrecondition, Some("OCTAVO_NOT_ENABLED")) => Self::NotEnabled(status),
            (Code::Unavailable, _) => Self::Unavailable(status),
            _ => Self::Other(status),
        }
    }
}

impl From<Status> for InsertHistoryError {
    fn from(status: Status) -> Self {
        match (status.code(), hub_reason(&status).as_deref()) {
            (Code::InvalidArgument, Some("INVALID_HISTORY")) => Self::Invalid(status),
            (Code::AlreadyExists, Some("HISTORY_ALREADY_EXISTS")) => Self::AlreadyExists(status),
            _ => Self::Call(status.into()),
        }
    }
}

fn hub_reason(status: &Status) -> Option<String> {
    status.get_details_error_info().filter(|info| info.domain == DOMAIN).map(|info| info.reason)
}
