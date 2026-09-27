mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    #![allow(clippy::large_enum_variant, reason = "prost-generated code")]
    #![allow(clippy::same_name_method, reason = "prost-generated code")]
    tonic::include_proto!("ai.session");
}

use std::path::PathBuf;

use atuin_client::ai_session::{
    HarnessKind, HarnessSession, Session, SessionFilter as DomainSessionFilter, SessionMatch,
};
use atuin_common::string::highlighted::{HighlightedString, HighlightedTextProto};
use atuin_domain::record::HostId;
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
        }
    }
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
            workspace: value.workspace.map(PathBuf::from),
            directory: value.directory.map(PathBuf::from),
            branch: value.branch,
            harness,
            model: value.model,
            roots_only: value.roots_only,
        })
    }
}

/// A request carrying a [`SessionFilter`], and the older bare `harness` filter it supersedes.
pub(crate) trait SessionFilterRequest: HarnessFilterRequest {
    fn session_filter(&self) -> Option<SessionFilter>;

    fn filter(&self) -> Result<DomainSessionFilter, ParseError> {
        let mut filter = self.session_filter().map(TryInto::try_into).transpose()?;
        let filter = filter.get_or_insert_with(DomainSessionFilter::default);
        if filter.harness.is_none() {
            filter.harness = self.harness()?;
        }
        Ok(filter.clone())
    }
}

impl SessionFilterRequest for ListSessionsRequest {
    fn session_filter(&self) -> Option<SessionFilter> {
        self.filter.clone()
    }
}

impl SessionFilterRequest for SearchSessionsRequest {
    fn session_filter(&self) -> Option<SessionFilter> {
        self.filter.clone()
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
    fn a_session_filter_round_trips() {
        let original = DomainSessionFilter {
            host: Some(HostId(uuid::Uuid::from_u128(7))),
            workspace: Some(PathBuf::from("/work/atuin")),
            directory: Some(PathBuf::from("/work/atuin/crates")),
            branch: Some("main".to_owned()),
            harness: Some(HarnessKind::Codex),
            model: Some("opus".to_owned()),
            roots_only: true,
        };
        let decoded = DomainSessionFilter::try_from(SessionFilter::from(&original)).unwrap();
        assert_eq!(decoded, original);
    }

    /// The filter's harness wins over the older bare field, which still applies alone.
    #[rstest]
    #[case::bare_field_alone(Some(HarnessKind::Pi), None, Some(HarnessKind::Pi))]
    #[case::filter_wins(Some(HarnessKind::Pi), Some(HarnessKind::Codex), Some(HarnessKind::Codex))]
    #[case::neither(None, None, None)]
    fn the_filter_harness_supersedes_the_bare_one(
        #[case] bare: Option<HarnessKind>,
        #[case] filtered: Option<HarnessKind>,
        #[case] expected: Option<HarnessKind>,
    ) {
        let request = SearchSessionsRequest {
            query: String::new(),
            limit: 0,
            harness: bare.map(|h| h as i32),
            filter: Some(SessionFilter {
                harness: filtered.map(|h| h as i32),
                ..SessionFilter::default()
            }),
        };
        assert_eq!(request.filter().unwrap().harness, expected);
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
