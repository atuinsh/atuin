use atuin_client::ai_session::{HarnessKind, HarnessSession, Message, NativeSessionId, Session};
use atuin_common::harnesstools::session::{Content, Role, StopReason, Usage};
use thiserror::Error;

use crate::grpc::ai_agent::pb;
use crate::grpc::common::pb as common;

impl From<HarnessKind> for pb::HarnessKind {
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

impl From<pb::HarnessKind> for HarnessKind {
    fn from(value: pb::HarnessKind) -> Self {
        match value {
            pb::HarnessKind::Unknown => Self::Unknown,
            pb::HarnessKind::ClaudeCode => Self::ClaudeCode,
            pb::HarnessKind::Codex => Self::Codex,
            pb::HarnessKind::Copilot => Self::Copilot,
            pb::HarnessKind::Opencode => Self::Opencode,
            pb::HarnessKind::Pi => Self::Pi,
        }
    }
}

impl From<HarnessSession> for pb::HarnessSession {
    fn from(value: HarnessSession) -> Self {
        Self {
            harness: pb::HarnessKind::from(value.harness) as i32,
            session_id: value.session.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum HarnessSessionParseError {
    #[error("unrecognized harness kind: {0}")]
    UnknownHarnessKind(i32),
}

impl TryFrom<pb::HarnessSession> for HarnessSession {
    type Error = HarnessSessionParseError;

    fn try_from(value: pb::HarnessSession) -> Result<Self, Self::Error> {
        let harness = pb::HarnessKind::try_from(value.harness)
            .map_err(|_| HarnessSessionParseError::UnknownHarnessKind(value.harness))?
            .into();
        Ok(Self {
            harness,
            session: NativeSessionId::from(value.session_id),
        })
    }
}

impl From<Role> for pb::Role {
    fn from(value: Role) -> Self {
        match value {
            Role::User => Self::User,
            Role::Assistant => Self::Assistant,
            Role::System => Self::System,
            Role::Tool => Self::Tool,
            Role::Other(_) => Self::Unknown,
        }
    }
}

impl From<StopReason> for pb::StopReason {
    fn from(value: StopReason) -> Self {
        match value {
            StopReason::EndTurn | StopReason::StopSequence | StopReason::Refusal => Self::EndTurn,
            StopReason::MaxTokens => Self::MaxTokens,
            StopReason::ToolUse => Self::ToolUse,
            StopReason::Aborted => Self::Aborted,
            StopReason::Other(_) => Self::Unknown,
        }
    }
}

impl From<Usage> for pb::Tokens {
    fn from(value: Usage) -> Self {
        Self {
            input: value.input.unwrap_or(0),
            output: value.output.unwrap_or(0),
            cache_read: value.cache_read.unwrap_or(0),
            cache_write: value.cache_write.unwrap_or(0),
        }
    }
}

impl From<Content> for pb::ContentBlock {
    fn from(value: Content) -> Self {
        use pb::content_block::Block;

        let block = match value {
            Content::Text(text) => Block::Text(text),
            Content::Reasoning(text) => Block::Thinking(text),
            Content::ToolUse(tu) => Block::ToolCall(pb::ToolCall {
                id: tu.id.into(),
                name: tu.name,
                input: serde_json::to_string(&tu.input).unwrap_or_default(),
            }),
            Content::ToolResult(tr) => Block::ToolResult(pb::ToolResult {
                tool_use_id: tr.call.into(),
                // The proto documents this as the raw tool output, unlike ToolCall.input which is
                // JSON. A string output is already the raw text, so emit it verbatim rather than
                // re-encoding it into a quoted, escaped JSON string.
                content: match tr.output {
                    serde_json::Value::String(s) => s,
                    other => serde_json::to_string(&other).unwrap_or_default(),
                },
                is_error: tr.error,
            }),
            Content::Other(v) => Block::Text(v.to_string()),
        };

        Self { block: Some(block) }
    }
}

impl From<Message> for pb::Message {
    fn from(value: Message) -> Self {
        Self {
            id: Some(common::Uuid {
                value: value.id.0.into_bytes().to_vec(),
            }),
            harness: pb::HarnessKind::from(value.session.harness) as i32,
            session_id: value.session.session.into(),
            parent: value.parent.map(Into::into),
            thread: value.thread,
            source_id: value.source_id.into(),
            parent_source_id: value.parent_source_id.map(Into::into),
            timestamp: Some(prost_types::Timestamp {
                seconds: value.timestamp.unix_timestamp(),
                nanos: value.timestamp.nanosecond().cast_signed(),
            }),
            role: pb::Role::from(value.role) as i32,
            content: value.content.into_iter().map(pb::ContentBlock::from).collect(),
            cwd: value.cwd.map(|path| path.to_string_lossy().into_owned()),
            git_branch: value.git_branch,
            model: value.model,
            tokens: Some(value.usage.map(pb::Tokens::from).unwrap_or_default()),
            stop_reason: value
                .stop_reason
                .map(pb::StopReason::from)
                .unwrap_or(pb::StopReason::Unknown) as i32,
        }
    }
}

impl From<Session> for pb::Session {
    fn from(value: Session) -> Self {
        Self {
            harness: pb::HarnessKind::from(value.handle.harness) as i32,
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

    fn arb_harness_kind() -> impl Strategy<Value = HarnessKind> {
        prop_oneof![
            Just(HarnessKind::Unknown),
            Just(HarnessKind::ClaudeCode),
            Just(HarnessKind::Codex),
            Just(HarnessKind::Copilot),
            Just(HarnessKind::Opencode),
            Just(HarnessKind::Pi),
        ]
    }

    fn arb_harness_session() -> impl Strategy<Value = HarnessSession> {
        (arb_harness_kind(), "[a-z0-9]{1,16}").prop_map(|(harness, session)| HarnessSession {
            harness,
            session: NativeSessionId::from(session),
        })
    }

    proptest! {
        #[test]
        fn harness_session_roundtrips(hs in arb_harness_session()) {
            let pb: pb::HarnessSession = hs.clone().into();
            prop_assert_eq!(HarnessSession::try_from(pb).unwrap(), hs);
        }
    }

    #[rstest]
    #[case(StopReason::StopSequence, pb::StopReason::EndTurn)]
    #[case(StopReason::Refusal, pb::StopReason::EndTurn)]
    #[case(StopReason::Other("x".into()), pb::StopReason::Unknown)]
    fn stop_reason_coalesces_at_edge(#[case] from: StopReason, #[case] want: pb::StopReason) {
        assert_eq!(pb::StopReason::from(from), want);
    }

    #[rstest]
    fn tool_result_string_output_is_emitted_raw() {
        use atuin_common::harnesstools::session::{ToolCallId, ToolResult};

        let raw: pb::ContentBlock = Content::ToolResult(ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output: serde_json::Value::String("line1\nline2".to_owned()),
            error: false,
        })
        .into();
        let pb::content_block::Block::ToolResult(tr) = raw.block.unwrap() else {
            panic!("expected a tool result block");
        };
        assert_eq!(tr.content, "line1\nline2");

        let structured: pb::ContentBlock = Content::ToolResult(ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output: serde_json::json!({"exit": 0}),
            error: false,
        })
        .into();
        let pb::content_block::Block::ToolResult(tr) = structured.block.unwrap() else {
            panic!("expected a tool result block");
        };
        assert_eq!(tr.content, r#"{"exit":0}"#);
    }

    #[rstest]
    fn usage_none_becomes_zero() {
        let t: pb::Tokens = Usage {
            input: None,
            output: Some(3),
            cache_read: None,
            cache_write: None,
        }
        .into();
        assert_eq!((t.input, t.output, t.cache_read, t.cache_write), (0, 3, 0, 0));
    }
}
