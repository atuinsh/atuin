use derive_more::{AsRef, Display, From, Into};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, From, Into, AsRef)]
pub struct SessionId(#[as_ref(str)] String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, From, Into, AsRef)]
pub struct MessageId(#[as_ref(str)] String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, From, Into, AsRef)]
pub struct ToolCallId(#[as_ref(str)] String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    Reasoning(String),
    ToolUse(ToolUse),
    ToolResult(ToolResult),
    Other(serde_json::Value),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolUse {
    pub id: ToolCallId,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub call: ToolCallId,
    pub output: serde_json::Value,
    pub error: bool,
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
