//! Messages an agent hands atuin itself (`atuin threads push`), rather than capture reading them
//! from its transcript: one JSON object per line, in a shape of atuin's own that is the same for
//! every agent. A plugin or hook converts its agent's messages to it.
//!
//! ```json
//! {"v":1,"id":"m2","parent":"m1","role":"assistant","time":"2026-10-16T10:00:04Z","model":"m",
//!  "content":[{"type":"text","text":"Running the tests."},
//!             {"type":"tool_use","id":"t1","name":"Bash","input":{"cmd":"cargo test"}}]}
//! ```
//!
//! Every field but `v` (always 1, so a later shape can be told apart) and `role` may be left
//! out. A line with no `id` is keyed on its content, as
//! capture keys a Codex line, so it needs a `time` that stays the same each time it is pushed. A
//! block of a type this does not know is kept as it is ([`Content::Other`]); an unknown field
//! anywhere else is an error.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;
use time::OffsetDateTime;

use super::{
    Change, Content, FilePatch, Hunk, Message, MessageId, ParentKind, Patch, Role, SessionId,
    StopReason, TitleChange, TitleSource, ToolCallId, ToolResult, ToolUse, Usage,
};

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushedMessage {
    pub v: Version,
    pub id: Option<String>,
    /// The `id` of the message this one follows.
    pub parent: Option<String>,
    /// `user`, `assistant`, `system`, `tool`, or the agent's own name for another.
    pub role: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub time: Option<OffsetDateTime>,
    #[serde(default)]
    pub content: Vec<Block>,
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub model: Option<String>,
    /// Who served `model`, when the agent says (`openai-codex`, `anthropic`).
    pub provider: Option<String>,
    /// The model call this message came from, so the usage of messages one call is split into
    /// counts once.
    pub turn: Option<String>,
    pub usage: Option<Usage>,
    /// `end_turn`, `max_tokens`, `tool_use`, `stop_sequence`, `refusal`, `aborted`, `error`, or
    /// the agent's own.
    pub stop: Option<String>,
    pub title: Option<Title>,
    /// The session this one was spawned or forked from.
    pub parent_session: Option<String>,
    pub parent_kind: Option<Kind>,
}

/// The version of the shape a line is in. Only 1 so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct Version;

impl TryFrom<u64> for Version {
    type Error = String;

    fn try_from(v: u64) -> Result<Self, Self::Error> {
        match v {
            1 => Ok(Self),
            v => Err(format!("unknown version {v}: this atuin reads version 1")),
        }
    }
}

impl From<Version> for u64 {
    fn from(_: Version) -> Self {
        1
    }
}

