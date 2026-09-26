use std::path::PathBuf;

use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{
    Content, Role, StopReason, TitleChange, TitleSource, Usage,
};
use atuin_common::string::highlighted::HighlightedString;
use atuin_domain::record::RecordId;
use derive_more::{AsRef, Display, From, Into};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use typed_builder::TypedBuilder;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "proto", derive(prost::Enumeration))]
pub enum HarnessKind {
    Unknown = 0,
    ClaudeCode = 1,
    Codex = 2,
    Copilot = 3,
    Opencode = 4,
    Pi = 5,
}

impl From<&AnyHarness> for HarnessKind {
    fn from(value: &AnyHarness) -> Self {
        match value {
            AnyHarness::ClaudeCode(_) => Self::ClaudeCode,
            AnyHarness::Codex(_) => Self::Codex,
            AnyHarness::Opencode(_) => Self::Opencode,
            AnyHarness::Pi(_) => Self::Pi,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, From, Into, AsRef, Display)]
#[as_ref(str)]
pub struct NativeSessionId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, From, Into, AsRef, Display)]
#[as_ref(str)]
pub struct SourceId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HarnessSession {
    pub harness: HarnessKind,
    pub session: NativeSessionId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder)]
pub struct Message {
    pub id: RecordId,
    pub session: HarnessSession,
    pub source_id: SourceId,
    #[builder(default)]
    pub parent: Option<HarnessSession>,
    #[builder(default)]
    pub parent_source_id: Option<SourceId>,
    pub timestamp: OffsetDateTime,
    pub role: Role,
    pub content: Vec<Content>,
    #[builder(default)]
    pub cwd: Option<PathBuf>,
    #[builder(default)]
    pub git_branch: Option<String>,
    #[builder(default)]
    pub model: Option<String>,
    /// The usage this row reported, exactly as the harness reported it. Every row of one model
    /// call (see `turn_id`) may repeat or grow the same figures, and copies of a call in forked
    /// sessions repeat them again, so rows are never summed: [`Session::usage`] counts each call
    /// once.
    #[builder(default)]
    pub usage: Option<Usage>,
    #[builder(default)]
    pub stop_reason: Option<StopReason>,
    /// The session's title at capture time, denormalised onto the message so session-level metadata
    /// survives a reproject from the synced record store (records carry messages only, not the
    /// separate `Started` metadata). `None` once a title is cleared: the newest row decides.
    #[builder(default)]
    #[serde(default)]
    pub session_title: Option<String>,
    /// Where [`Self::session_title`] came from, so a resumed capture keeps ranking it.
    #[builder(default)]
    #[serde(default)]
    pub session_title_source: Option<TitleSource>,
    /// The title this row's own line set or cleared. Replayed on resume, so every source's
    /// title is known again and a cleared one can fall back to the next.
    #[builder(default)]
    #[serde(default)]
    pub title_change: Option<TitleChange>,
    /// The model call this row came from, unique within the harness and the same in every
    /// session a harness copies the row into. Groups the rows one response is split into, so
    /// their usage counts once.
    #[builder(default)]
    #[serde(default)]
    pub turn_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder)]
pub struct Session {
    pub handle: HarnessSession,
    #[builder(default)]
    pub parent: Option<HarnessSession>,
    #[builder(default)]
    pub cwd: Option<PathBuf>,
    #[builder(default)]
    pub git_branch: Option<String>,
    #[builder(default)]
    pub model: Option<String>,
    pub started_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    #[builder(default)]
    pub message_count: u64,
    /// Usage attributed to this session: each model call counted once across every session
    /// holding a copy of it, at the most its rows reported, and owned by the earliest-started
    /// session holding it that does not descend from another.
    pub usage: Usage,
    #[builder(default)]
    pub title: Option<String>,
    /// Where [`Self::title`] came from.
    #[builder(default)]
    pub title_source: Option<TitleSource>,
    #[builder(default)]
    pub preview: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SessionMatch {
    pub session: Session,
    pub title: HighlightedString,
    pub preview: HighlightedString,
    pub score: f64,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn arb_content() -> impl Strategy<Value = Content> {
        "[a-zA-Z0-9 ]{0,16}".prop_map(Content::Text)
    }

    fn arb_message() -> impl Strategy<Value = Message> {
        ("[a-z0-9]{1,8}", "[a-z0-9]{1,8}", proptest::collection::vec(arb_content(), 0..3)).prop_map(
            |(native_session, source_id, content)| {
                Message::builder()
                    .id(RecordId(atuin_common::utils::uuid_v7()))
                    .session(HarnessSession {
                        harness: HarnessKind::ClaudeCode,
                        session: NativeSessionId::from(native_session),
                    })
                    .source_id(SourceId::from(source_id))
                    .timestamp(OffsetDateTime::UNIX_EPOCH)
                    .role(Role::User)
                    .content(content)
                    .build()
            },
        )
    }

    /// Records are named-field msgpack, so a host whose build predates a field (here `turn_id`)
    /// still decodes a newer host's records, skipping the field it does not know.
    #[rstest]
    fn older_decoder_ignores_a_newer_field() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct OldMessage {
            id: RecordId,
            session: HarnessSession,
            source_id: SourceId,
            parent: Option<HarnessSession>,
            parent_source_id: Option<SourceId>,
            timestamp: OffsetDateTime,
            role: Role,
            content: Vec<Content>,
            cwd: Option<PathBuf>,
            git_branch: Option<String>,
            model: Option<String>,
            usage: Option<Usage>,
            stop_reason: Option<StopReason>,
            #[serde(default)]
            session_title: Option<String>,
        }
        let msg = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from("s".to_owned()),
            })
            .source_id(SourceId::from("x".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(vec![])
            .turn_id(Some("msg_1".to_owned()))
            .build();
        let record = crate::ai_session::AiSessionRecord::Message(msg).serialize();
        let old = rmp_serde::from_slice::<OldMessage>(&record[1..]);
        assert!(old.is_ok(), "older host cannot decode: {:?}", old.err());
    }

    #[rstest]
    fn harness_kind_covers_every_known_harness() {
        for harness in AnyHarness::all() {
            assert_ne!(HarnessKind::from(harness), HarnessKind::Unknown);
        }
    }

    proptest! {
        #[test]
        fn message_msgpack_roundtrips(m in arb_message()) {
            let bytes = rmp_serde::to_vec_named(&m).unwrap();
            let back: Message = rmp_serde::from_slice(&bytes).unwrap();
            prop_assert_eq!(m, back);
        }
    }
}
