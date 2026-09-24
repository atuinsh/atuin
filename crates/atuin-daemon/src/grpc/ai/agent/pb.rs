//! Model conversion utilities for the `ai.agent` gRPC protobuf.
mod codegen {
    #![allow(clippy::must_use_candidate)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("ai.agent");
}

use atuin_client::ai_session::{
    HarnessKind as DomainHarnessKind, HarnessSession as DomainHarnessSession,
    Message as DomainMessage, NativeSessionId, Session as DomainSession,
};
use atuin_common::harnesstools::session::{
    Content, Role as DomainRole, StopReason as DomainStopReason, Usage,
};
pub use codegen::*;
use thiserror::Error;

use crate::grpc::common::pb::Uuid;

impl From<DomainHarnessKind> for HarnessKind {
    fn from(value: DomainHarnessKind) -> Self {
        match value {
            DomainHarnessKind::Unknown => Self::Unknown,
            DomainHarnessKind::ClaudeCode => Self::ClaudeCode,
            DomainHarnessKind::Codex => Self::Codex,
            DomainHarnessKind::Copilot => Self::Copilot,
            DomainHarnessKind::Opencode => Self::Opencode,
            DomainHarnessKind::Pi => Self::Pi,
        }
    }
}

impl From<HarnessKind> for DomainHarnessKind {
    fn from(value: HarnessKind) -> Self {
        match value {
            HarnessKind::Unknown => Self::Unknown,
            HarnessKind::ClaudeCode => Self::ClaudeCode,
            HarnessKind::Codex => Self::Codex,
            HarnessKind::Copilot => Self::Copilot,
            HarnessKind::Opencode => Self::Opencode,
            HarnessKind::Pi => Self::Pi,
        }
    }
}

