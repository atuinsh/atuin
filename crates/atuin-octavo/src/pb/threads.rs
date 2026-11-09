use std::fmt;
use std::ops::RangeInclusive;

use atuin_client::ai_session::{
    AtuinSessionId, HarnessSession as DomainHarnessSession, MAX_ROW_TOKENS, Message,
};
use atuin_common::harnesstools::Harness as _;
use atuin_common::harnesstools::session::{
    Change, Content, FilePatch, Hunk, ParentKind, Patch, Role, StopReason, ToolResult, ToolUse,
};
use atuin_common::string::{Bounded, NonNulStr, TooLong};
use atuin_domain::record::RecordId;
use prost::Message as _;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use super::content_block::{self, Block, file_patch};
use super::{
    CapturedThreadMessage, ContentBlock, HarnessSession, NotUuidV7, ThreadMessage,
    ThreadParentKind, UuidV7, thread_message,
};

/// The most one message may encode to, which holds the strings of its content to what Octavo
/// takes of them.
const MAX_MESSAGE_BYTES: usize = 1 << 20;
/// What a key Octavo indexes or matches on holds at most.
const MAX_ID_BYTES: usize = 512;
/// `google.protobuf.Timestamp`'s range, which Octavo stores whole.
const TIMESTAMP_SECONDS: RangeInclusive<i64> = -62_135_596_800..=253_402_300_799;

/// A string as Octavo takes it: of at most `MAX` bytes, with no NUL.
type Text<const MAX: usize> = Bounded<MAX, NonNulStr>;
type Title = Text<1024>;
/// A model, its provider or a tool's name.
type Name = Text<256>;
/// The label of a role or stop reason the wire has no value for.
type Label = Text<128>;
type Cwd = Text<4096>;
type Branch = Text<256>;
/// A key Octavo indexes or matches on: a [`Field`].
type Key = Text<MAX_ID_BYTES>;

/// Octavo's strings from what capture stores, without the NULs Octavo refuses in a string.
trait FromCaptured: Sized {
    /// `text` cut at a char boundary to fit: for text a reader reads, which a cut leaves readable.
    fn clipped(text: String) -> Self;

    /// `text` whole, if it fits: clipped, a path or key would name another one.
    fn whole(text: String) -> Result<Self, TooLong>;
}

impl<const MAX: usize> FromCaptured for Text<MAX> {
    fn clipped(text: String) -> Self {
        Self::clipping(NonNulStr::stripping(text).into())
    }

    fn whole(text: String) -> Result<Self, TooLong> {
        Self::new(NonNulStr::stripping(text).into())
    }
}

/// A key of a [`CapturedThreadMessage`], displayed as its path in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    ThreadSessionId,
    ParentSessionId,
    SourceId,
    ParentSourceId,
    TurnId,
    ToolCallId,
    ToolResultCallId,
    PatchCallId,
}

impl Field {
    /// `text` as this field's key, sent whole: clipped, it would name another line or thread.
    fn key(self, text: impl Into<String>) -> Result<Key, KeyError> {
        Key::whole(text.into())
            .map_err(|TooLong { len, .. }| KeyError::TooLong { field: self, len })
    }

    /// [`Self::key`], refused when empty: an empty key would name every line or thread.
    fn nonempty_key(self, text: impl Into<String>) -> Result<Key, KeyError> {
        let key = self.key(text)?;
        if key.is_empty() {
            return Err(KeyError::Empty { field: self });
        }

        Ok(key)
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ThreadSessionId => "thread.session_id",
            Self::ParentSessionId => "parent.session_id",
            Self::SourceId => "message.source_id",
            Self::ParentSourceId => "message.parent_source_id",
            Self::TurnId => "message.turn_id",
            Self::ToolCallId => "tool_call.id",
            Self::ToolResultCallId => "tool_result.call_id",
            Self::PatchCallId => "patch.call_id",
        })
    }
}

/// Why a key can't go to Octavo.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum KeyError {
    #[error("{field} is {len} bytes, over the {MAX_ID_BYTES} Octavo takes")]
    TooLong {
        field: Field,
        len: usize,
    },
    #[error("{field} is empty, so it names no thread or line")]
    Empty {
        field: Field,
    },
}

