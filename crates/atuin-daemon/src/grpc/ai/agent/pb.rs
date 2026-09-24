//! Model conversion utilities for the `ai.agent` gRPC protobuf.
//!
//! The wire is lossless: a domain value survives `domain -> wire -> domain` unchanged, so how to
//! render it is the client's call. The exceptions are `Message::session_title` and
//! `Message::turn_id`, storage bookkeeping the wire never carries, and a non-UTF-8 `cwd`, which is
//! sent lossily.
mod codegen {
    #![allow(clippy::must_use_candidate)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("ai.agent");
}

use std::borrow::Cow;
use std::path::PathBuf;

use atuin_client::ai_session::{
    HarnessKind, HarnessSession as DomainHarnessSession, Message as DomainMessage, NativeSessionId,
    Session as DomainSession, SourceId,
};
use atuin_common::harnesstools::session::{
    Content, Role as DomainRole, StopReason as DomainStopReason, ToolCallId,
    ToolResult as DomainToolResult, ToolUse, Usage,
};
use atuin_common::time::{OffsetDateTimeExt, TimespecOutOfRange};
use atuin_domain::record::RecordId;
pub use codegen::*;
use thiserror::Error;
use time::OffsetDateTime;

use crate::grpc::common::pb::Uuid;

/// Errors decoding an `ai.agent` wire type into its domain type.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("unrecognized harness kind: {0}")]
    UnknownHarnessKind(i32),
    #[error("unrecognized role: {0}")]
    UnknownRole(i32),
    #[error("unrecognized stop reason: {0}")]
    UnknownStopReason(i32),
    #[error("missing {0}")]
    Missing(&'static str),
    #[error("invalid id: {0}")]
    InvalidId(#[from] uuid::Error),
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(#[from] TimespecOutOfRange),
    #[error("invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

impl From<DomainHarnessSession> for HarnessSession {
    fn from(value: DomainHarnessSession) -> Self {
        Self {
            harness: value.harness as i32,
            session_id: value.session.into(),
        }
    }
}

impl TryFrom<HarnessSession> for DomainHarnessSession {
    type Error = ParseError;

    fn try_from(value: HarnessSession) -> Result<Self, Self::Error> {
        let harness = HarnessKind::try_from(value.harness)
            .map_err(|_| ParseError::UnknownHarnessKind(value.harness))?;
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
            DomainRole::Other(_) => Self::Other,
        }
    }
}

impl From<DomainStopReason> for StopReason {
    fn from(value: DomainStopReason) -> Self {
        match value {
            DomainStopReason::EndTurn => Self::EndTurn,
            DomainStopReason::MaxTokens => Self::MaxTokens,
            DomainStopReason::ToolUse => Self::ToolUse,
            DomainStopReason::StopSequence => Self::StopSequence,
            DomainStopReason::Refusal => Self::Refusal,
            DomainStopReason::Aborted => Self::Aborted,
            DomainStopReason::Error => Self::Error,
            DomainStopReason::Other(_) => Self::Other,
        }
    }
}

impl From<Usage> for Tokens {
    fn from(value: Usage) -> Self {
        Self {
            input: value.input,
            output: value.output,
            cache_read: value.cache_read,
            cache_write: value.cache_write,
        }
    }
}

impl From<Tokens> for Usage {
    fn from(value: Tokens) -> Self {
        Self {
            input: value.input,
            output: value.output,
            cache_read: value.cache_read,
            cache_write: value.cache_write,
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
                Block::ReasoningSummary(ReasoningSummary { tokens })
            }
            // Capture stores a JSON null in place of arguments and results it does not keep; on
            // the wire that is an absent field.
            Content::ToolUse(tu) => Block::ToolCall(ToolCall {
                id: tu.id.into(),
                name: tu.name,
                input: (!tu.input.is_null()).then(|| tu.input.to_string()),
            }),
            Content::ToolResult(tr) => Block::ToolResult(ToolResult {
                tool_use_id: tr.call.into(),
                output: (!tr.output.is_null()).then(|| tr.output.to_string()),
                is_error: tr.error,
            }),
            Content::Other(v) => Block::Other(v.to_string()),
        };

        Self { block: Some(block) }
    }
}

impl TryFrom<ContentBlock> for Content {
    type Error = ParseError;

    fn try_from(value: ContentBlock) -> Result<Self, Self::Error> {
        use content_block::Block;

        let captured = |json: Option<String>| {
            json.map_or(Ok(serde_json::Value::Null), |json| serde_json::from_str(&json))
        };

        Ok(match value.block.ok_or(ParseError::Missing("content block"))? {
            Block::Text(text) => Self::Text(text),
            Block::Thinking(text) => Self::Reasoning(text),
            Block::ReasoningSummary(summary) => Self::ReasoningSummary {
                tokens: summary.tokens,
            },
            Block::ToolCall(tc) => Self::ToolUse(ToolUse {
                id: ToolCallId::from(tc.id),
                name: tc.name,
                input: captured(tc.input)?,
            }),
            Block::ToolResult(tr) => Self::ToolResult(DomainToolResult {
                call: ToolCallId::from(tr.tool_use_id),
                output: captured(tr.output)?,
                error: tr.is_error,
            }),
            Block::Other(json) => Self::Other(serde_json::from_str(&json)?),
        })
    }
}

impl ToolResult {
    /// The output as display text: a JSON string's contents verbatim, any other JSON as encoded,
    /// or `None` when capture did not keep it.
    #[must_use]
    pub fn output_text(&self) -> Option<Cow<'_, str>> {
        let json = self.output.as_deref()?;
        Some(serde_json::from_str::<String>(json).map_or(Cow::Borrowed(json), Cow::Owned))
    }
}

