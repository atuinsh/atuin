use std::borrow::Cow;
use std::fmt;

use derive_more::{AsRef, Display, From, Into};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::harnesstools::session::{Checkpoint, Patch};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, From, Into, AsRef, Serialize, Deserialize)]
pub struct SessionId(#[as_ref(str)] String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, From, Into, AsRef, Serialize, Deserialize)]
pub struct MessageId(#[as_ref(str)] String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, From, Into, AsRef, Serialize, Deserialize)]
pub struct ToolCallId(#[as_ref(str)] String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
    Other(String),
}

/// A block of a message's content.
///
/// A block is stored under its variant's name, in records' msgpack and in JSON alike. For
/// example, a text block is `{"Text": "hi"}`. `ContentVisitor` reads a name that this build
/// doesn't know as [`Content::Unknown`]. The rest of the message then still reads. Readers do
/// this from v6 on, so a new variant needs no new record version. A new variant does need three
/// things:
///
/// - Its name goes in `ContentVisitor`. The test `every_kind_reads_back_as_itself` checks this.
/// - It needs a sidecar migration that rebuilds the projection from the record store, as
///   `0006_reproject_for_hermes.sql` does. A replay never rewrites a row that it already has. So
///   the migration empties the projection and its watermarks, and then bumps
///   `projection_state.generation`. Messages stored while the variant was unknown are then read
///   again, with their search text and block bits.
/// - Its data must be something that JSON can hold. For example, JSON can't hold a map keyed by
///   numbers. A reader that doesn't know the variant fails on such data, as readers did before.
///
/// Never rename a variant, because its name is what's stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Content {
    Text(String),
    Reasoning(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    Other(serde_json::Value),
    /// Payload-free reasoning activity: a presence marker. How many tokens the call spent
    /// reasoning is usage ([`Usage::reasoning`]), counted once per call like the rest; `tokens`
    /// is only for display, never estimates or usage to add up.
    /// Older readers that do not know this variant skip records containing it; sync peers
    /// need a compatible version to display these newly captured messages.
    ReasoningSummary {
        tokens: Option<u64>,
    },
    /// A model-written summary standing in for earlier conversation (compaction, an abandoned
    /// branch). Conversation text whatever the line's role.
    Summary(String),
    /// Why a model call failed or was aborted, as the harness reported it.
    Error(String),
    /// The changes a tool call made to files, as its harness recorded them beside the call's
    /// result (or, for Codex, after it).
    Patch(Patch),
    /// A block of a kind that a newer atuin wrote and that this build doesn't know. It is written
    /// back as it was read. Otherwise it counts for nothing: it holds no text, no call and no
    /// conversation.
    #[serde(untagged)]
    Unknown(UnknownBlock),
}

/// The kind's name and the data of a [`Content::Unknown`] block. The data is JSON. It is `None`
/// for a kind with no data, which is stored as its name alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownBlock {
    pub kind: String,
    pub value: Option<serde_json::Value>,
}

impl Serialize for UnknownBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.value {
            None => serializer.serialize_str(&self.kind),
            Some(value) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(&self.kind, value)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ContentVisitor)
    }
}

/// Reads a [`Content`] block. It reads a kind that it doesn't know as [`Content::Unknown`].
///
/// A derived `#[serde(untagged)]` fallback variant would do the same, but it has two costs. It
/// reads every block into a buffer before it matches the block. It also turns an error in a block
/// of a known kind into an unknown block, which hides the error. This visitor reads a known kind
/// straight from the input, and a malformed block of that kind fails as it always did.
struct ContentVisitor;

impl<'de> Visitor<'de> for ContentVisitor {
    type Value = Content;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a content block")
    }

    /// A kind with no data. This build knows no such kind.
    fn visit_str<E: de::Error>(self, kind: &str) -> Result<Content, E> {
        Ok(Content::Unknown(UnknownBlock {
            kind: kind.to_owned(),
            value: None,
        }))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Content, A::Error> {
        let Some(kind) = map.next_key::<String>()? else {
            return Err(de::Error::invalid_length(0, &self));
        };
        let content = match kind.as_str() {
            "Text" => Content::Text(map.next_value()?),
            "Reasoning" => Content::Reasoning(map.next_value()?),
            "ToolUse" => Content::ToolUse(map.next_value()?),
            "ToolResult" => Content::ToolResult(map.next_value()?),
            "Other" => Content::Other(map.next_value()?),
            "ReasoningSummary" => {
                let ReasoningSummaryFields { tokens } = map.next_value()?;
                Content::ReasoningSummary { tokens }
            }
            "Summary" => Content::Summary(map.next_value()?),
            "Error" => Content::Error(map.next_value()?),
            "Patch" => Content::Patch(map.next_value()?),
            _ => Content::Unknown(UnknownBlock {
                kind,
                value: Some(map.next_value()?),
            }),
        };
        if map.next_key::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(2, &self));
        }
        Ok(content)
    }
}