/// Why a [`Message`] can't be sent to Octavo.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThreadMessageConversionError {
    #[error("record id {0} is not a UUIDv7, so Octavo can't store it")]
    RecordId(RecordId),
    #[error("message {0} names no atuin session id")]
    NoAtuinId(RecordId),
    #[error("atuin session id {0} is not a UUIDv7")]
    AtuinId(AtuinSessionId),
    #[error("message {0} names no host")]
    NoHost(RecordId),
    #[error("message {0} is of no known harness")]
    UnknownHarness(RecordId),
    #[error("message {id}: {source}")]
    Key {
        id: RecordId,
        source: KeyError,
    },
    #[error("message {id} is {len} bytes encoded, over the {MAX_MESSAGE_BYTES} Octavo takes")]
    TooLarge {
        id: RecordId,
        len: usize,
    },
}

impl TryFrom<Message> for CapturedThreadMessage {
    type Error = ThreadMessageConversionError;

    fn try_from(message: Message) -> Result<Self, Self::Error> {
        // Destructured whole, so a field added to `Message` fails to compile here until it is
        // sent or dropped on purpose.
        let Message {
            id,
            session,
            source_id,
            parent,
            parent_kind,
            parent_source_id,
            timestamp,
            role,
            content,
            cwd,
            git_branch,
            model,
            usage,
            stop_reason,
            session_title,
            session_title_source: _,
            title_change: _,
            turn_id,
            atuin_id,
            host,
            provider,
        } = message;

        let at = |source| ThreadMessageConversionError::Key { id, source };
        let record_id = UuidV7::try_from(id)
            .map_err(|NotUuidV7(id)| ThreadMessageConversionError::RecordId(id))?;
        let atuin_id = atuin_id.ok_or(ThreadMessageConversionError::NoAtuinId(id))?;
        let atuin_uuid = UuidV7::try_from(Uuid::from(atuin_id))
            .map_err(|_| ThreadMessageConversionError::AtuinId(atuin_id))?;
        let host = host.ok_or(ThreadMessageConversionError::NoHost(id))?;
        let thread = HarnessSession::from_captured(Field::ThreadSessionId, session)
            .map_err(at)?
            .ok_or(ThreadMessageConversionError::UnknownHarness(id))?;
        if thread.session_id.is_empty() {
            return Err(at(KeyError::Empty {
                field: Field::ThreadSessionId,
            }));
        }
        // Octavo refuses a kind without a parent, and a parent of no known harness, with no
        // session id, or naming the thread itself. The line is worth more than its parent link,
        // so those go.
        let parent = parent
            .map(|parent| HarnessSession::from_captured(Field::ParentSessionId, parent))
            .transpose()
            .map_err(at)?
            .flatten()
            .filter(|parent| !parent.session_id.is_empty() && *parent != thread);
        let parent_kind = parent
            .as_ref()
            .and(parent_kind)
            .map_or(ThreadParentKind::Unspecified, ThreadParentKind::from);
        let Labelled {
            value: role,
            label: role_label,
        } = role.into();
        let Labelled {
            value: stop_reason,
            label: stop_reason_label,
        } = stop_reason.into();
        let content = content
            .into_iter()
            .map(ContentBlock::from_captured)
            .filter_map(Result::transpose)
            .collect::<Result<_, _>>()
            .map_err(at)?;

        let converted = Self {
            thread: Some(thread),
            client_session_id: Some(atuin_uuid),
            parent,
            parent_kind: parent_kind.into(),
            thread_title: session_title.map(|title| Title::clipped(title).into()),
            message: Some(ThreadMessage {
                id: Some(record_id),
                source_id: Field::SourceId.nonempty_key(source_id).map_err(at)?.into(),
                parent_source_id: parent_source_id
                    .map(|parent| Field::ParentSourceId.key(parent))
                    .transpose()
                    .map_err(at)?
                    .map(Into::into),
                timestamp: Some(prost_types::Timestamp {
                    seconds: timestamp
                        .unix_timestamp()
                        .clamp(*TIMESTAMP_SECONDS.start(), *TIMESTAMP_SECONDS.end()),
                    nanos: timestamp.nanosecond().cast_signed(),
                }),
                role: role.into(),
                role_label: role_label.map(Into::into),
                content,
                cwd: cwd
                    .and_then(|cwd| Cwd::whole(cwd.to_string_lossy().into_owned()).ok())
                    .map(Into::into),
                git_branch: git_branch
                    .and_then(|branch| Branch::whole(branch).ok())
                    .map(Into::into),
                model: model.map(|model| Name::clipped(model).into()),
                provider: provider.map(|provider| Name::clipped(provider).into()),
                // A bogus counter never costs the line.
                usage: usage.map(|usage| usage.map(|count| count.min(MAX_ROW_TOKENS))),
                stop_reason: stop_reason.into(),
                stop_reason_label: stop_reason_label.map(Into::into),
                turn_id: turn_id
                    .map(|turn| Field::TurnId.key(turn))
                    .transpose()
                    .map_err(at)?
                    .map(Into::into),
                host: Some(host.into()),
            }),
        };

        match converted.encoded_len() {
            len if len > MAX_MESSAGE_BYTES => {
                Err(ThreadMessageConversionError::TooLarge { id, len })
            }
            _ => Ok(converted),
        }
    }
}