impl From<DomainMessage> for Message {
    fn from(value: DomainMessage) -> Self {
        let role_label = match &value.role {
            DomainRole::Other(other) => Some(other.clone()),
            _ => None,
        };
        let stop_reason_label = match &value.stop_reason {
            Some(DomainStopReason::Other(other)) => Some(other.clone()),
            _ => None,
        };
        Self {
            id: Some(Uuid {
                value: value.id.0.into_bytes().to_vec(),
            }),
            harness: value.session.harness as i32,
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
            content: value.content.into_iter().map(ContentBlock::from).collect(),
            cwd: value.cwd.map(|path| path.to_string_lossy().into_owned()),
            git_branch: value.git_branch,
            model: value.model,
            tokens: value.usage.map(Tokens::from),
            stop_reason: value.stop_reason.map(|reason| StopReason::from(reason) as i32),
            role_label,
            stop_reason_label,
        }
    }
}

impl Message {
    fn domain_role(&self) -> Result<DomainRole, ParseError> {
        Ok(match Role::try_from(self.role).map_err(|_| ParseError::UnknownRole(self.role))? {
            Role::User => DomainRole::User,
            Role::Assistant => DomainRole::Assistant,
            Role::System => DomainRole::System,
            Role::Tool => DomainRole::Tool,
            Role::Other => {
                DomainRole::Other(self.role_label.clone().ok_or(ParseError::Missing("role_label"))?)
            }
        })
    }

    fn domain_stop_reason(&self) -> Result<Option<DomainStopReason>, ParseError> {
        let Some(raw) = self.stop_reason else {
            return Ok(None);
        };
        let stop_reason =
            StopReason::try_from(raw).map_err(|_| ParseError::UnknownStopReason(raw))?;
        Ok(Some(match stop_reason {
            StopReason::EndTurn => DomainStopReason::EndTurn,
            StopReason::ToolUse => DomainStopReason::ToolUse,
            StopReason::MaxTokens => DomainStopReason::MaxTokens,
            StopReason::Aborted => DomainStopReason::Aborted,
            StopReason::Error => DomainStopReason::Error,
            StopReason::StopSequence => DomainStopReason::StopSequence,
            StopReason::Refusal => DomainStopReason::Refusal,
            StopReason::Other => DomainStopReason::Other(
                self.stop_reason_label.clone().ok_or(ParseError::Missing("stop_reason_label"))?,
            ),
        }))
    }
}

