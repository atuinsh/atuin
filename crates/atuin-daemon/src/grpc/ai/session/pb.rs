mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    #![allow(clippy::large_enum_variant, reason = "prost-generated code")]
    #![allow(clippy::same_name_method, reason = "prost-generated code")]
    tonic::include_proto!("ai.session");
}

use std::path::PathBuf;

use atuin_client::ai_session::{
    HarnessKind, HarnessSession, MatchedSession as DomainMatchedSession, Session,
    SessionFilter as DomainSessionFilter, SessionMatch,
};
use atuin_common::string::highlighted::{HighlightedString, HighlightedTextProto};
use atuin_common::time::OffsetDateTimeExt;
use atuin_domain::record::HostId;
pub use codegen::*;
use time::OffsetDateTime;

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

impl HarnessFilterRequest for TailSessionsRequest {
    fn harness_filter(&self) -> Option<i32> {
        self.harness
    }
}

impl From<&DomainSessionFilter> for SessionFilter {
    fn from(value: &DomainSessionFilter) -> Self {
        let path = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().into_owned());
        Self {
            host_id: value.host.map(|h| h.as_hyphenated().to_string()),
            workspace: path(&value.workspace),
            directory: path(&value.directory),
            branch: value.branch.clone(),
            harness: value.harness.map(|h| h as i32),
            model: value.model.clone(),
            roots_only: value.roots_only,
            updated_since: value.updated_since.map(timestamp),
        }
    }
}

fn timestamp(ts: OffsetDateTime) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ts.unix_timestamp(),
        nanos: ts.nanosecond().cast_signed(),
    }
}

fn from_timestamp(ts: prost_types::Timestamp) -> Result<OffsetDateTime, ParseError> {
    Ok(OffsetDateTime::from_timespec(ts.seconds.into(), ts.nanos.into())?)
}

impl TryFrom<SessionFilter> for DomainSessionFilter {
    type Error = ParseError;

    fn try_from(value: SessionFilter) -> Result<Self, Self::Error> {
        let harness = value
            .harness
            .map(|h| HarnessKind::try_from(h).map_err(|_| ParseError::UnknownHarnessKind(h)))
            .transpose()?;
        Ok(Self {
            host: value.host_id.map(|h| uuid::Uuid::parse_str(&h)).transpose()?.map(HostId),
            // Not on the wire: only the picker sets it, and it reads the sidecar itself.
            or_unrecorded: false,
            workspace: value.workspace.map(PathBuf::from),
            directory: value.directory.map(PathBuf::from),
            branch: value.branch,
            harness,
            model: value.model,
            roots_only: value.roots_only,
            updated_since: value.updated_since.map(from_timestamp).transpose()?,
        })
    }
}

impl From<SessionMatch> for SearchSessionsMatch {
    fn from(value: SessionMatch) -> Self {
        Self {
            session: Some(agent::Session::from(value.session)),
            title: Some(HighlightedTextProto::from(&value.title)),
            preview: Some(HighlightedTextProto::from(&value.preview)),
            score: value.score,
            message_index: value.message_index,
            matched: value.matched.map(|matched| MatchedSession {
                session: Some(agent::HarnessSession::from(matched.handle)),
                title: Some(HighlightedTextProto::from(&matched.title)),
            }),
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
            message_index: value.message_index,
            matched: value
                .matched
                .map(|matched| {
                    Ok::<_, ParseError>(DomainMatchedSession {
                        handle: matched
                            .session
                            .ok_or(ParseError::Missing("matched.session"))?
                            .try_into()?,
                        title: highlighted(matched.title, "matched.title")?,
                    })
                })
                .transpose()?,
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
            message_index: 7,
            matched: Some(DomainMatchedSession {
                handle: HarnessSession {
                    harness: HarnessKind::Codex,
                    session: NativeSessionId::from("child".to_owned()),
                },
                title: highlighter.as_highlighted("a \u{E000}build\u{E001} fix".to_owned()),
            }),
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
        assert_eq!(decoded.message_index, original.message_index);
        let (matched, expected) = (decoded.matched.unwrap(), original.matched.unwrap());
        assert_eq!(matched.handle, expected.handle);
        assert_eq!(matched.title.raw(), expected.title.raw());
        assert_eq!(matched.title.markers(), expected.title.markers());

        let unmatched = SessionMatch {
            matched: None,
            ..session_match()
        };
        assert!(
            SessionMatch::try_from(SearchSessionsMatch::from(unmatched)).unwrap().matched.is_none()
        );
    }

    #[rstest]
    fn a_session_filter_round_trips() {
        let original = DomainSessionFilter {
            host: Some(HostId(uuid::Uuid::from_u128(7))),
            or_unrecorded: false,
            workspace: Some(PathBuf::from("/work/atuin")),
            directory: Some(PathBuf::from("/work/atuin/crates")),
            branch: Some("main".to_owned()),
            harness: Some(HarnessKind::Codex),
            model: Some("opus".to_owned()),
            roots_only: true,
            updated_since: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(90)),
        };
        let decoded = DomainSessionFilter::try_from(SessionFilter::from(&original)).unwrap();
        assert_eq!(decoded, original);
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