impl HarnessSession {
    /// `session`, the key of `field`, as Octavo names a thread, or `None` for a session of no known
    /// harness.
    fn from_captured(
        field: Field,
        session: DomainHarnessSession,
    ) -> Result<Option<Self>, KeyError> {
        let Some(harness) = session.harness.harness() else {
            return Ok(None);
        };

        Ok(Some(Self {
            harness: harness.name().to_owned(),
            session_id: field.key(session.session)?.into(),
        }))
    }
}

impl From<ParentKind> for ThreadParentKind {
    fn from(kind: ParentKind) -> Self {
        match kind {
            ParentKind::Subagent => Self::Subagent,
            ParentKind::Fork => Self::Fork,
            ParentKind::Continuation => Self::Continuation,
        }
    }
}

/// A wire enum's value, and the label of one it has no value for: set exactly when `value` is
/// `Other`.
struct Labelled<E> {
    value: E,
    label: Option<Label>,
}

impl<E> Labelled<E> {
    fn known(value: E) -> Self {
        Self { value, label: None }
    }
}

impl From<Role> for Labelled<thread_message::Role> {
    fn from(role: Role) -> Self {
        use thread_message::Role as Wire;

        match role {
            Role::User => Self::known(Wire::User),
            Role::Assistant => Self::known(Wire::Assistant),
            Role::System => Self::known(Wire::System),
            Role::Tool => Self::known(Wire::Tool),
            Role::Other(label) => Self {
                value: Wire::Other,
                label: Some(Label::clipped(label)),
            },
        }
    }
}

impl From<Option<StopReason>> for Labelled<thread_message::StopReason> {
    fn from(reason: Option<StopReason>) -> Self {
        use thread_message::StopReason as Wire;

        let Some(reason) = reason else {
            return Self::known(Wire::Unspecified);
        };
        match reason {
            StopReason::EndTurn => Self::known(Wire::EndTurn),
            StopReason::MaxTokens => Self::known(Wire::MaxTokens),
            StopReason::ToolUse => Self::known(Wire::ToolUse),
            StopReason::StopSequence => Self::known(Wire::StopSequence),
            StopReason::Refusal => Self::known(Wire::Refusal),
            StopReason::Aborted => Self::known(Wire::Aborted),
            StopReason::Error => Self::known(Wire::Error),
            StopReason::Other(label) => Self {
                value: Wire::Other,
                label: Some(Label::clipped(label)),
            },
        }
    }
}

impl ContentBlock {
    /// `content` as a block, or `None` for one Octavo doesn't store.
    fn from_captured(content: Content) -> Result<Option<Self>, KeyError> {
        let block = match content {
            Content::Text(text) => Block::Text(NonNulStr::stripping(text).into()),
            // Capture keeps only that the model reasoned, and drops blocks atuin doesn't model, but
            // records stored before it did may hold either.
            Content::Reasoning(_) => {
                Block::ReasoningSummary(content_block::ReasoningSummary { tokens: None })
            }
            Content::Other(_) => return Ok(None),
            Content::ReasoningSummary { tokens } => {
                Block::ReasoningSummary(content_block::ReasoningSummary { tokens })
            }
            Content::ToolUse(ToolUse {
                id: call,
                name,
                input,
            }) => Block::ToolCall(content_block::ToolCall {
                id: Field::ToolCallId.key(call)?.into(),
                name: Name::clipped(name).into(),
                input: Payload::of(input).map(Into::into),
            }),
            Content::ToolResult(ToolResult {
                call,
                output,
                error,
            }) => Block::ToolResult(content_block::ToolResult {
                call_id: Field::ToolResultCallId.key(call)?.into(),
                output: Payload::of(output).map(Into::into),
                is_error: error,
            }),
            Content::Summary(text) => Block::Summary(NonNulStr::stripping(text).into()),
            Content::Error(text) => Block::Failure(NonNulStr::stripping(text).into()),
            Content::Patch(Patch { call, files }) => Block::Patch(content_block::Patch {
                call_id: Field::PatchCallId.key(call)?.into(),
                files: files.into_iter().map(content_block::FilePatch::from).collect(),
            }),
        };

        Ok(Some(Self { block: Some(block) }))
    }
}