impl TryFrom<Message> for DomainMessage {
    type Error = ParseError;

    fn try_from(value: Message) -> Result<Self, Self::Error> {
        let role = value.domain_role()?;
        let stop_reason = value.domain_stop_reason()?;
        let id = value.id.ok_or(ParseError::Missing("id"))?;
        let timestamp = value.timestamp.ok_or(ParseError::Missing("timestamp"))?;
        let session = HarnessSession {
            harness: value.harness,
            session_id: value.session_id,
        };
        Ok(Self {
            id: RecordId(uuid::Uuid::from_slice(&id.value)?),
            session: session.try_into()?,
            source_id: SourceId::from(value.source_id),
            parent: value.parent.map(TryInto::try_into).transpose()?,
            parent_source_id: value.parent_source_id.map(SourceId::from),
            thread: value.thread,
            timestamp: OffsetDateTime::from_timespec(
                timestamp.seconds.into(),
                timestamp.nanos.into(),
            )?,
            role,
            content: value.content.into_iter().map(Content::try_from).collect::<Result<_, _>>()?,
            cwd: value.cwd.map(PathBuf::from),
            git_branch: value.git_branch,
            model: value.model,
            usage: value.tokens.map(Usage::from),
            stop_reason,
            session_title: None,
            turn_id: None,
        })
    }
}

