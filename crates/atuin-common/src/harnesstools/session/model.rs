use derive_more::{AsRef, Display, From, Into};
use serde::{Deserialize, Serialize};

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
    Other(String),
}

#[derive(Clone, Debug)]
pub struct SessionEvent<M> {
    pub session: SessionId,
    pub kind: SessionEventKind<M>,
}

#[derive(Clone, Debug)]
pub enum SessionEventKind<M> {
    Started,
    Message(M),
}

impl<M> SessionEvent<M> {
    #[must_use]
    pub fn started(session: SessionId) -> Self {
        Self {
            session,
            kind: SessionEventKind::Started,
        }
    }

    #[must_use]
    pub fn message(session: SessionId, message: M) -> Self {
        Self {
            session,
            kind: SessionEventKind::Message(message),
        }
    }

    #[must_use]
    pub fn map_message<N>(self, f: impl FnOnce(M) -> N) -> SessionEvent<N> {
        SessionEvent {
            session: self.session,
            kind: match self.kind {
                SessionEventKind::Started => SessionEventKind::Started,
                SessionEventKind::Message(message) => SessionEventKind::Message(f(message)),
            },
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
