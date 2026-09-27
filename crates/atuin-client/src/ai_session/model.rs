use std::path::PathBuf;

use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};
use atuin_common::harnesstools::session::{
    Content, Role, StopReason, TitleChange, TitleSource, Usage,
};
use atuin_common::string::highlighted::HighlightedString;
use atuin_domain::record::{HostId, RecordId};
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

impl HarnessKind {
    /// The harness tools for this kind. `None` for a kind atuin has none for (Copilot, and
    /// sessions of no known harness), whose sessions can be viewed but not resumed.
    #[must_use]
    pub fn harness(self) -> Option<AnyHarness> {
        use atuin_common::harnesstools::{ccode, codex, opencode, pi};
        match self {
            Self::ClaudeCode => Some(AnyHarness::ClaudeCode(ccode::Ccode)),
            Self::Codex => Some(AnyHarness::Codex(codex::Codex)),
            Self::Opencode => Some(AnyHarness::Opencode(opencode::Opencode)),
            Self::Pi => Some(AnyHarness::Pi(pi::Pi)),
            Self::Copilot | Self::Unknown => None,
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
    /// The host that captured this row. Never part of the record body: the record envelope
    /// already carries it, so a reproject takes it from there and live capture from the local
    /// host. `None` for a row stored before hosts were tracked, until a reproject fills it in.
    #[builder(default)]
    #[serde(skip)]
    pub host: Option<HostId>,
}

impl From<Message> for RehydrateMessage {
    fn from(m: Message) -> Self {
        Self {
            source_id: m.source_id.into(),
            parent_source_id: m.parent_source_id.map(Into::into),
            timestamp: m.timestamp,
            role: m.role,
            content: m.content,
            model: m.model,
            usage: m.usage,
            stop_reason: m.stop_reason,
            turn_id: m.turn_id,
            cwd: m.cwd,
            git_branch: m.git_branch,
        }
    }
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
    /// The host that captured the session (its first row's). `None` until known.
    #[builder(default)]
    pub host: Option<HostId>,
    /// The top-most stored ancestor this session is grouped under, following parent links;
    /// `None` when it is a root itself. A session whose parent is not stored is a root until the
    /// parent arrives.
    #[builder(default)]
    pub root: Option<HarnessSession>,
    /// How many sessions are grouped under this one. Only counted by roots-only queries (see
    /// [`SessionFilter::roots_only`]); 0 elsewhere.
    #[builder(default)]
    pub child_count: u64,
    /// The newest `updated_at` across this session and the sessions grouped under it, which is
    /// what roots-only queries order by. Only set by roots-only queries.
    #[builder(default)]
    pub group_updated_at: Option<OffsetDateTime>,
}

/// How a session relates to its parent, as far as its identity tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionRelation {
    /// No parent.
    Root,
    /// Spawned by its parent to do part of its work (a Claude Code `agent-*` transcript).
    Subagent,
    /// Continues or branches off its parent's conversation (Claude Code `--resume`/`--fork`,
    /// Pi branches).
    Fork,
    /// Has a parent, but the harness does not say which kind of child it is: Codex and opencode
    /// link subagents and forks alike.
    Child,
}

impl Session {
    /// This session and its `messages` (in transcript order), as its harness can write them back
    /// out to be resumed in `cwd` (see [`atuin_common::harnesstools::Harness::rehydrate`]).
    #[must_use]
    pub fn rehydrate(
        &self,
        messages: impl IntoIterator<Item = Message>,
        cwd: PathBuf,
    ) -> RehydrateSession {
        RehydrateSession {
            id: self.handle.session.to_string(),
            title: self.title.clone(),
            cwd,
            original_cwd: self.cwd.clone(),
            git_branch: self.git_branch.clone(),
            model: self.model.clone(),
            started_at: self.started_at,
            messages: messages.into_iter().map(RehydrateMessage::from).collect(),
        }
    }

    /// Whether this session is a root: no stored ancestor.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.root.is_none()
    }

    /// The session this one is grouped under: its [root](Self::root), else itself.
    #[must_use]
    pub fn group(&self) -> &HarnessSession {
        self.root.as_ref().unwrap_or(&self.handle)
    }