/// The fields of [`Content::ReasoningSummary`], read as a derived struct variant's fields are.
/// Records' positional msgpack holds them as a sequence. JSON holds them as a map.
#[derive(Deserialize)]
struct ReasoningSummaryFields {
    tokens: Option<u64>,
}

/// The [`Content::Summary`] of a compaction whose summary can't be read (Codex's, compacted by
/// the API, which keeps it encrypted): the conversation before it is no longer what the model
/// sees, and no summary says what it was.
pub const UNREADABLE_SUMMARY: &str =
    "[the conversation before this was compacted; its summary can't be read]";

/// Human-readable breadcrumb shared by the transcript and `atuin threads` rendering.
#[must_use]
pub fn reasoning_label(tokens: Option<u64>) -> String {
    tokens.map_or_else(|| "Reasoned".to_owned(), |n| format!("Reasoning · {n} tokens"))
}

/// Whether a message of `content` is tool output: tool results (and the patches they made) and
/// nothing else, whatever its envelope role, since some harnesses (Claude Code) send tool output
/// back as a user turn.
#[must_use]
pub fn is_tool_output(content: &[Content]) -> bool {
    !content.is_empty()
        && content.iter().all(|c| matches!(c, Content::ToolResult(_) | Content::Patch(_)))
}

/// Whether a row with `role` and `content` is a message that makes a line of conversation worth
/// telling apart from another: a user prompt, or assistant text. Tool calls, their results,
/// reasoning, usage and system context are not, so a fork in a session's rows only counts as a
/// branch where it holds one of these.
#[must_use]
pub fn is_substantive(role: &Role, content: &[Content]) -> bool {
    match role {
        Role::User => true,
        Role::Assistant => {
            content.iter().any(|c| matches!(c, Content::Text(t) if !t.trim().is_empty()))
        }
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolUse {
    pub id: ToolCallId,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call: ToolCallId,
    pub output: serde_json::Value,
    pub error: bool,
}

impl ToolResult {
    /// The output as display text: a JSON string's contents verbatim, a list of content blocks
    /// as their text (Claude Code's and pi's `text`, Codex's `input_text`; a block without text
    /// as encoded), any other JSON as encoded, or `None` when capture did not keep it.
    #[must_use]
    pub fn output_text(&self) -> Option<Cow<'_, str>> {
        match &self.output {
            serde_json::Value::Null => None,
            serde_json::Value::String(text) => Some(Cow::Borrowed(text)),
            serde_json::Value::Array(blocks) => Some(Cow::Owned(
                blocks
                    .iter()
                    .map(|block| match block {
                        serde_json::Value::String(text) => text.clone(),
                        _ => {
                            block["text"].as_str().map_or_else(|| block.to_string(), str::to_owned)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
            json => Some(Cow::Owned(json.to_string())),
        }
    }
}

/// The tokens of a model call (or, summed, of a session).
///
/// Every harness's reader fills it the same way, whatever its provider counts: `input` is the
/// input neither read from nor written to the prompt cache, and `cache_read` and `cache_write`
/// the rest, so the three add up to the input the model processed ([`Self::total_input`]).
/// Anthropic counts input this way; OpenAI (and so Codex) counts the cached tokens inside
/// `input_tokens`, and those readers take them out.
///
/// Also the daemon's `ai.agent.Tokens` and atuin-octavo's `atuin.api.v1.TokenUsage` wire messages,
/// so its prost tags are their field numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "proto", derive(prost::Message), prost(skip_debug))]
#[cfg_attr(not(feature = "proto"), derive(Default))]
pub struct Usage {
    /// Input not read from the cache nor written to it.
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "1"))]
    pub input: Option<u64>,
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "2"))]
    pub output: Option<u64>,
    /// Input read from the prompt cache.
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "3"))]
    pub cache_read: Option<u64>,
    /// Input written to the prompt cache.
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "4"))]
    pub cache_write: Option<u64>,
    /// Reasoning (thinking) tokens of the model call, already included in `output`: a
    /// breakdown, never extra usage to add to it.
    #[serde(default)]
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "5"))]
    pub reasoning: Option<u64>,
}

impl Usage {
    /// Each count `f` makes of it, a count not reported staying unreported.
    #[must_use]
    pub fn map(self, f: impl Fn(u64) -> u64) -> Self {
        let Self {
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
        } = self;

        Self {
            input: input.map(&f),
            output: output.map(&f),
            cache_read: cache_read.map(&f),
            cache_write: cache_write.map(&f),
            reasoning: reasoning.map(&f),
        }
    }

