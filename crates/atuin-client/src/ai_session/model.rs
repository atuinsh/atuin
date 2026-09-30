use std::path::PathBuf;

use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{
    Content, ParentKind, Role, StopReason, TitleChange, TitleSource, Usage,
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
    /// How this session relates to [`Self::parent`]. Absent in records written before it was
    /// captured, and whenever the harness did not say. A kind this build does not know (a newer
    /// build's) reads as absent, rather than failing the whole record.
    #[builder(default)]
    #[serde(default, deserialize_with = "known_parent_kind")]
    pub parent_kind: Option<ParentKind>,
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

/// [`Message::parent_kind`] as a record holds it: `None` for a kind this build does not know,
/// so a newer build can add kinds without its records failing to decode here, and for a value
/// that names no kind at all (some development builds' records hold another field there).
fn known_parent_kind<'de, D>(deserializer: D) -> Result<Option<ParentKind>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::IntoDeserializer;
    use serde::de::value::{Error, StringDeserializer};

    /// A kind's name, or anything else a record may hold there.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Name(String),
        Other(serde::de::IgnoredAny),
    }

    // Parse the name with `ParentKind`'s own `Deserialize`, so a new variant is known here
    // without listing it twice.
    Ok(match Option::<Raw>::deserialize(deserializer)? {
        Some(Raw::Name(name)) => {
            let name: StringDeserializer<Error> = name.into_deserializer();
            ParentKind::deserialize(name).ok()
        }
        Some(Raw::Other(_)) | None => None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TypedBuilder)]
pub struct Session {
    pub handle: HarnessSession,
    #[builder(default)]
    pub parent: Option<HarnessSession>,
    /// How this session relates to [`Self::parent`], when known.
    #[builder(default)]
    pub parent_kind: Option<ParentKind>,
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
    /// The session's most recent assistant reply, clipped: how it ended, or where it is now.
    #[builder(default)]
    pub last_reply: Option<String>,
    /// The host that captured the session (its first row's). `None` until known.
    #[builder(default)]
    pub host: Option<HostId>,
    /// The top-most stored ancestor this session is grouped under, following parent links (else
    /// [`Self::copy_of`]); `None` when it is a root itself. A session whose parent is not stored
    /// is a root until the parent arrives.
    #[builder(default)]
    pub root: Option<HarnessSession>,
    /// For a session with no parent, the session it was copied from, inferred from the model
    /// calls they share: the harness named none (a Claude Code `--fork-session`, or a `--resume`
    /// it turned into a fork). Always of the same harness.
    #[builder(default)]
    #[serde(default)]
    pub copy_of: Option<HarnessSession>,
    /// How many sessions are grouped under this one. Only counted by roots-only queries (see
    /// [`SessionFilter::roots_only`]); 0 elsewhere.
    #[builder(default)]
    pub child_count: u64,
    /// The newest `updated_at` across this session and the sessions grouped under it, which is
    /// what roots-only queries order by. Only set by roots-only queries.
    #[builder(default)]
    pub group_updated_at: Option<OffsetDateTime>,
}

/// How a session relates to the session it is grouped under: its [`ParentKind`] where capture
/// recorded one, else what its harness and id tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionRelation {
    /// No parent.
    Root,
    /// Spawned by its parent to do part of its work ([`ParentKind::Subagent`]).
    Subagent,
    /// Branches off its parent's conversation ([`ParentKind::Fork`]; a Claude Code
    /// `--fork-session`, a pi branch), or a copy of it (see [`Session::copy_of`]).
    Fork,
    /// Carries its parent's conversation on under a new session ([`ParentKind::Continuation`]),
    /// such as a Codex revert recorded as a session of its own, or a session carried on in
    /// another harness.
    Continuation,
    /// Has a parent, but no kind was recorded (a record from before capture recorded one) and
    /// the harness links subagents and forks alike (Codex, opencode).
    Child,
}

impl SessionRelation {
    /// Whether a session related so is one a person carried on: a root, fork or continuation.
    /// Subagents are fragments of their parent's work, and so are taken to be the children of
    /// unknown kind (most of Codex's and opencode's are spawned agents). A group's
    /// [`Session::child_count`] counts only these.
    #[must_use]
    pub fn carries_on(self) -> bool {
        matches!(self, Self::Root | Self::Fork | Self::Continuation)
    }
}

impl Session {
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