impl From<FilePatch> for content_block::FilePatch {
    fn from(
        FilePatch {
            path,
            change,
            moved_to,
            hunks,
        }: FilePatch,
    ) -> Self {
        Self {
            path: NonNulStr::stripping(path).into(),
            change: file_patch::Change::from(change).into(),
            moved_to: moved_to.map(|moved_to| NonNulStr::stripping(moved_to).into()),
            hunks: hunks.into_iter().map(content_block::Hunk::from).collect(),
        }
    }
}

impl From<Change> for file_patch::Change {
    fn from(change: Change) -> Self {
        match change {
            Change::Add => Self::Add,
            Change::Update => Self::Update,
            Change::Delete => Self::Delete,
        }
    }
}

impl From<Hunk> for content_block::Hunk {
    fn from(
        Hunk {
            old_start,
            old_lines,
            new_start,
            new_lines,
            lines,
        }: Hunk,
    ) -> Self {
        Self {
            old_start,
            old_lines,
            new_start,
            new_lines,
            lines: lines.into_iter().map(|line| NonNulStr::stripping(line).into()).collect(),
        }
    }
}

/// A tool's input or output as JSON text, with no NUL in any key or string: serde would escape it
/// as `\u0000` for Octavo to refuse.
struct Payload(String);

impl Payload {
    /// `value`, or `None` for the `null` capture stores in place of a payload it didn't keep.
    fn of(value: Value) -> Option<Self> {
        (!value.is_null()).then(|| Self(Self::without_nul(value).to_string()))
    }

    fn without_nul(value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(NonNulStr::stripping(text).into()),
            Value::Array(items) => Value::Array(items.into_iter().map(Self::without_nul).collect()),
            Value::Object(members) => Value::Object(
                members
                    .into_iter()
                    .map(|(key, value)| {
                        (NonNulStr::stripping(key).into(), Self::without_nul(value))
                    })
                    .collect(),
            ),
            Value::Null | Value::Bool(_) | Value::Number(_) => value,
        }
    }
}