/// A title the message gives its session; `text` of `None` clears the one `source` gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    pub text: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Summary,
    Generated,
    Named,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Subagent,
    Fork,
    Continuation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Block {
    Known(Known),
    /// A block of a type not listed in [`Known`], kept as it is.
    Other(Value),
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Known {
    Text {
        text: String,
    },
    /// Without `text`, reasoning whose text the agent could not read, maybe with how many tokens
    /// it took.
    Reasoning {
        text: Option<String>,
        tokens: Option<u64>,
    },
    /// A summary standing in for the conversation before it (a compaction).
    Summary {
        text: String,
    },
    /// Why a model call failed.
    Error {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        id: String,
        #[serde(default)]
        output: Value,
        #[serde(default)]
        error: bool,
    },
    /// The files a tool call changed.
    Patch {
        id: String,
        files: Vec<PatchFile>,
    },
}

const KNOWN: &[&str] =
    &["text", "reasoning", "summary", "error", "tool_use", "tool_result", "patch"];

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match value.get("type").and_then(Value::as_str) {
            Some(kind) if KNOWN.contains(&kind) => {
                serde_json::from_value(value).map(Self::Known).map_err(serde::de::Error::custom)
            }
            _ => Ok(Self::Other(value)),
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchFile {
    pub path: String,
    pub change: FileChange,
    pub moved_to: Option<String>,
    /// The file's hunks as a unified diff (`@@ -1,2 +1,3 @@` and their lines); file headers
    /// are not needed.
    #[serde(default)]
    pub diff: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileChange {
    Add,
    Update,
    Delete,
}

impl Message for PushedMessage {
    fn id(&self) -> Option<MessageId> {
        self.id.clone().map(MessageId::from)
    }

    fn role(&self) -> Role {
        match self.role.as_str() {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "tool" => Role::Tool,
            other => Role::Other(other.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.time
    }

    fn content(&self) -> Vec<Content> {
        self.content.iter().map(Block::content).collect()
    }

    fn model(&self) -> Option<String> {
        self.model.clone()
    }

    fn provider(&self) -> Option<String> {
        self.provider.clone()
    }

    fn usage(&self) -> Option<Usage> {
        self.usage
    }

    fn stop_reason(&self) -> Option<StopReason> {
        Some(match self.stop.as_deref()? {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            "stop_sequence" => StopReason::StopSequence,
            "refusal" => StopReason::Refusal,
            "aborted" => StopReason::Aborted,
            "error" => StopReason::Error,
            other => StopReason::Other(other.to_owned()),
        })
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    fn git_branch(&self) -> Option<String> {
        self.branch.clone()
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent.clone().map(MessageId::from)
    }

    fn parent_session(&self) -> Option<SessionId> {
        self.parent_session.clone().map(SessionId::from)
    }

    fn parent_kind(&self) -> Option<ParentKind> {
        Some(match self.parent_kind? {
            Kind::Subagent => ParentKind::Subagent,
            Kind::Fork => ParentKind::Fork,
            Kind::Continuation => ParentKind::Continuation,
        })
    }

    fn turn_id(&self) -> Option<String> {
        self.turn.clone()
    }

    fn title(&self) -> Option<TitleChange> {
        let title = self.title.as_ref()?;
        let source = match title.source {
            Source::Summary => TitleSource::Summary,
            Source::Generated => TitleSource::Generated,
            Source::Named => TitleSource::Named,
            Source::Agent => TitleSource::Agent,
        };
        Some(TitleChange {
            source,
            text: title.text.clone(),
        })
    }
}

impl Block {
    fn content(&self) -> Content {
        let known = match self {
            Self::Known(known) => known.clone(),
            Self::Other(value) => return Content::Other(value.clone()),
        };
        match known {
            Known::Text { text } => Content::Text(text),
            Known::Reasoning {
                text: Some(text), ..
            } => Content::Reasoning(text),
            Known::Reasoning { text: None, tokens } => Content::ReasoningSummary { tokens },
            Known::Summary { text } => Content::Summary(text),
            Known::Error { text } => Content::Error(text),
            Known::ToolUse { id, name, input } => Content::ToolUse(ToolUse {
                id: ToolCallId::from(id),
                name,
                input,
            }),
            Known::ToolResult { id, output, error } => Content::ToolResult(ToolResult {
                call: ToolCallId::from(id),
                output,
                error,
            }),
            Known::Patch { id, files } => Content::Patch(Patch {
                call: ToolCallId::from(id),
                files: files
                    .into_iter()
                    .map(|file| FilePatch {
                        path: file.path,
                        change: match file.change {
                            FileChange::Add => Change::Add,
                            FileChange::Update => Change::Update,
                            FileChange::Delete => Change::Delete,
                        },
                        moved_to: file.moved_to,
                        hunks: Hunk::parse(&file.diff),
                    })
                    .collect(),
            }),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::num::NonZeroUsize;

    use futures::TryStreamExt;
    use rstest::rstest;
    use serde_json::json;

    use super::*;
    use crate::harnesstools::ccode::session::CcodeSession;
    use crate::harnesstools::codex::session::CodexSession;
    use crate::harnesstools::copilot::session::CopilotSession;
    use crate::harnesstools::pi::session::PiSession;
    use crate::harnesstools::session::{AnyMessage, Session};
    use crate::path::PathBufExt as _;
    use crate::sync::BlockingPool;

    /// `m` as an agent would push it.
    fn pushed(m: &impl Message) -> PushedMessage {
        PushedMessage {
            v: Version,
            id: m.id().map(String::from),
            parent: m.parent_id().map(String::from),
            role: match m.role() {
                Role::User => "user".to_owned(),
                Role::Assistant => "assistant".to_owned(),
                Role::System => "system".to_owned(),
                Role::Tool => "tool".to_owned(),
                Role::Other(other) => other,
            },
            time: m.timestamp(),
            content: m.content().into_iter().map(block).collect(),
            cwd: m.cwd(),
            branch: m.git_branch(),
            model: m.model(),
            provider: m.provider(),
            turn: m.turn_id(),
            usage: m.usage(),
            stop: m.stop_reason().map(|stop| match stop {
                StopReason::EndTurn => "end_turn".to_owned(),
                StopReason::MaxTokens => "max_tokens".to_owned(),
                StopReason::ToolUse => "tool_use".to_owned(),
                StopReason::StopSequence => "stop_sequence".to_owned(),
                StopReason::Refusal => "refusal".to_owned(),
                StopReason::Aborted => "aborted".to_owned(),
                StopReason::Error => "error".to_owned(),
                StopReason::Other(other) => other,
            }),
            title: m.title().map(|title| Title {
                text: title.text,
                source: match title.source {
                    TitleSource::Summary => Source::Summary,
                    TitleSource::Generated => Source::Generated,
                    TitleSource::Named => Source::Named,
                    TitleSource::Agent => Source::Agent,
                },
            }),
            parent_session: m.parent_session().map(String::from),
            parent_kind: m.parent_kind().map(|kind| match kind {
                ParentKind::Subagent => Kind::Subagent,
                ParentKind::Fork => Kind::Fork,
                ParentKind::Continuation => Kind::Continuation,
            }),
        }
    }

    fn block(content: Content) -> Block {
        Block::Known(match content {
            Content::Text(text) => Known::Text { text },
            Content::Reasoning(text) => Known::Reasoning {
                text: Some(text),
                tokens: None,
            },
            Content::ReasoningSummary { tokens } => Known::Reasoning { text: None, tokens },
            Content::Summary(text) => Known::Summary { text },
            Content::Error(text) => Known::Error { text },
            Content::ToolUse(call) => Known::ToolUse {
                id: call.id.into(),
                name: call.name,
                input: call.input,
            },
            Content::ToolResult(result) => Known::ToolResult {
                id: result.call.into(),
                output: result.output,
                error: result.error,
            },
            Content::Patch(patch) => Known::Patch {
                id: patch.call.into(),
                files: patch
                    .files
                    .iter()
                    .map(|file| PatchFile {
                        path: file.path.clone(),
                        change: match file.change {
                            Change::Add => FileChange::Add,
                            Change::Update => FileChange::Update,
                            Change::Delete => FileChange::Delete,
                        },
                        moved_to: file.moved_to.clone(),
                        diff: file.hunks_text(),
                    })
                    .collect(),
            },
            Content::Other(value) => return Block::Other(value),
            Content::Unknown(_) => unreachable!("a harness's reader makes no unknown block"),
        })
    }

    /// Everything capture reads from a line.
    fn read_by_capture(m: &impl Message) -> Value {
        json!([
            m.id(),
            m.parent_id(),
            m.role(),
            m.timestamp().map(OffsetDateTime::unix_timestamp_nanos).map(|ns| ns.to_string()),
            m.content(),
            m.model(),
            m.provider(),
            m.usage(),
            m.stop_reason(),
            m.cwd(),
            m.git_branch(),
            m.parent_session(),
            m.parent_kind(),
            m.turn_id(),
            m.title(),
        ])
    }

    async fn fixture(harness: &str, file: &str) -> Vec<AnyMessage> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .joined("tests/fixtures")
            .joined(harness)
            .joined(file);
        let (id, pool) = (SessionId::from("s".to_owned()), BlockingPool::new(NonZeroUsize::MIN));
        match harness {
            "ccode" => {
                CcodeSession::open(id, path, pool)
                    .read()
                    .map_ok(AnyMessage::from)
                    .try_collect()
                    .await
            }
            "codex" => {
                CodexSession::open(id, path, pool)
                    .read()
                    .map_ok(AnyMessage::from)
                    .try_collect()
                    .await
            }
            "copilot" => {
                CopilotSession::open(id, path, pool)
                    .read()
                    .map_ok(AnyMessage::from)
                    .try_collect()
                    .await
            }
            "pi" => {
                PiSession::open(id, path, pool).read().map_ok(AnyMessage::from).try_collect().await
            }
            _ => unreachable!(),
        }
        .unwrap()
    }

    /// Every line of a real transcript, pushed, reads back as capture read it from the transcript.
    #[rstest]
    #[tokio::test]
    async fn a_transcript_pushed_reads_as_captured(
        #[values(
            ("ccode", "session1.jsonl"),
            ("codex", "session1.jsonl"),
            ("codex", "code-mode.jsonl"),
            ("codex", "paginated-compacted.jsonl"),
            ("codex", "forked-subagent.jsonl"),
            ("pi", "session1.jsonl"),
            ("pi", "session2.jsonl"),
            ("pi", "session-mock-0.85.jsonl"),
            ("copilot", "task-and-edit.jsonl"),
            ("copilot", "reasoning.jsonl"),
            ("copilot", "compacted.jsonl")
        )]
        (harness, file): (&str, &str),
    ) {
        let lines = fixture(harness, file).await;
        assert!(!lines.is_empty());
        lines.iter().for_each(assert_round_trips);
    }

    /// `line`, pushed, reads back as capture read it.
    pub fn assert_round_trips(line: &impl Message) {
        let json = serde_json::to_string(&pushed(line)).unwrap();
        let back: PushedMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(read_by_capture(&back), read_by_capture(line), "{json}");
    }

    #[rstest]
    #[case::no_version(r#"{"role":"user"}"#)]
    #[case::another_version(r#"{"v":2,"role":"user"}"#)]
    #[case::unknown_field(r#"{"v":1,"role":"user","colour":"red"}"#)]
    #[case::no_role(r#"{"v":1,"content":[]}"#)]
    #[case::malformed_known_block(r#"{"v":1,"role":"user","content":[{"type":"text"}]}"#)]
    #[case::unknown_field_in_known_block(
        r#"{"v":1,"role":"user","content":[{"type":"text","text":"a","x":1}]}"#
    )]
    #[case::unknown_title_source(r#"{"v":1,"role":"user","title":{"text":"a","source":"mine"}}"#)]
    fn malformed_lines_are_refused(#[case] line: &str) {
        assert!(serde_json::from_str::<PushedMessage>(line).is_err());
    }

    #[rstest]
    fn blocks_read_as_content() {
        let line: PushedMessage = serde_json::from_value(json!({
            "v": 1,
            "role": "assistant",
            "content": [
                {"type": "image", "mime": "image/png"},
                {"type": "reasoning", "tokens": 12},
                {"type": "patch", "id": "t1", "files": [
                    {"path": "a.rs", "change": "update", "diff": "@@ -1 +1 @@\n-a\n+b\n"}
                ]}
            ]
        }))
        .unwrap();
        assert_eq!(line.content(), vec![
            Content::Other(json!({"type": "image", "mime": "image/png"})),
            Content::ReasoningSummary { tokens: Some(12) },
            Content::Patch(Patch {
                call: ToolCallId::from("t1".to_owned()),
                files: vec![FilePatch {
                    path: "a.rs".to_owned(),
                    change: Change::Update,
                    moved_to: None,
                    hunks: vec![Hunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        lines: vec!["-a".to_owned(), "+b".to_owned()],
                    }],
                }],
            }),
        ]);
    }
}
