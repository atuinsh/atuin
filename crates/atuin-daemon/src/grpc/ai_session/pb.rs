mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    #![allow(clippy::large_enum_variant, reason = "prost-generated code")]
    #![allow(clippy::same_name_method, reason = "prost-generated code")]
    tonic::include_proto!("ai.session");
}

use atuin_client::ai_session::{HarnessKind, HarnessSession};
pub use codegen::*;

use crate::grpc::ai_agent::pb as agent;

#[derive(Debug, thiserror::Error)]
pub(crate) enum HarnessFilterParseError {
    #[error("unrecognized harness kind: {0}")]
    Unrecognized(i32),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionRefParseError {
    #[error("missing session")]
    Missing,
    #[error("invalid session: {0}")]
    Invalid(#[from] agent::HarnessSessionParseError),
}

pub(crate) trait HarnessFilterRequest {
    fn harness_filter(&self) -> Option<i32>;

    fn harness(&self) -> Result<Option<HarnessKind>, HarnessFilterParseError> {
        self.harness_filter()
            .map(|h| {
                agent::HarnessKind::try_from(h)
                    .map(HarnessKind::from)
                    .map_err(|_| HarnessFilterParseError::Unrecognized(h))
            })
            .transpose()
    }
}

impl HarnessFilterRequest for ListSessionsRequest {
    fn harness_filter(&self) -> Option<i32> {
        self.harness
    }
}

impl HarnessFilterRequest for TailSessionsRequest {
    fn harness_filter(&self) -> Option<i32> {
        self.harness
    }
}

pub(crate) trait SessionRefRequest {
    fn session_ref(self) -> Option<agent::HarnessSession>;

    fn session(self) -> Result<HarnessSession, SessionRefParseError>
    where
        Self: Sized,
    {
        self.session_ref().ok_or(SessionRefParseError::Missing)?.try_into().map_err(Into::into)
    }
}

impl SessionRefRequest for GetSessionRequest {
    fn session_ref(self) -> Option<agent::HarnessSession> {
        self.session
    }
}

impl SessionRefRequest for GetTranscriptRequest {
    fn session_ref(self) -> Option<agent::HarnessSession> {
        self.session
    }
}

invalid_argument_errors!(HarnessFilterParseError, SessionRefParseError);
