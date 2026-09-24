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
    /// Payload-free reasoning activity. Tokens are harness-reported model-call totals,
    /// never estimates or additional usage to add to output tokens.
    /// Older readers that do not know this variant skip records containing it; sync peers
    /// need a compatible version to display these newly captured messages.
    ReasoningSummary {
        tokens: Option<u64>,
    },
}

/// Human-readable breadcrumb shared by transcript and RPC rendering.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
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

    proptest! {
        #[test]
        fn message_id_display_equals_source(raw in "[a-zA-Z0-9-]{0,32}") {
            let id = MessageId::from(raw.clone());
            prop_assert_eq!(id.to_string(), raw);
        }
    }
}