impl From<Payload> for String {
    fn from(Payload(json): Payload) -> Self {
        json
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use atuin_client::ai_session::{HarnessKind, NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::{ToolCallId, Usage};
    use atuin_domain::record::HostId as DomainHostId;
    use prost_types::{FileDescriptorProto, FileDescriptorSet, Timestamp};
    use rstest::{fixture, rstest};
    use serde_json::json;
    use time::macros::datetime;
    use uuid::Builder;

    use super::*;
    use crate::pb::{HostId, InsertThreadMessagesRequest, ThreadBatch};

    /// A UUIDv7 told apart from others by `n`.
    fn v7(n: u8) -> Uuid {
        Builder::from_unix_timestamp_millis(1_793_275_200_000, &[n; 10]).into_uuid()
    }

    /// [`v7`], as Octavo takes it.
    fn wire_v7(n: u8) -> UuidV7 {
        UuidV7::try_from(v7(n)).expect("v7 makes UUIDv7s")
    }

    /// A UUID of another version than 7.
    fn v4() -> Uuid {
        Builder::from_random_bytes([7; 16]).into_uuid()
    }

    fn handle(harness: HarnessKind, session: &str) -> DomainHarnessSession {
        DomainHarnessSession {
            harness,
            session: NativeSessionId::from(session.to_owned()),
        }
    }

    /// A line as capture stores it.
    fn message() -> Message {
        Message::builder()
            .id(RecordId(v7(1)))
            .session(handle(HarnessKind::ClaudeCode, "session"))
            .source_id(SourceId::from("line".to_owned()))
            .timestamp(datetime!(2026-10-29 12:00:00.5 UTC))
            .role(Role::User)
            .content(vec![Content::Text("hi".to_owned())])
            .atuin_id(Some(AtuinSessionId::from(v7(2))))
            .host(Some(DomainHostId(v7(3))))
            .build()
    }

    /// [`message`], as Octavo takes it.
    fn sent() -> CapturedThreadMessage {
        CapturedThreadMessage {
            thread: Some(HarnessSession {
                harness: "claude-code".to_owned(),
                session_id: "session".to_owned(),
            }),
            client_session_id: Some(wire_v7(2)),
            parent: None,
            parent_kind: ThreadParentKind::Unspecified.into(),
            thread_title: None,
            message: Some(ThreadMessage {
                id: Some(wire_v7(1)),
                source_id: "line".to_owned(),
                parent_source_id: None,
                timestamp: Some(Timestamp {
                    seconds: 1_793_275_200,
                    nanos: 500_000_000,
                }),
                role: thread_message::Role::User.into(),
                role_label: None,
                content: vec![ContentBlock {
                    block: Some(Block::Text("hi".to_owned())),
                }],
                cwd: None,
                git_branch: None,
                model: None,
                provider: None,
                usage: None,
                stop_reason: thread_message::StopReason::Unspecified.into(),
                stop_reason_label: None,
                turn_id: None,
                host: Some(HostId {
                    value: v7(3).into_bytes().to_vec(),
                }),
            }),
        }
    }

    fn line(sent: &mut CapturedThreadMessage) -> &mut ThreadMessage {
        sent.message.as_mut().expect("`sent` holds a message")
    }

    fn converted(message: Message) -> CapturedThreadMessage {
        CapturedThreadMessage::try_from(message).expect("the message converts")
    }

    #[rstest]
    #[case::as_captured(|_: &mut Message| {}, |_: &mut CapturedThreadMessage| {})]
    #[case::every_optional_field(
        |m: &mut Message| {
            m.parent_source_id = Some(SourceId::from("previous".to_owned()));
            m.cwd = Some(PathBuf::from("/src/atuin"));
            m.git_branch = Some("main".to_owned());
            m.model = Some("claude-opus".to_owned());
            m.provider = Some("anthropic".to_owned());
            m.usage = Some(Usage { input: Some(3), output: Some(5), ..Usage::default() });
            m.turn_id = Some("turn".to_owned());
            m.session_title = Some("Fix the build".to_owned());
        },
        |s: &mut CapturedThreadMessage| {
            s.thread_title = Some("Fix the build".to_owned());
            let line = line(s);
            line.parent_source_id = Some("previous".to_owned());
            line.cwd = Some("/src/atuin".to_owned());
            line.git_branch = Some("main".to_owned());
            line.model = Some("claude-opus".to_owned());
            line.provider = Some("anthropic".to_owned());
            line.usage = Some(Usage { input: Some(3), output: Some(5), ..Usage::default() });
            line.turn_id = Some("turn".to_owned());
        },
    )]
    #[case::nul_in_strings(
        |m: &mut Message| {
            m.source_id = SourceId::from("li\0ne".to_owned());
            m.content = vec![Content::Text("h\0i".to_owned())];
        },
        |_: &mut CapturedThreadMessage| {},
    )]
    #[case::parent_kept(
        |m: &mut Message| {
            m.parent = Some(handle(HarnessKind::Codex, "parent"));
            m.parent_kind = Some(ParentKind::Fork);
        },
        |s: &mut CapturedThreadMessage| {
            s.parent = Some(HarnessSession {
                harness: "codex".to_owned(),
                session_id: "parent".to_owned(),
            });
            s.parent_kind = ThreadParentKind::Fork.into();
        },
    )]
    #[case::parent_kind_without_a_parent(
        |m: &mut Message| m.parent_kind = Some(ParentKind::Subagent),
        |_: &mut CapturedThreadMessage| {},
    )]
    #[case::parent_naming_its_own_thread(
        |m: &mut Message| {
            m.parent = Some(m.session.clone());
            m.parent_kind = Some(ParentKind::Subagent);
        },
        |_: &mut CapturedThreadMessage| {},
    )]
    #[case::parent_of_no_known_harness(
        |m: &mut Message| {
            m.parent = Some(handle(HarnessKind::Unknown, "parent"));
            m.parent_kind = Some(ParentKind::Subagent);
        },
        |_: &mut CapturedThreadMessage| {},
    )]
    #[case::parent_with_no_session_id_but_nul(
        |m: &mut Message| {
            m.parent = Some(handle(HarnessKind::Codex, "\0"));
            m.parent_kind = Some(ParentKind::Fork);
        },
        |_: &mut CapturedThreadMessage| {},
    )]
    #[case::long_title_clipped_at_a_char_boundary(
        |m: &mut Message| m.session_title = Some(format!("a{}", "é".repeat(600))),
        |s: &mut CapturedThreadMessage| s.thread_title = Some(format!("a{}", "é".repeat(511))),
    )]
    #[case::long_branch_left_out(
        |m: &mut Message| m.git_branch = Some("b".repeat(Branch::MAX + 1)),
        |_: &mut CapturedThreadMessage| {},
    )]
    #[case::over_long_count_clamped(
        |m: &mut Message| m.usage = Some(Usage { output: Some(u64::MAX), ..Usage::default() }),
        |s: &mut CapturedThreadMessage| {
            line(s).usage = Some(Usage { output: Some(MAX_ROW_TOKENS), ..Usage::default() });
        },
    )]
    #[case::before_year_one_clamped(
        |m: &mut Message| m.timestamp = Message::EARLIEST,
        |s: &mut CapturedThreadMessage| {
            line(s).timestamp = Some(Timestamp { seconds: *TIMESTAMP_SECONDS.start(), nanos: 0 });
        },
    )]
    #[case::past_year_9999_clamped(
        |m: &mut Message| m.timestamp = datetime!(9999-12-31 23:59:59 -23:59),
        |s: &mut CapturedThreadMessage| {
            line(s).timestamp = Some(Timestamp { seconds: *TIMESTAMP_SECONDS.end(), nanos: 0 });
        },
    )]
    fn sends_what_octavo_takes_of_a_message(
        #[case] edit: fn(&mut Message),
        #[case] expect: fn(&mut CapturedThreadMessage),
    ) {
        let mut message = message();
        edit(&mut message);
        let mut want = sent();
        expect(&mut want);

        assert_eq!(converted(message), want);
    }

    #[rstest]
    #[case::user(Role::User, thread_message::Role::User, None)]
    #[case::assistant(Role::Assistant, thread_message::Role::Assistant, None)]
    #[case::system(Role::System, thread_message::Role::System, None)]
    #[case::tool(Role::Tool, thread_message::Role::Tool, None)]
    #[case::other(Role::Other("developer".to_owned()), thread_message::Role::Other, Some("developer"))]
    fn maps_each_role(
        #[case] role: Role,
        #[case] wire: thread_message::Role,
        #[case] label: Option<&str>,
    ) {
        let line = converted(Message { role, ..message() }).message.expect("a message");

        assert_eq!((line.role(), line.role_label.as_deref()), (wire, label));
    }

    #[rstest]
    #[case::unreported(None, thread_message::StopReason::Unspecified, None)]
    #[case::end_turn(Some(StopReason::EndTurn), thread_message::StopReason::EndTurn, None)]
    #[case::max_tokens(Some(StopReason::MaxTokens), thread_message::StopReason::MaxTokens, None)]
    #[case::tool_use(Some(StopReason::ToolUse), thread_message::StopReason::ToolUse, None)]
    #[case::stop_sequence(
        Some(StopReason::StopSequence),
        thread_message::StopReason::StopSequence,
        None
    )]
    #[case::refusal(Some(StopReason::Refusal), thread_message::StopReason::Refusal, None)]
    #[case::aborted(Some(StopReason::Aborted), thread_message::StopReason::Aborted, None)]
    #[case::error(Some(StopReason::Error), thread_message::StopReason::Error, None)]
    #[case::other(
        Some(StopReason::Other("pause_turn".to_owned())),
        thread_message::StopReason::Other,
        Some("pause_turn")
    )]
    fn maps_each_stop_reason(
        #[case] reason: Option<StopReason>,
        #[case] wire: thread_message::StopReason,
        #[case] label: Option<&str>,
    ) {
        let line = converted(Message {
            stop_reason: reason,
            ..message()
        })
        .message
        .expect("a message");

        assert_eq!((line.stop_reason(), line.stop_reason_label.as_deref()), (wire, label));
    }

    #[rstest]
    #[case::text(Content::Text("hi".to_owned()), Some(Block::Text("hi".to_owned())))]
    #[case::reasoning_to_a_summary(
        Content::Reasoning("let me think".to_owned()),
        Some(Block::ReasoningSummary(content_block::ReasoningSummary { tokens: None })),
    )]
    #[case::reasoning_summary(
        Content::ReasoningSummary { tokens: Some(12) },
        Some(Block::ReasoningSummary(content_block::ReasoningSummary { tokens: Some(12) })),
    )]
    #[case::tool_call(
        Content::ToolUse(ToolUse {
            id: ToolCallId::from("call".to_owned()),
            name: "Bash".to_owned(),
            input: json!({"command": "ls"}),
        }),
        Some(Block::ToolCall(content_block::ToolCall {
            id: "call".to_owned(),
            name: "Bash".to_owned(),
            input: Some(r#"{"command":"ls"}"#.to_owned()),
        })),
    )]
    #[case::tool_call_whose_input_capture_kept_none(
        Content::ToolUse(ToolUse {
            id: ToolCallId::from("call".to_owned()),
            name: "Bash".to_owned(),
            input: Value::Null,
        }),
        Some(Block::ToolCall(content_block::ToolCall {
            id: "call".to_owned(),
            name: "Bash".to_owned(),
            input: None,
        })),
    )]
    #[case::nul_in_a_json_key_and_leaf(
        Content::ToolUse(ToolUse {
            id: ToolCallId::from("call".to_owned()),
            name: "Bash".to_owned(),
            input: json!({"com\u{0}mand": ["l\u{0}s", 1]}),
        }),
        Some(Block::ToolCall(content_block::ToolCall {
            id: "call".to_owned(),
            name: "Bash".to_owned(),
            input: Some(r#"{"command":["ls",1]}"#.to_owned()),
        })),
    )]
    #[case::tool_result(
        Content::ToolResult(ToolResult {
            call: ToolCallId::from("call".to_owned()),
            output: json!("Cargo.toml"),
            error: true,
        }),
        Some(Block::ToolResult(content_block::ToolResult {
            call_id: "call".to_owned(),
            output: Some(r#""Cargo.toml""#.to_owned()),
            is_error: true,
        })),
    )]
    #[case::summary(Content::Summary("so far".to_owned()), Some(Block::Summary("so far".to_owned())))]
    #[case::error_to_a_failure(
        Content::Error("overloaded".to_owned()),
        Some(Block::Failure("overloaded".to_owned())),
    )]
    #[case::patch(
        Content::Patch(Patch {
            call: ToolCallId::from("call".to_owned()),
            files: vec![FilePatch {
                path: "src/lib.rs".to_owned(),
                change: Change::Update,
                moved_to: Some("src/main.rs".to_owned()),
                hunks: vec![Hunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 1,
                    lines: vec!["-old".to_owned(), "+new".to_owned()],
                }],
            }],
        }),
        Some(Block::Patch(content_block::Patch {
            call_id: "call".to_owned(),
            files: vec![content_block::FilePatch {
                path: "src/lib.rs".to_owned(),
                change: file_patch::Change::Update.into(),
                moved_to: Some("src/main.rs".to_owned()),
                hunks: vec![content_block::Hunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 1,
                    lines: vec!["-old".to_owned(), "+new".to_owned()],
                }],
            }],
        })),
    )]
    #[case::other_dropped(Content::Other(json!({"type": "image"})), None)]
    fn maps_each_content_block(#[case] content: Content, #[case] block: Option<Block>) {
        let line = converted(Message {
            content: vec![content],
            ..message()
        })
        .message
        .expect("a message");

        assert_eq!(
            line.content,
            Vec::from_iter(block.map(|block| ContentBlock { block: Some(block) }))
        );
    }

    #[rstest]
    #[case::unknown_harness(
        |m: &mut Message| m.session.harness = HarnessKind::Unknown,
        ThreadMessageConversionError::UnknownHarness(RecordId(v7(1))),
    )]
    #[case::record_id_not_v7(
        |m: &mut Message| m.id = RecordId(v4()),
        ThreadMessageConversionError::RecordId(RecordId(v4())),
    )]
    #[case::no_atuin_id(
        |m: &mut Message| m.atuin_id = None,
        ThreadMessageConversionError::NoAtuinId(RecordId(v7(1))),
    )]
    #[case::atuin_id_not_v7(
        |m: &mut Message| m.atuin_id = Some(AtuinSessionId::from(v4())),
        ThreadMessageConversionError::AtuinId(AtuinSessionId::from(v4())),
    )]
    #[case::no_host(
        |m: &mut Message| m.host = None,
        ThreadMessageConversionError::NoHost(RecordId(v7(1))),
    )]
    #[case::long_source_id(
        |m: &mut Message| m.source_id = SourceId::from("s".repeat(MAX_ID_BYTES + 1)),
        ThreadMessageConversionError::Key {
            id: RecordId(v7(1)),
            source: KeyError::TooLong { field: Field::SourceId, len: MAX_ID_BYTES + 1 },
        },
    )]
    #[case::session_id_of_nul(
        |m: &mut Message| m.session = handle(HarnessKind::ClaudeCode, "\0"),
        ThreadMessageConversionError::Key {
            id: RecordId(v7(1)),
            source: KeyError::Empty { field: Field::ThreadSessionId },
        },
    )]
    #[case::empty_source_id(
        |m: &mut Message| m.source_id = SourceId::from(String::new()),
        ThreadMessageConversionError::Key {
            id: RecordId(v7(1)),
            source: KeyError::Empty { field: Field::SourceId },
        },
    )]
    #[case::long_tool_call_id(
        |m: &mut Message| m.content = vec![Content::ToolUse(ToolUse {
            id: ToolCallId::from("c".repeat(MAX_ID_BYTES + 1)),
            name: "Bash".to_owned(),
            input: Value::Null,
        })],
        ThreadMessageConversionError::Key {
            id: RecordId(v7(1)),
            source: KeyError::TooLong { field: Field::ToolCallId, len: MAX_ID_BYTES + 1 },
        },
    )]
    fn refuses(#[case] edit: fn(&mut Message), #[case] error: ThreadMessageConversionError) {
        let mut message = message();
        edit(&mut message);

        assert_eq!(CapturedThreadMessage::try_from(message), Err(error));
    }

    #[rstest]
    fn refuses_a_message_too_large_for_octavo() {
        let message = Message {
            content: vec![Content::Text("x".repeat(MAX_MESSAGE_BYTES))],
            ..message()
        };

        let refused = CapturedThreadMessage::try_from(message);

        assert!(
            matches!(
                refused,
                Err(ThreadMessageConversionError::TooLarge { id, len })
                    if id == RecordId(v7(1)) && len > MAX_MESSAGE_BYTES
            ),
            "{refused:?}"
        );
    }

    /// A message that adds exactly `len` bytes to the request holding it, all but tags and
    /// lengths its title.
    fn message_framed_to(len: usize) -> CapturedThreadMessage {
        let delimited = |len: usize| 1 + prost::length_delimiter_len(len) + len;
        let title = (0..len)
            .rev()
            .find(|&title| delimited(delimited(title)) == len)
            .expect("a title frames to it");
        CapturedThreadMessage {
            thread_title: Some("t".repeat(title)),
            ..CapturedThreadMessage::default()
        }
    }

    const SMALL: usize = 16;
    const QUARTER: usize = ThreadBatch::MAX_BYTES / 4;
    const MESSAGES: usize = ThreadBatch::MAX_ITEMS.get() as usize;

    #[rstest]
    #[case::count_at_the_cap(vec![SMALL; MESSAGES], vec![MESSAGES])]
    #[case::count_past_the_cap(vec![SMALL; MESSAGES + 1], vec![MESSAGES, 1])]
    #[case::bytes_at_the_cap(vec![QUARTER; 4], vec![4])]
    #[case::bytes_past_the_cap(vec![QUARTER, QUARTER, QUARTER, QUARTER + 1], vec![3, 1])]
    #[case::message_past_the_byte_cap_alone(vec![SMALL, ThreadBatch::MAX_BYTES + 1, SMALL], vec![1, 1, 1])]
    fn thread_batches_cut_at_the_count_and_byte_caps(
        #[case] lens: Vec<usize>,
        #[case] batch_lens: Vec<usize>,
    ) {
        let messages: Vec<_> = lens.into_iter().map(message_framed_to).collect();

        let batches = ThreadBatch::batches(messages.clone());

        assert_eq!(batches.iter().map(|batch| batch.items().len()).collect::<Vec<_>>(), batch_lens);
        assert_eq!(
            batches.iter().flat_map(ThreadBatch::items).cloned().collect::<Vec<_>>(),
            messages
        );
        assert!(
            batches.iter().filter(|batch| batch.items().len() > 1).all(|batch| {
                InsertThreadMessagesRequest::from(batch.clone()).encoded_len()
                    <= ThreadBatch::MAX_BYTES
            }),
            "a request of more than one message encodes past {} bytes",
            ThreadBatch::MAX_BYTES
        );
    }

    /// `threads.proto`, as build.rs compiled it.
    #[fixture]
    fn threads_proto() -> FileDescriptorProto {
        FileDescriptorSet::decode(
            include_bytes!(concat!(env!("OUT_DIR"), "/hub_service.bin")).as_slice(),
        )
        .expect("build.rs writes an encoded descriptor set")
        .file
        .into_iter()
        .find(|file| file.name() == "atuin/api/v1/threads.proto")
        .expect("hub_service.proto imports threads.proto")
    }

    /// `atuin.api.v1.TokenUsage` is extern-pathed to [`Usage`], whose prost tags are written by
    /// hand.
    #[rstest]
    #[case::input(Usage { input: Some(7), ..Usage::default() }, "input")]
    #[case::output(Usage { output: Some(7), ..Usage::default() }, "output")]
    #[case::cache_read(Usage { cache_read: Some(7), ..Usage::default() }, "cache_read")]
    #[case::cache_write(Usage { cache_write: Some(7), ..Usage::default() }, "cache_write")]
    #[case::reasoning(Usage { reasoning: Some(7), ..Usage::default() }, "reasoning")]
    fn usage_encodes_each_count_under_its_proto_number(
        threads_proto: FileDescriptorProto,
        #[case] usage: Usage,
        #[case] field: &str,
    ) {
        let number = threads_proto
            .message_type
            .iter()
            .find(|message| message.name() == "TokenUsage")
            .and_then(|message| message.field.iter().find(|f| f.name() == field))
            .expect("TokenUsage has the field")
            .number();

        assert_eq!(usage.encode_to_vec(), [
            u8::try_from(number << 3).expect("tags 1-5 fit a byte"),
            7
        ]);
    }
}