    /// All the input the model processed: uncached, read from the cache and written to it.
    /// `None` when none of them was reported.
    #[must_use]
    pub fn total_input(&self) -> Option<u64> {
        match (self.input, self.cache_read, self.cache_write) {
            (None, None, None) => None,
            (input, read, write) => Some(
                input
                    .unwrap_or(0)
                    .saturating_add(read.unwrap_or(0))
                    .saturating_add(write.unwrap_or(0)),
            ),
        }
    }
}

/// How a session relates to the one it names as its parent. Harnesses link several kinds of
/// session to an earlier one, and readers treat them differently: a subagent is a fragment of
/// its parent's work, while a fork or continuation is a conversation a person carried on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParentKind {
    /// Spawned by its parent to do part of its work (a Claude Code `Task` agent, a Codex
    /// `spawn_agent` or reviewer thread, an opencode `@general` subagent).
    Subagent,
    /// A copy of the parent's conversation that went its own way (Claude Code
    /// `--fork-session`, a Codex or opencode fork, a pi branch).
    Fork,
    /// The parent's conversation carried on under a new session (a Codex revert).
    Continuation,
}

/// Where a session title came from. A higher source outranks a lower one whatever order they
/// arrive in, the way Claude Code shows an agent name over a custom title over a generated one
/// over a legacy summary; titles from one source replace each other, newest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TitleSource {
    /// A summary line standing in for a title (Claude Code's legacy `summary`).
    Summary,
    /// Written by the harness or a model (Claude Code `ai-title`, opencode's titles).
    Generated,
    /// Set by the user (Claude Code `/rename`, Codex thread names, Pi `/name`).
    Named,
    /// The name of the agent the session runs as (Claude Code `agent-name`).
    Agent,
}

/// A title a line assigns to its session, or takes away: `text` of `None` clears whatever
/// title `source` gave, letting a lower-ranked one show again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleChange {
    pub source: TitleSource,
    pub text: Option<String>,
}

