mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    #![allow(clippy::large_enum_variant, reason = "prost-generated code")]
    #![allow(clippy::same_name_method, reason = "prost-generated code")]
    tonic::include_proto!("ai.session");
}

use atuin_client::ai_session::{HarnessKind, HarnessSession, Session, SessionMatch};
use atuin_common::string::highlighted::{HighlightedString, HighlightedTextProto};
pub use codegen::*;

use crate::grpc::ai::agent::pb as agent;
use crate::grpc::ai::agent::pb::ParseError;

pub(crate) trait HarnessFilterRequest {
    fn harness_filter(&self) -> Option<i32>;

    fn harness(&self) -> Result<Option<HarnessKind>, ParseError> {
        self.harness_filter()
            .map(|h| HarnessKind::try_from(h).map_err(|_| ParseError::UnknownHarnessKind(h)))
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

impl HarnessFilterRequest for SearchSessionsRequest {
    fn harness_filter(&self) -> Option<i32> {
        self.harness
    }
}

impl From<SessionMatch> for SearchSessionsMatch {
    fn from(value: SessionMatch) -> Self {
        Self {
            session: Some(agent::Session::from(value.session)),
            title: Some(HighlightedTextProto::from(&value.title)),
            preview: Some(HighlightedTextProto::from(&value.preview)),
            score: value.score,
        }
    }
}

impl TryFrom<SearchSessionsMatch> for SessionMatch {
    type Error = ParseError;

    fn try_from(value: SearchSessionsMatch) -> Result<Self, Self::Error> {
        let highlighted = |text: Option<HighlightedTextProto>, field| {
            Ok::<_, ParseError>(HighlightedString::try_from(
                text.ok_or(ParseError::Missing(field))?,
            )?)
        };
        Ok(Self {
            session: Session::try_from(value.session.ok_or(ParseError::Missing("session"))?)?,
            title: highlighted(value.title, "title")?,
            preview: highlighted(value.preview, "preview")?,
            score: value.score,
        })
    }
}

impl HarnessFilterRequest for ImportSessionsRequest {
    fn harness_filter(&self) -> Option<i32> {
        self.harness
    }
}

pub(crate) trait SessionRefRequest {
    fn session_ref(self) -> Option<agent::HarnessSession>;

    fn session(self) -> Result<HarnessSession, ParseError>
    where
        Self: Sized,
    {
        self.session_ref().ok_or(ParseError::Missing("session"))?.try_into()
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

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::NativeSessionId;
    use atuin_common::harnesstools::session::Usage;
    use atuin_common::string::highlighted::TextHighlighter;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;

    fn session_match() -> SessionMatch {
        let highlighter = TextHighlighter::with_markers(['\u{E000}', '\u{E001}']).unwrap();
        SessionMatch {
            session: Session::builder()
                .handle(HarnessSession {
                    harness: HarnessKind::Codex,
                    session: NativeSessionId::from("abc".to_owned()),
                })
                .started_at(OffsetDateTime::UNIX_EPOCH)
                .updated_at(OffsetDateTime::UNIX_EPOCH)
                .usage(Usage::default())
                .build(),
            title: highlighter.as_highlighted("the \u{E000}build\u{E001}".to_owned()),
            preview: highlighter.as_highlighted("a preview".to_owned()),
            score: 2.5,
        }
    }

    #[rstest]
    fn a_session_match_round_trips() {
        let original = session_match();
        let decoded = SessionMatch::try_from(SearchSessionsMatch::from(original.clone())).unwrap();
        assert_eq!(decoded.session, original.session);
        assert_eq!(decoded.title.raw(), original.title.raw());
        assert_eq!(decoded.title.markers(), original.title.markers());
        assert_eq!(decoded.preview.raw(), original.preview.raw());
        assert!((decoded.score - original.score).abs() < f64::EPSILON);
    }

    #[rstest]
    fn a_session_match_without_its_session_is_rejected() {
        let wire = SearchSessionsMatch {
            session: None,
            ..SearchSessionsMatch::from(session_match())
        };
        assert!(matches!(SessionMatch::try_from(wire), Err(ParseError::Missing("session"))));
    }
}