    /// How this session relates to its parent: its [`ParentKind`] when capture recorded one.
    ///
    /// A parent recorded without a kind (by a build from before kinds were captured) is told
    /// apart as well as the harness and id allow: another harness's session is only ever named
    /// by a continuation, and a Claude Code `agent-*` session is a subagent while its other
    /// children are forks, as are all of pi's; Codex and opencode link both alike
    /// ([`SessionRelation::Child`]).
    #[must_use]
    pub fn relation(&self) -> SessionRelation {
        let Some(parent) = &self.parent else {
            return match self.copy_of {
                Some(_) => SessionRelation::Fork,
                None => SessionRelation::Root,
            };
        };
        match self.parent_kind {
            Some(ParentKind::Subagent) => SessionRelation::Subagent,
            Some(ParentKind::Fork) => SessionRelation::Fork,
            Some(ParentKind::Continuation) => SessionRelation::Continuation,
            None if parent.harness != self.handle.harness => SessionRelation::Continuation,
            None => match self.handle.harness {
                HarnessKind::ClaudeCode if self.handle.session.as_ref().starts_with("agent-") => {
                    SessionRelation::Subagent
                }
                HarnessKind::ClaudeCode | HarnessKind::Pi => SessionRelation::Fork,
                _ => SessionRelation::Child,
            },
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
    /// Active at or after this time (its `updated_at`).
    pub updated_since: Option<OffsetDateTime>,
}

/// How a search query's terms match a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SearchTerms {
    /// Every term, each as a whole word (`app` finds `app`, not `apple`).
    #[default]
    All,
    /// [`Self::All`] for search as you type: the last term also matches as a prefix, unless
    /// the query ends in whitespace (see
    /// [`atuin_common::db::sqlite::fts::prefix_match_expression`]).
    Typed,
    /// Any term, each as a prefix: the fallback when no message holds every term (see
    /// [`atuin_common::db::sqlite::fts::match_any_expression`]).
    Any,
}

#[derive(Clone, Debug)]
pub struct SessionMatch {
    pub session: Session,
    /// `session`'s title, with the query's matches highlighted when the best-matching message is
    /// `session`'s own; plain when it is in [`Self::matched`].
    pub title: HighlightedString,
    /// A snippet of the best-matching message, with the query's matches highlighted.
    pub preview: HighlightedString,
    /// Position of the best-matching message within the session holding it (`session`, or
    /// [`Self::matched`]), in transcript order.
    pub message_index: u64,
    /// The session holding the best-matching message, when it is not `session`: with
    /// [`SessionFilter::roots_only`], a session grouped under the root returned.
    pub matched: Option<MatchedSession>,
    pub score: f64,
}

/// The session in a group that holds a [`SessionMatch`]'s best-matching message, when that is not
/// the group's root.
#[derive(Clone, Debug)]
pub struct MatchedSession {
    pub handle: HarnessSession,
    /// Its title, with the query's matches highlighted.
    pub title: HighlightedString,
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

