use std::path::{Path, PathBuf};

use futures::{Stream, TryStreamExt};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{NodeContext, TreeWatcher};
use crate::harnesstools::codex::Codex;
use crate::harnesstools::session::model::{
    Content, MessageId, Role, ToolCallId, ToolResult, ToolUse,
};
use crate::harnesstools::session::{
    Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId, Sessions,
    WatchError,
};
use crate::json::jsonl;
use crate::utils::{env_nonempty, home_dir};

#[derive(Debug, Clone, TypedBuilder)]
pub struct CodexSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
}

impl CodexSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| {
            env_nonempty("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().join(".codex"))
                .join("sessions")
        })
    }
}

impl Sessions for CodexSessions {
    type Listener = CodexListener;

    fn listener(&self) -> Result<CodexListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(CodexListener { root })
    }
}

impl Observable for Codex {
    type Sessions = CodexSessions;

    fn sessions(&self) -> CodexSessions {
        CodexSessions::builder().build()
    }
}

#[derive(Debug, Clone)]
pub struct CodexListener {
    root: PathBuf,
}

impl CodexListener {
    fn accept(ctx: &NodeContext) -> Option<CodexSession> {
        let path = ctx.path();
        let name = path.file_name()?.to_string_lossy();
        if !ctx.is_file() || !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
            return None;
        }
        let stem = path.file_stem()?.to_string_lossy();
        let mut groups: Vec<&str> = stem.rsplitn(6, '-').collect();
        groups.truncate(5);
        groups.reverse();
        let id = groups.join("-");
        Some(CodexSession::open(SessionId::from(id), path.to_path_buf()))
    }
}

impl Listener for CodexListener {
    type Session = CodexSession;