    /// How this session relates to its parent, from its harness and id alone.
    #[must_use]
    pub fn relation(&self) -> SessionRelation {
        if self.parent.is_none() {
            return SessionRelation::Root;
        }
        match self.handle.harness {
            HarnessKind::ClaudeCode if self.handle.session.as_ref().starts_with("agent-") => {
                SessionRelation::Subagent
            }
            HarnessKind::ClaudeCode | HarnessKind::Pi => SessionRelation::Fork,
            _ => SessionRelation::Child,
        }
    }
}

/// Which sessions a listing or search returns. An absent field is not a filter; every present
/// one must hold.
///
/// Filters apply to each session on its own. With [`Self::roots_only`], a group is returned
/// (as its root) when any of its sessions passes, so a subagent's model or a fork's branch still
/// finds the group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Captured on this host.
    pub host: Option<HostId>,
    /// Working directory at or under this path (a workspace or git root).
    pub workspace: Option<PathBuf>,
    /// Working directory exactly this path.
    pub directory: Option<PathBuf>,
    /// On this git branch, exactly.
    pub branch: Option<String>,
    pub harness: Option<HarnessKind>,
    /// Model name containing this, ignoring ASCII case (`opus` finds `claude-opus-4-5`).
    pub model: Option<String>,
    /// Return only roots, each carrying its [`Session::child_count`], with the sessions grouped
    /// under it counting toward it: in a search, a child's match is its root's.
    pub roots_only: bool,
}

/// The hosts a host filter by name keeps sessions from: a name can stand for several host ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostSet {
    /// Captured on one of these. None at all keeps nothing (but see [`Self::unrecorded`]).
    pub ids: Vec<HostId>,
    /// Also keep sessions with no recorded host, captured before hosts were tracked and not yet
    /// backfilled. Those can only be this host's, so set it when the set holds this host.
    pub unrecorded: bool,
}

/// What a session's preview shows, read without the content of every message (see
/// [`crate::ai_session::AiSessionDatabase::preview_parts`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewParts {
    /// When each message was sent, and by whom, oldest first.
    pub activity: Vec<(OffsetDateTime, Role)>,
    /// The content of the first user message.
    pub first_user: Option<Vec<Content>>,
    /// The content of the last assistant message with conversation text (text or a summary),
    /// skipping those holding only tool calls or reasoning.
    pub last_assistant: Option<Vec<Content>>,
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

    /// Every kind with harness tools maps back to itself; the rest have none to resume with.
    #[rstest]
    #[case::claude(HarnessKind::ClaudeCode, true)]
    #[case::codex(HarnessKind::Codex, true)]
    #[case::opencode(HarnessKind::Opencode, true)]
    #[case::pi(HarnessKind::Pi, true)]
    #[case::copilot(HarnessKind::Copilot, false)]
    #[case::unknown(HarnessKind::Unknown, false)]
    fn a_kind_has_harness_tools_only_when_atuin_knows_the_harness(
        #[case] kind: HarnessKind,
        #[case] resumable: bool,
    ) {
        let harness = kind.harness();
        assert_eq!(harness.is_some(), resumable);
        if let Some(harness) = harness {
            assert_eq!(HarnessKind::from(&harness), kind);
        }
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
    #[case::no_parent(HarnessKind::ClaudeCode, "s", false, SessionRelation::Root)]
    #[case::claude_subagent(HarnessKind::ClaudeCode, "agent-a1", true, SessionRelation::Subagent)]
    #[case::claude_fork(HarnessKind::ClaudeCode, "0b3c", true, SessionRelation::Fork)]
    #[case::pi_branch(HarnessKind::Pi, "s", true, SessionRelation::Fork)]
    #[case::codex_child(HarnessKind::Codex, "s", true, SessionRelation::Child)]
    #[case::opencode_child(HarnessKind::Opencode, "ses_1", true, SessionRelation::Child)]
    fn relation_follows_the_harness_and_id(
        #[case] harness: HarnessKind,
        #[case] id: &str,
        #[case] has_parent: bool,
        #[case] expected: SessionRelation,
    ) {
        let handle = |id: &str| HarnessSession {
            harness,
            session: NativeSessionId::from(id.to_owned()),
        };
        let session = Session::builder()
            .handle(handle(id))
            .parent(has_parent.then(|| handle("parent")))
            .started_at(OffsetDateTime::UNIX_EPOCH)
            .updated_at(OffsetDateTime::UNIX_EPOCH)
            .usage(Usage::default())
            .build();
        assert_eq!(session.relation(), expected);
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