    fn kinded(kind: Option<ParentKind>) -> Message {
        let session = HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("s".to_owned()),
        };
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from("x".to_owned()))
            .parent(Some(session))
            .parent_kind(kind)
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(vec![])
            .build()
    }

    /// Every kind round-trips through a record.
    #[rstest]
    #[case::none(None)]
    #[case::subagent(Some(ParentKind::Subagent))]
    #[case::fork(Some(ParentKind::Fork))]
    #[case::continuation(Some(ParentKind::Continuation))]
    fn a_parent_kind_round_trips(#[case] kind: Option<ParentKind>) {
        let record = crate::ai_session::AiSessionRecord::Message(kinded(kind)).serialize();
        let crate::ai_session::AiSessionRecord::Message(back) =
            crate::ai_session::AiSessionRecord::deserialize(&record).unwrap();
        assert_eq!(back.parent_kind, kind);
    }

    /// A kind this build does not know (a newer build's), or a value naming none, reads as no
    /// kind: the rest of the record still decodes.
    #[rstest]
    #[case::a_newer_kind("Handoff")]
    #[case::not_a_kind("784ad9be-9b3d-48e2-a3d7-a7cf227fd86e")]
    fn an_unknown_parent_kind_reads_as_none(#[case] kind: &str) {
        /// A record as a build that writes `kind` would.
        #[derive(Serialize)]
        struct Written<'a> {
            id: RecordId,
            session: HarnessSession,
            source_id: SourceId,
            parent_kind: &'a str,
            timestamp: OffsetDateTime,
            role: Role,
            content: Vec<Content>,
        }
        let written = Written {
            id: RecordId(atuin_common::utils::uuid_v7()),
            session: HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from("s".to_owned()),
            },
            source_id: SourceId::from("x".to_owned()),
            parent_kind: kind,
            timestamp: OffsetDateTime::UNIX_EPOCH,
            role: Role::User,
            content: vec![],
        };
        let body = rmp_serde::to_vec_named(&written).unwrap();
        let back: Message = rmp_serde::from_slice(&body).unwrap();
        assert_eq!(back.parent_kind, None);
        assert_eq!(back.source_id, written.source_id);
    }

    /// Without a recorded kind (records from before capture recorded one), the relation is told
    /// from the harness and id.
    #[rstest]
    #[case::no_parent(HarnessKind::ClaudeCode, "s", false, SessionRelation::Root)]
    #[case::claude_subagent(HarnessKind::ClaudeCode, "agent-a1", true, SessionRelation::Subagent)]
    #[case::claude_fork(HarnessKind::ClaudeCode, "0b3c", true, SessionRelation::Fork)]
    #[case::pi_branch(HarnessKind::Pi, "s", true, SessionRelation::Fork)]
    #[case::codex_child(HarnessKind::Codex, "s", true, SessionRelation::Child)]
    #[case::opencode_child(HarnessKind::Opencode, "ses_1", true, SessionRelation::Child)]
    fn without_a_kind_relation_follows_the_harness_and_id(
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

    /// A session carried on in another harness is a continuation of the one it continues, whatever its own harness calls its children, even in records from
    /// before capture recorded the kind.
    #[rstest]
    fn a_continuation_in_another_harness_is_a_continuation(
        #[values(HarnessKind::Codex, HarnessKind::Opencode, HarnessKind::ClaudeCode)]
        harness: HarnessKind,
        #[values(None, Some(ParentKind::Continuation))] kind: Option<ParentKind>,
    ) {
        let session = Session::builder()
            .handle(HarnessSession {
                harness,
                session: NativeSessionId::from("agent-new".to_owned()),
            })
            .parent(Some(HarnessSession {
                harness: HarnessKind::Pi,
                session: NativeSessionId::from("original".to_owned()),
            }))
            .parent_kind(kind)
            .started_at(OffsetDateTime::UNIX_EPOCH)
            .updated_at(OffsetDateTime::UNIX_EPOCH)
            .usage(Usage::default())
            .build();
        assert_eq!(session.relation(), SessionRelation::Continuation);
    }

    /// A recorded kind decides, whatever the harness and id suggest.
    #[rstest]
    #[case::codex_subagent(
        HarnessKind::Codex,
        "s",
        ParentKind::Subagent,
        SessionRelation::Subagent
    )]
    #[case::codex_fork(HarnessKind::Codex, "s", ParentKind::Fork, SessionRelation::Fork)]
    #[case::opencode_fork(HarnessKind::Opencode, "ses_1", ParentKind::Fork, SessionRelation::Fork)]
    #[case::claude_fork_named_like_an_agent(
        HarnessKind::ClaudeCode,
        "agent-a1",
        ParentKind::Fork,
        SessionRelation::Fork
    )]
    #[case::pi_continuation(
        HarnessKind::Pi,
        "s",
        ParentKind::Continuation,
        SessionRelation::Continuation
    )]
    fn a_recorded_kind_decides_the_relation(
        #[case] harness: HarnessKind,
        #[case] id: &str,
        #[case] kind: ParentKind,
        #[case] expected: SessionRelation,
    ) {
        let handle = |id: &str| HarnessSession {
            harness,
            session: NativeSessionId::from(id.to_owned()),
        };
        let session = Session::builder()
            .handle(handle(id))
            .parent(Some(handle("parent")))
            .parent_kind(Some(kind))
            .started_at(OffsetDateTime::UNIX_EPOCH)
            .updated_at(OffsetDateTime::UNIX_EPOCH)
            .usage(Usage::default())
            .build();
        assert_eq!(session.relation(), expected);
    }

    /// A parentless session copied from another (a Claude Code `--fork-session`) is its fork.
    #[rstest]
    fn a_copied_session_is_a_fork() {
        let handle = |id: &str| HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from(id.to_owned()),
        };
        let session = Session::builder()
            .handle(handle("copy"))
            .copy_of(Some(handle("original")))
            .started_at(OffsetDateTime::UNIX_EPOCH)
            .updated_at(OffsetDateTime::UNIX_EPOCH)
            .usage(Usage::default())
            .build();
        assert_eq!(session.relation(), SessionRelation::Fork);
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