    fn watch(self) -> impl Stream<Item = Result<CodexSession, WatchError>> + Send + 'static {
        let root = self.root;
        async_stream::stream! {
            let (tx, rx) = flume::unbounded::<CodexSession>();
            let _watcher = match TreeWatcher::builder().recursive(true).watch(&root, move |ctx| {
                if let Some(session) = Self::accept(&ctx) {
                    let _ = tx.send(session);
                }
                None::<()>
            }) {
                Ok(watcher) => watcher,
                Err(err) => {
                    yield Err(WatchError::from(err));
                    return;
                }
            };
            while let Ok(session) = rx.recv_async().await {
                yield Ok(session);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexSession {
    id: SessionId,
    path: PathBuf,
}

impl CodexSession {
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf) -> Self {
        Self { id, path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Session for CodexSession {
    type Message = CodexMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages(self) -> impl Stream<Item = Result<CodexMessage, MessageError>> + Send + 'static {
        jsonl::tail::from_path::<CodexMessage>(self.path).map_err(MessageError::from)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodexMessage {
    #[serde(rename = "type")]
    kind: String,
    timestamp: Option<String>,
    payload: Option<serde_json::Value>,
}

impl CodexMessage {
    fn block(value: &serde_json::Value) -> Content {
        match value["type"].as_str() {
            Some("input_text" | "output_text" | "text") => {
                Content::Text(value["text"].as_str().unwrap_or_default().to_owned())
            }
            _ => Content::Other(value.clone()),
        }
    }
}

impl Message for CodexMessage {
    fn id(&self) -> Option<MessageId> {
        self.payload
            .as_ref()
            .and_then(|p| p["call_id"].as_str().or_else(|| p["id"].as_str()))
            .map(|s| MessageId::from(s.to_owned()))
    }

    fn role(&self) -> Role {
        let payload = self.payload.as_ref();
        match payload.and_then(|p| p["type"].as_str()) {
            Some("function_call" | "custom_tool_call") => Role::Assistant,
            Some("function_call_output" | "custom_tool_call_output") => Role::Tool,
            _ => match payload.and_then(|p| p["role"].as_str()).unwrap_or(self.kind.as_str()) {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "system" => Role::System,
                "tool" => Role::Tool,
                other => Role::Other(other.to_owned()),
            },
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    }

    fn content(&self) -> Vec<Content> {
        let Some(payload) = self.payload.as_ref() else {
            return Vec::new();
        };
        match payload["type"].as_str() {
            Some("function_call") => vec![Content::ToolUse(ToolUse {
                id: ToolCallId::from(payload["call_id"].as_str().unwrap_or_default().to_owned()),
                name: payload["name"].as_str().unwrap_or_default().to_owned(),
                input: payload["arguments"].clone(),
            })],
            Some("custom_tool_call") => vec![Content::ToolUse(ToolUse {
                id: ToolCallId::from(payload["call_id"].as_str().unwrap_or_default().to_owned()),
                name: payload["name"].as_str().unwrap_or_default().to_owned(),
                input: payload["input"].clone(),
            })],
            Some("function_call_output" | "custom_tool_call_output") => {
                vec![Content::ToolResult(ToolResult {
                    call: ToolCallId::from(
                        payload["call_id"].as_str().unwrap_or_default().to_owned(),
                    ),
                    output: payload["output"].clone(),
                    error: false,
                })]
            }
            _ => match &payload["content"] {
                serde_json::Value::Array(blocks) => blocks.iter().map(Self::block).collect(),
                serde_json::Value::String(text) => vec![Content::Text(text.clone())],
                _ => Vec::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use futures::{StreamExt, TryStreamExt};
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::session::model::{Content, Role};
    use crate::harnesstools::session::{Message, Session, Sessions};

    #[rstest]
    fn normalizes_a_codex_assistant_message() {
        let raw = serde_json::json!({
            "timestamp": "2026-09-18T10:00:00Z",
            "type": "message",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "done"}],
            },
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::Assistant);
        assert_eq!(m.content(), vec![Content::Text("done".into())]);
    }

    #[rstest]
    fn normalizes_a_codex_function_call() {
        let raw = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "shell",
                "arguments": "{\"cmd\":\"ls\"}",
                "call_id": "c1",
            },
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert!(matches!(m.content().as_slice(), [Content::ToolUse(_)]));
    }

    #[rstest]
    fn normalizes_a_codex_custom_tool_call() {
        let call = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call", "name": "shell", "input": "ls", "call_id": "c1"},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&call).unwrap();
        assert_eq!(m.role(), Role::Assistant);
        assert!(
            matches!(m.content().as_slice(), [Content::ToolUse(u)] if u.id.to_string() == "c1")
        );

        let output = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": "files"},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&output).unwrap();
        assert_eq!(m.role(), Role::Tool);
        assert!(
            matches!(m.content().as_slice(), [Content::ToolResult(r)] if r.call.to_string() == "c1")
        );
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_root() {
        let sessions = CodexSessions::builder().root(PathBuf::from("/no/such/codex")).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    #[tokio::test]
    async fn messages_streams_turns_from_a_rollout_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-2026-09-18-th1.jsonl");
        let body = [
            serde_json::json!({"type": "session_meta", "payload": {"id": "th1"}}).to_string(),
            serde_json::json!({
                "type": "message",
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            })
            .to_string(),
        ]
        .join("\n");
        std::fs::write(&path, body).unwrap();

        let session = CodexSession::open(SessionId::from("th1".to_owned()), path);
        let roles: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(roles.last(), Some(&Role::User));
    }

    #[rstest]
    #[tokio::test]
    async fn watch_emits_sessions_as_files_appear() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("2026").join("09").join("19");
        std::fs::create_dir_all(&sub).unwrap();
        let sid = "0a1b2c3d-4e5f-6789-abcd-ef0123456789";
        std::fs::write(
            sub.join(format!("rollout-2026-09-19T00-00-00-{sid}.jsonl")),
            serde_json::json!({"type": "session_meta", "payload": {"id": sid}}).to_string(),
        )
        .unwrap();

        let listener =
            CodexSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        let seen: Vec<SessionId> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            listener.watch().take(1).map_ok(|s| s.id()).try_collect(),
        )
        .await
        .expect("watch() did not emit a session within 10s")
        .unwrap();
        assert_eq!(seen, vec![SessionId::from(sid.to_owned())]);
    }

    #[rstest]
    #[case(include_str!("../../../tests/fixtures/codex/session1.jsonl"))]
    fn normalizes_a_real_redacted_session(#[case] jsonl: &str) {
        let msgs: Vec<CodexMessage> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<CodexMessage>(l).expect("fixture record parses"))
            .collect();
        assert!(msgs.len() >= 15);

        let mut saw_user = false;
        let mut saw_assistant = false;
        let mut tool_uses = 0usize;
        let mut tool_results = 0usize;
        for m in &msgs {
            let _ = m.timestamp();
            saw_user |= m.role() == Role::User;
            saw_assistant |= m.role() == Role::Assistant;
            for c in m.content() {
                match c {
                    Content::ToolUse(u) => {
                        assert!(!u.name.is_empty(), "custom_tool_call normalized to an empty name");
                        assert!(!u.id.to_string().is_empty(), "tool call lost its id");
                        tool_uses += 1;
                    }
                    Content::ToolResult(r) => {
                        assert!(!r.call.to_string().is_empty(), "tool result lost its call id");
                        tool_results += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(saw_user && saw_assistant, "expected both user and assistant turns");
        assert!(tool_uses >= 1, "expected at least one normalized tool call");
        assert!(tool_results >= 1, "expected at least one normalized tool result");
    }
}