impl TitleChange {
    /// `text` from `source`, trimmed; blank text clears.
    #[must_use]
    pub fn new(source: TitleSource, text: &str) -> Self {
        let text = text.trim();
        Self {
            source,
            text: (!text.is_empty()).then(|| text.to_owned()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    Refusal,
    Aborted,
    Error,
    Other(String),
}

/// One transcript line, tagged with the session it belongs to.
#[derive(Clone, Debug)]
pub struct SessionEvent<M> {
    pub session: SessionId,
    /// Just past this message: store it, and resume from it with `Session::messages_from`.
    pub checkpoint: Checkpoint,
    pub message: M,
}

impl<M> SessionEvent<M> {
    #[must_use]
    pub fn map_message<N>(self, f: impl FnOnce(M) -> N) -> SessionEvent<N> {
        SessionEvent {
            session: self.session,
            checkpoint: self.checkpoint,
            message: f(self.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("abc")]
    #[case("6f9619ff-8b86-d011-b42d-00cf4fc964ff")]
    fn session_id_round_trips_through_string(#[case] raw: &str) {
        let id = SessionId::from(raw.to_owned());
        assert_eq!(id.to_string(), raw);
        assert_eq!(id.as_ref(), raw);
    }

    /// A block of every kind that this build knows. The match fails to compile until a new kind
    /// is listed. Then the new kind needs a block here too.
    fn every_kind() -> Vec<Content> {
        use crate::harnesstools::session::{Change, FilePatch, Hunk};

        let blocks = vec![
            Content::Text("hi".to_owned()),
            Content::Reasoning("hmm".to_owned()),
            Content::ToolUse(ToolUse {
                id: ToolCallId::from("t1".to_owned()),
                name: "bash".to_owned(),
                input: serde_json::json!({"cmd": "ls"}),
            }),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("t1".to_owned()),
                output: serde_json::json!("a\nb"),
                error: true,
            }),
            Content::Other(serde_json::json!({"type": "image"})),
            Content::ReasoningSummary { tokens: Some(7) },
            Content::ReasoningSummary { tokens: None },
            Content::Summary("so far".to_owned()),
            Content::Error("overloaded".to_owned()),
            Content::Patch(Patch {
                call: ToolCallId::from("t1".to_owned()),
                files: vec![FilePatch {
                    path: "a.txt".to_owned(),
                    moved_to: None,
                    change: Change::Update,
                    hunks: vec![Hunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        lines: vec!["-a".to_owned(), "+b".to_owned()],
                    }],
                }],
            }),
        ];
        for block in &blocks {
            match block {
                Content::Text(_)
                | Content::Reasoning(_)
                | Content::ToolUse(_)
                | Content::ToolResult(_)
                | Content::Other(_)
                | Content::ReasoningSummary { .. }
                | Content::Summary(_)
                | Content::Error(_)
                | Content::Patch(_) => {}
                Content::Unknown(_) => unreachable!("not a kind this build knows"),
            }
        }
        blocks
    }

    /// [`ContentVisitor`] matches the names that the derived `Serialize` writes.
    #[rstest]
    fn every_kind_reads_back_as_itself() {
        let blocks = every_kind();

        let msgpack: Vec<Content> =
            rmp_serde::from_slice(&rmp_serde::to_vec(&blocks).unwrap()).unwrap();
        let json: Vec<Content> =
            serde_json::from_str(&serde_json::to_string(&blocks).unwrap()).unwrap();

        assert_eq!(msgpack, blocks);
        assert_eq!(json, blocks);
    }

    /// Kinds that a newer atuin might add. One has fields, one has a value, and one has no data.
    #[derive(Serialize)]
    enum Newer {
        Undone {
            through: String,
        },
        Marker(u32),
        Ping,
    }

    /// A kind that this build doesn't know reads as [`Content::Unknown`], beside the kinds that it
    /// knows. It is written back exactly as it was read.
    #[rstest]
    #[case::fields(Newer::Undone { through: "u1".to_owned() }, "Undone", Some(serde_json::json!(["u1"])), Some(serde_json::json!({"through": "u1"})))]
    #[case::value(Newer::Marker(3), "Marker", Some(serde_json::json!(3)), Some(serde_json::json!(3)))]
    #[case::no_data(Newer::Ping, "Ping", None, None)]
    fn an_unknown_kind_is_kept_as_it_was_written(
        #[case] newer: Newer,
        #[case] kind: &str,
        // Fields are positional in records' msgpack, named in JSON.
        #[case] msgpack_value: Option<serde_json::Value>,
        #[case] json_value: Option<serde_json::Value>,
    ) {
        let text = serde_json::json!({"Text": "hi"});
        let written = (
            rmp_serde::to_vec(&(&text, &newer)).unwrap(),
            serde_json::to_string(&(&text, &newer)).unwrap(),
        );
        let unknown = |value| {
            Content::Unknown(UnknownBlock {
                kind: kind.to_owned(),
                value,
            })
        };

        let msgpack: Vec<Content> = rmp_serde::from_slice(&written.0).unwrap();
        let json: Vec<Content> = serde_json::from_str(&written.1).unwrap();

        let hi = Content::Text("hi".to_owned());
        assert_eq!(msgpack, vec![hi.clone(), unknown(msgpack_value)]);
        assert_eq!(json, vec![hi, unknown(json_value)]);
        assert_eq!(rmp_serde::to_vec(&msgpack).unwrap(), written.0);
        assert_eq!(serde_json::to_string(&json).unwrap(), written.1);
    }

    /// A block of a known kind that holds the wrong data is an error, not an unknown block. It
    /// was an error before unknown kinds were read, too.
    #[rstest]
    #[case::wrong_data(r#"{"Text": 5}"#)]
    #[case::wrong_fields(r#"{"ToolUse": {"name": "bash"}}"#)]
    #[case::two_kinds(r#"{"Text": "a", "Summary": "b"}"#)]
    #[case::no_kind("{}")]
    #[case::not_a_block("[1]")]
    fn a_malformed_block_is_an_error(#[case] json: &str) {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let msgpack = rmp_serde::to_vec(&value).unwrap();

        assert!(serde_json::from_str::<Content>(json).is_err(), "{json}");
        assert!(rmp_serde::from_slice::<Content>(&msgpack).is_err(), "{json}");
    }

    #[rstest]
    fn content_variants_are_distinct() {
        let a = Content::Text("hi".into());
        let b = Content::ToolUse(ToolUse {
            id: ToolCallId::from("t1".to_owned()),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        });
        assert_ne!(a, b);
    }

    #[rstest]
    #[case::string(serde_json::json!("line1\nline2"), Some("line1\nline2"))]
    #[case::structured(serde_json::json!({"exit": 0}), Some(r#"{"exit":0}"#))]
    #[case::blocks(
        serde_json::json!([{"type": "text", "text": "a"}, {"type": "input_text", "text": "b"}]),
        Some("a\nb")
    )]
    #[case::blocks_without_text(
        serde_json::json!([{"type": "text", "text": "a"}, {"type": "image"}]),
        Some("a\n{\"type\":\"image\"}")
    )]
    #[case::uncaptured(serde_json::Value::Null, None)]
    fn tool_result_output_text_unwraps_json_strings(
        #[case] output: serde_json::Value,
        #[case] want: Option<&str>,
    ) {
        let result = ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output,
            error: false,
        };
        assert_eq!(result.output_text().as_deref(), want);
    }

    proptest! {
        #[test]
        fn message_id_display_equals_source(raw in "[a-zA-Z0-9-]{0,32}") {
            let id = MessageId::from(raw.clone());
            prop_assert_eq!(id.to_string(), raw);
        }
    }
}