impl From<DomainSession> for Session {
    fn from(value: DomainSession) -> Self {
        Self {
            harness: value.handle.harness as i32,
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

impl TryFrom<Session> for DomainSession {
    type Error = ParseError;

    fn try_from(value: Session) -> Result<Self, Self::Error> {
        let at = |timestamp: Option<prost_types::Timestamp>, field| {
            let timestamp = timestamp.ok_or(ParseError::Missing(field))?;
            Ok::<_, ParseError>(OffsetDateTime::from_timespec(
                timestamp.seconds.into(),
                timestamp.nanos.into(),
            )?)
        };
        let handle = HarnessSession {
            harness: value.harness,
            session_id: value.session_id,
        };
        Ok(Self {
            handle: handle.try_into()?,
            parent: value.parent.map(TryInto::try_into).transpose()?,
            cwd: value.cwd.map(PathBuf::from),
            git_branch: value.git_branch,
            model: value.model,
            started_at: at(value.started_at, "started_at")?,
            updated_at: at(value.updated_at, "updated_at")?,
            message_count: value.message_count,
            usage: value.tokens.ok_or(ParseError::Missing("tokens"))?.into(),
            title: value.title,
            preview: value.preview,
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use serde_json::{Value, json};

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

    fn arb_harness_session() -> impl Strategy<Value = DomainHarnessSession> {
        (arb_harness_kind(), "[a-z0-9]{1,16}").prop_map(|(harness, session)| DomainHarnessSession {
            harness,
            session: NativeSessionId::from(session),
        })
    }

    fn arb_timestamp() -> impl Strategy<Value = OffsetDateTime> {
        // Roughly ±6000 years around the epoch, inside what `OffsetDateTime` represents.
        (-190_000_000_000i64..190_000_000_000, 0i32..1_000_000_000).prop_map(|(secs, nanos)| {
            OffsetDateTime::from_timespec(secs.into(), nanos.into()).unwrap()
        })
    }

    fn arb_usage() -> impl Strategy<Value = Usage> {
        any::<[Option<u64>; 4]>().prop_map(|[input, output, cache_read, cache_write]| Usage {
            input,
            output,
            cache_read,
            cache_write,
        })
    }

    fn arb_role() -> impl Strategy<Value = DomainRole> {
        prop_oneof![
            Just(DomainRole::User),
            Just(DomainRole::Assistant),
            Just(DomainRole::System),
            Just(DomainRole::Tool),
            ".{0,8}".prop_map(DomainRole::Other),
        ]
    }

    fn arb_stop_reason() -> impl Strategy<Value = DomainStopReason> {
        prop_oneof![
            Just(DomainStopReason::EndTurn),
            Just(DomainStopReason::MaxTokens),
            Just(DomainStopReason::ToolUse),
            Just(DomainStopReason::StopSequence),
            Just(DomainStopReason::Refusal),
            Just(DomainStopReason::Aborted),
            Just(DomainStopReason::Error),
            ".{0,8}".prop_map(DomainStopReason::Other),
        ]
    }

    /// No floats: `serde_json` without `float_roundtrip` may parse a float back one ULP off.
    fn arb_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::from),
            any::<i64>().prop_map(Value::from),
            ".{0,8}".prop_map(Value::from),
        ];
        leaf.prop_recursive(2, 8, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
                prop::collection::btree_map("[a-z]{1,4}", inner, 0..4)
                    .prop_map(|map| Value::Object(map.into_iter().collect())),
            ]
        })
    }

    fn arb_content() -> impl Strategy<Value = Content> {
        prop_oneof![
            ".{0,8}".prop_map(Content::Text),
            ".{0,8}".prop_map(Content::Reasoning),
            any::<Option<u64>>().prop_map(|tokens| Content::ReasoningSummary { tokens }),
            ("[a-z0-9]{1,8}", "[a-z]{1,8}", arb_json()).prop_map(|(id, name, input)| {
                Content::ToolUse(ToolUse {
                    id: ToolCallId::from(id),
                    name,
                    input,
                })
            }),
            ("[a-z0-9]{1,8}", arb_json(), any::<bool>()).prop_map(|(call, output, error)| {
                Content::ToolResult(DomainToolResult {
                    call: ToolCallId::from(call),
                    output,
                    error,
                })
            }),
            arb_json().prop_map(Content::Other),
        ]
    }

    fn arb_message() -> impl Strategy<Value = DomainMessage> {
        let handles = (
            any::<u128>(),
            arb_harness_session(),
            "[a-z0-9]{1,8}",
            prop::option::of(arb_harness_session()),
            prop::option::of("[a-z0-9]{1,8}"),
            prop::option::of("[a-z0-9]{1,8}"),
        );
        let body = (
            arb_timestamp(),
            arb_role(),
            prop::collection::vec(arb_content(), 0..4),
            prop::option::of("[a-z/]{1,16}"),
            prop::option::of("[a-z]{1,8}"),
            prop::option::of("[a-z]{1,8}"),
        );
        let outcome = (prop::option::of(arb_usage()), prop::option::of(arb_stop_reason()));
        (handles, body, outcome).prop_map(
            |(
                (id, session, source_id, parent, parent_source_id, thread),
                (timestamp, role, content, cwd, git_branch, model),
                (usage, stop_reason),
            )| DomainMessage {
                id: RecordId(uuid::Uuid::from_u128(id)),
                session,
                source_id: SourceId::from(source_id),
                parent,
                parent_source_id: parent_source_id.map(SourceId::from),
                thread,
                timestamp,
                role,
                content,
                cwd: cwd.map(PathBuf::from),
                git_branch,
                model,
                usage,
                stop_reason,
                session_title: None,
                turn_id: None,
            },
        )
    }

    fn arb_session() -> impl Strategy<Value = DomainSession> {
        let handles = (
            arb_harness_session(),
            prop::option::of(arb_harness_session()),
            prop::option::of("[a-z/]{1,16}"),
            prop::option::of("[a-z]{1,8}"),
            prop::option::of("[a-z]{1,8}"),
        );
        let summary = (
            arb_timestamp(),
            arb_timestamp(),
            any::<u64>(),
            arb_usage(),
            prop::option::of(".{0,8}"),
            prop::option::of(".{0,8}"),
        );
        (handles, summary).prop_map(
            |(
                (handle, parent, cwd, git_branch, model),
                (started_at, updated_at, message_count, usage, title, preview),
            )| DomainSession {
                handle,
                parent,
                cwd: cwd.map(PathBuf::from),
                git_branch,
                model,
                started_at,
                updated_at,
                message_count,
                usage,
                title,
                preview,
            },
        )
    }

    proptest! {
        #[test]
        fn harness_session_roundtrips(hs in arb_harness_session()) {
            let pb: HarnessSession = hs.clone().into();
            prop_assert_eq!(DomainHarnessSession::try_from(pb).unwrap(), hs);
        }

        #[test]
        fn message_roundtrips(message in arb_message()) {
            let pb = Message::from(message.clone());
            prop_assert_eq!(DomainMessage::try_from(pb).unwrap(), message);
        }

        #[test]
        fn session_roundtrips(session in arb_session()) {
            let pb = Session::from(session.clone());
            prop_assert_eq!(DomainSession::try_from(pb).unwrap(), session);
        }
    }

    /// `ai.agent.HarnessKind` is extern-pathed to the domain enum, so its discriminants are the wire
    /// values and nothing generated from `agent.proto` keeps the two in step.
    #[rstest]
    fn harness_kind_matches_the_proto_enum() {
        use std::collections::BTreeMap;

        use prost::Message as _;

        let descriptors = prost_types::FileDescriptorSet::decode(
            include_bytes!(concat!(env!("OUT_DIR"), "/file_descriptor_set.bin")).as_slice(),
        )
        .unwrap();
        let proto = descriptors
            .file
            .iter()
            .filter(|file| file.package() == "ai.agent")
            .flat_map(|file| &file.enum_type)
            .find(|e| e.name() == "HarnessKind")
            .unwrap();
        let wire: BTreeMap<i32, String> = proto
            .value
            .iter()
            .map(|v| {
                let variant = v.name().strip_prefix("HARNESS_KIND_").unwrap();
                let camel = variant.split('_').map(|w| w[..1].to_owned() + &w[1..].to_lowercase());
                (v.number(), camel.collect())
            })
            .collect();

        let domain: BTreeMap<i32, String> = (0..=i32::from(u8::MAX))
            .filter_map(|n| HarnessKind::try_from(n).ok().map(|kind| (n, format!("{kind:?}"))))
            .collect();
        assert_eq!(domain, wire);
    }

    #[rstest]
    fn uncaptured_tool_input_is_absent_on_the_wire() {
        let call: ContentBlock = Content::ToolUse(ToolUse {
            id: ToolCallId::from("c1".to_owned()),
            name: "Bash".to_owned(),
            input: Value::Null,
        })
        .into();
        let content_block::Block::ToolCall(tc) = call.block.unwrap() else {
            panic!("expected a tool call block");
        };
        assert_eq!(tc.input, None);
    }

    #[rstest]
    #[case::string(json!("line1\nline2"), Some("line1\nline2"))]
    #[case::structured(json!({"exit": 0}), Some(r#"{"exit":0}"#))]
    #[case::uncaptured(Value::Null, None)]
    fn tool_result_output_text_unwraps_json_strings(
        #[case] output: Value,
        #[case] want: Option<&str>,
    ) {
        let result: ContentBlock = Content::ToolResult(DomainToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output,
            error: false,
        })
        .into();
        let content_block::Block::ToolResult(tr) = result.block.unwrap() else {
            panic!("expected a tool result block");
        };
        assert_eq!(tr.output_text().as_deref(), want);
    }

    #[rstest]
    fn other_role_is_carried_as_role_label() {
        let msg = DomainMessage::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(DomainHarnessSession {
                harness: HarnessKind::Codex,
                session: NativeSessionId::from("s".to_owned()),
            })
            .source_id(SourceId::from("src".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(DomainRole::Other("developer".to_owned()))
            .content(vec![Content::Text("hi".to_owned())])
            .build();

        let pb = Message::from(msg);
        assert_eq!(pb.role, Role::Other as i32);
        assert_eq!(pb.role_label.as_deref(), Some("developer"));
    }
}
