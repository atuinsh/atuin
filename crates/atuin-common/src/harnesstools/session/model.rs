use std::borrow::Cow;

use derive_more::{AsRef, Display, From, Into};
use serde::{Deserialize, Serialize};

use crate::harnesstools::session::Checkpoint;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// Human-readable breadcrumb shared by the transcript and `atuin ai session` rendering.
#[must_use]
pub fn reasoning_label(tokens: Option<u64>) -> String {
    tokens.map_or_else(|| "Reasoned".to_owned(), |n| format!("Reasoning · {n} tokens"))
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
    /// The output as display text: a JSON string's contents verbatim, any other JSON as encoded,
    /// or `None` when capture did not keep it.
    #[must_use]
    pub fn output_text(&self) -> Option<Cow<'_, str>> {
        match &self.output {
            serde_json::Value::Null => None,
            serde_json::Value::String(text) => Some(Cow::Borrowed(text)),
            json => Some(Cow::Owned(json.to_string())),
        }
    }
}

/// Also the daemon's `ai.agent.Tokens` wire message, so its prost tags are that message's field
/// numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "proto", derive(prost::Message), prost(skip_debug))]
#[cfg_attr(not(feature = "proto"), derive(Default))]
pub struct Usage {
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "1"))]
    pub input: Option<u64>,
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "2"))]
    pub output: Option<u64>,
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "3"))]
    pub cache_read: Option<u64>,
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "4"))]
    pub cache_write: Option<u64>,
    /// Reasoning (thinking) tokens of the model call, already included in `output`: a
    /// breakdown, never extra usage to add to it.
    #[serde(default)]
    #[cfg_attr(feature = "proto", prost(uint64, optional, tag = "5"))]
    pub reasoning: Option<u64>,
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