impl From<DomainHarnessSession> for HarnessSession {
    fn from(value: DomainHarnessSession) -> Self {
        Self {
            harness: HarnessKind::from(value.harness) as i32,
            session_id: value.session.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum HarnessSessionParseError {
    #[error("unrecognized harness kind: {0}")]
    UnknownHarnessKind(i32),
}

impl TryFrom<HarnessSession> for DomainHarnessSession {
    type Error = HarnessSessionParseError;

    fn try_from(value: HarnessSession) -> Result<Self, Self::Error> {
        let harness = HarnessKind::try_from(value.harness)
            .map_err(|_| HarnessSessionParseError::UnknownHarnessKind(value.harness))?
            .into();
        Ok(Self {
            harness,
            session: NativeSessionId::from(value.session_id),
        })
    }
}

impl From<DomainRole> for Role {
    fn from(value: DomainRole) -> Self {
        match value {
            DomainRole::User => Self::User,
            DomainRole::Assistant => Self::Assistant,
            DomainRole::System => Self::System,
            DomainRole::Tool => Self::Tool,
            DomainRole::Other(_) => Self::Unknown,
        }
    }
}

impl From<DomainStopReason> for StopReason {
    fn from(value: DomainStopReason) -> Self {
        match value {
            DomainStopReason::EndTurn
            | DomainStopReason::StopSequence
            | DomainStopReason::Refusal => Self::EndTurn,
            DomainStopReason::MaxTokens => Self::MaxTokens,
            DomainStopReason::ToolUse => Self::ToolUse,
            DomainStopReason::Aborted => Self::Aborted,
            DomainStopReason::Error => Self::Error,
            DomainStopReason::Other(_) => Self::Unknown,
        }
    }
}

impl From<Usage> for Tokens {
    fn from(value: Usage) -> Self {
        Self {
            input: value.input.unwrap_or(0),
            output: value.output.unwrap_or(0),
            cache_read: value.cache_read.unwrap_or(0),
            cache_write: value.cache_write.unwrap_or(0),
            reasoning: value.reasoning.unwrap_or(0),
        }
    }
}

impl From<Content> for ContentBlock {
    fn from(value: Content) -> Self {
        use content_block::Block;

        let block = match value {
            Content::Text(text) => Block::Text(text),
            Content::Reasoning(text) => Block::Thinking(text),
            Content::ReasoningSummary { tokens } => {
                Block::Thinking(atuin_common::harnesstools::session::model::reasoning_label(tokens))
            }
            // Capture stores a JSON null in place of arguments and results it does not keep; on
            // the wire that is "not captured", an empty string, not the text `null`.
            Content::ToolUse(tu) => Block::ToolCall(ToolCall {
                id: tu.id.into(),
                name: tu.name,
                input: match tu.input {
                    serde_json::Value::Null => String::new(),
                    other => serde_json::to_string(&other).unwrap_or_default(),
                },
            }),
            Content::ToolResult(tr) => Block::ToolResult(ToolResult {
                tool_use_id: tr.call.into(),
                // The proto documents this as the raw tool output, unlike ToolCall.input which is
                // JSON. A string output is already the raw text, so emit it verbatim rather than
                // re-encoding it into a quoted, escaped JSON string.
                content: match tr.output {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => s,
                    other => serde_json::to_string(&other).unwrap_or_default(),
                },
                is_error: tr.error,
            }),
            Content::Summary(text) => Block::Summary(text),
            Content::Error(text) => Block::Error(text),
            Content::Other(v) => Block::Text(v.to_string()),
        };

        Self { block: Some(block) }
    }
}

impl From<DomainMessage> for Message {
    fn from(value: DomainMessage) -> Self {
        // The enum collapses non-standard roles to Unknown; keep the original string so clients can
        // display the real role (e.g. codex "developer") instead of "unknown".
        let role_label = match &value.role {
            DomainRole::Other(other) => Some(other.clone()),
            _ => None,
        };
        Self {
            id: Some(Uuid {
                value: value.id.0.into_bytes().to_vec(),
            }),
            harness: HarnessKind::from(value.session.harness) as i32,
            session_id: value.session.session.into(),
            parent: value.parent.map(Into::into),
            thread: value.thread,
            source_id: value.source_id.into(),
            parent_source_id: value.parent_source_id.map(Into::into),
            timestamp: Some(prost_types::Timestamp {
                seconds: value.timestamp.unix_timestamp(),
                nanos: value.timestamp.nanosecond().cast_signed(),
            }),
            role: Role::from(value.role) as i32,
            content: value
                .content
                .into_iter()
                .map(|block| ContentBlock::from(block.with_reasoning_of(value.usage.as_ref())))
                .collect(),
            cwd: value.cwd.map(|path| path.to_string_lossy().into_owned()),
            git_branch: value.git_branch,
            model: value.model,
            tokens: Some(value.usage.map(Tokens::from).unwrap_or_default()),
            stop_reason: value.stop_reason.map(StopReason::from).unwrap_or(StopReason::Unknown)
                as i32,
            role_label,
        }
    }
}

impl From<DomainSession> for Session {
    fn from(value: DomainSession) -> Self {
        Self {
            harness: HarnessKind::from(value.handle.harness) as i32,
            session_id: value.handle.session.into(),
            parent: value.parent.map(Into::into),
            cwd: value.cwd.map(|path| path.to_string_lossy().into_owned()),
            git_branch: value.git_branch,
            model: value.model,
            started_at: Some(prost_types::Timestamp {
                seconds: value.started_at.unix_timestamp(),
                nanos: value.started_at.nanosecond().cast_signed(),
            }),
            updated_at: Some(prost_types::Timestamp {
                seconds: value.updated_at.unix_timestamp(),
                nanos: value.updated_at.nanosecond().cast_signed(),
            }),
            message_count: value.message_count,
            tokens: Some(value.usage.into()),
            title: value.title,
            preview: value.preview,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn arb_harness_kind() -> impl Strategy<Value = DomainHarnessKind> {
        prop_oneof![
            Just(DomainHarnessKind::Unknown),
            Just(DomainHarnessKind::ClaudeCode),
            Just(DomainHarnessKind::Codex),
            Just(DomainHarnessKind::Copilot),
            Just(DomainHarnessKind::Opencode),
            Just(DomainHarnessKind::Pi),
        ]
    }

    fn arb_harness_session() -> impl Strategy<Value = DomainHarnessSession> {
        (arb_harness_kind(), "[a-z0-9]{1,16}").prop_map(|(harness, session)| DomainHarnessSession {
            harness,
            session: NativeSessionId::from(session),
        })
    }

    proptest! {
        #[test]
        fn harness_session_roundtrips(hs in arb_harness_session()) {
            let pb: HarnessSession = hs.clone().into();
            prop_assert_eq!(DomainHarnessSession::try_from(pb).unwrap(), hs);
        }
    }

    #[rstest]
    #[case(DomainStopReason::StopSequence, StopReason::EndTurn)]
    #[case(DomainStopReason::Refusal, StopReason::EndTurn)]
    #[case(DomainStopReason::Other("x".into()), StopReason::Unknown)]
    fn stop_reason_coalesces_at_edge(#[case] from: DomainStopReason, #[case] want: StopReason) {
        assert_eq!(StopReason::from(from), want);
    }

    /// Capture nulls arguments and results it does not keep; the wire carries "not captured" as
    /// an empty string, never the text `null`.
    #[rstest]
    fn uncaptured_tool_payloads_are_empty_on_the_wire() {
        use atuin_common::harnesstools::session::{ToolCallId, ToolResult, ToolUse};

        let call: ContentBlock = Content::ToolUse(ToolUse {
            id: ToolCallId::from("c1".to_owned()),
            name: "Bash".to_owned(),
            input: serde_json::Value::Null,
        })
        .into();
        let content_block::Block::ToolCall(tc) = call.block.unwrap() else {
            panic!("expected a tool call block");
        };
        assert_eq!((tc.name.as_str(), tc.input.as_str()), ("Bash", ""));

        let result: ContentBlock = Content::ToolResult(ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output: serde_json::Value::Null,
            error: true,
        })
        .into();
        let content_block::Block::ToolResult(tr) = result.block.unwrap() else {
            panic!("expected a tool result block");
        };
        assert_eq!((tr.content.as_str(), tr.is_error), ("", true));
    }

    #[rstest]
    fn tool_result_string_output_is_emitted_raw() {
        use atuin_common::harnesstools::session::{ToolCallId, ToolResult};

        let raw: ContentBlock = Content::ToolResult(ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output: serde_json::Value::String("line1\nline2".to_owned()),
            error: false,
        })
        .into();
        let content_block::Block::ToolResult(tr) = raw.block.unwrap() else {
            panic!("expected a tool result block");
        };
        assert_eq!(tr.content, "line1\nline2");

        let structured: ContentBlock = Content::ToolResult(ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output: serde_json::json!({"exit": 0}),
            error: false,
        })
        .into();
        let content_block::Block::ToolResult(tr) = structured.block.unwrap() else {
            panic!("expected a tool result block");
        };
        assert_eq!(tr.content, r#"{"exit":0}"#);
    }

    #[rstest]
    fn other_role_is_carried_as_role_label() {
        use atuin_client::ai_session::SourceId;
        use atuin_domain::record::RecordId;
        use time::OffsetDateTime;

        let msg = DomainMessage::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(DomainHarnessSession {
                harness: DomainHarnessKind::Codex,
                session: NativeSessionId::from("s".to_owned()),
            })
            .source_id(SourceId::from("src".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(DomainRole::Other("developer".to_owned()))
            .content(vec![Content::Text("hi".to_owned())])
            .build();

        let pb = Message::from(msg);
        assert_eq!(pb.role, Role::Unknown as i32);
        assert_eq!(pb.role_label.as_deref(), Some("developer"));
    }

    #[rstest]
    fn usage_none_becomes_zero() {
        let t: Tokens = Usage {
            input: None,
            output: Some(3),
            cache_read: None,
            cache_write: None,
            reasoning: None,
        }
        .into();
        assert_eq!((t.input, t.output, t.cache_read, t.cache_write), (0, 3, 0, 0));
    }
}
