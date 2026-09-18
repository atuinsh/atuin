use std::path::{Path, PathBuf};

use futures::{Stream, TryStreamExt};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::NodeContext;
use crate::harnesstools::ccode::Ccode;
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
pub struct CcodeSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
}

impl CcodeSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| {
            env_nonempty("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().join(".claude"))
                .join("projects")
        })
    }
}

impl Sessions for CcodeSessions {
    type Listener = CcodeListener;

    fn listener(&self) -> Result<CcodeListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(CcodeListener { root })
    }
}

impl Observable for Ccode {
    type Sessions = CcodeSessions;

    fn sessions(&self) -> CcodeSessions {
        CcodeSessions::builder().build()
    }
}

#[derive(Debug, Clone)]
pub struct CcodeListener {
    root: PathBuf,
}

impl CcodeListener {
    fn accept(ctx: &NodeContext) -> Option<CcodeSession> {
        let path = ctx.path();
        if !ctx.is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
            return None;
        }
        let id = path.file_stem()?.to_string_lossy().into_owned();
        Some(CcodeSession::open(SessionId::from(id), path.to_path_buf()))
    }
}

impl Listener for CcodeListener {
    type Session = CcodeSession;

    fn watch(self) -> impl Stream<Item = Result<CcodeSession, WatchError>> + Send + 'static {
        let _ = (&self.root, CcodeListener::accept);
        futures::stream::empty()
    }
}

#[derive(Debug, Clone)]
pub struct CcodeSession {
    id: SessionId,
    path: PathBuf,
}

impl CcodeSession {
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf) -> Self {
        Self { id, path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Session for CcodeSession {
    type Message = CcodeMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages(self) -> impl Stream<Item = Result<CcodeMessage, MessageError>> + Send + 'static {
        jsonl::tail::from_path::<CcodeMessage>(self.path).map_err(MessageError::from)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CcodeMessage {
    #[serde(rename = "type")]
    kind: String,
    uuid: Option<String>,
    timestamp: Option<String>,
    message: Option<serde_json::Value>,
    content: Option<serde_json::Value>,
}

impl CcodeMessage {
    fn block(value: &serde_json::Value) -> Content {
        match value["type"].as_str() {
            Some("text") => Content::Text(value["text"].as_str().unwrap_or_default().to_owned()),
            Some("thinking") => {
                Content::Reasoning(value["thinking"].as_str().unwrap_or_default().to_owned())
            }
            Some("tool_use") => Content::ToolUse(ToolUse {
                id: ToolCallId::from(value["id"].as_str().unwrap_or_default().to_owned()),
                name: value["name"].as_str().unwrap_or_default().to_owned(),
                input: value["input"].clone(),
            }),
            Some("tool_result") => Content::ToolResult(ToolResult {
                call: ToolCallId::from(
                    value["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                ),
                output: value["content"].clone(),
                error: value["is_error"].as_bool().unwrap_or(false),
            }),
            _ => Content::Other(value.clone()),
        }
    }
}

impl Message for CcodeMessage {
    fn id(&self) -> Option<MessageId> {
        self.uuid.clone().map(MessageId::from)
    }

    fn role(&self) -> Role {
        let role =
            self.message.as_ref().and_then(|m| m["role"].as_str()).unwrap_or(self.kind.as_str());
        match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "tool" => Role::Tool,
            other => Role::Other(other.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    }

    fn content(&self) -> Vec<Content> {
        let raw = self.message.as_ref().map(|m| &m["content"]).or(self.content.as_ref());
        match raw {
            Some(serde_json::Value::String(text)) => vec![Content::Text(text.clone())],
            Some(serde_json::Value::Array(blocks)) => {
                blocks.iter().map(CcodeMessage::block).collect()
            }
            _ => Vec::new(),
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

    fn line(kind: &str, role: &str, content: serde_json::Value) -> String {
        serde_json::json!({
            "type": kind,
            "sessionId": "11111111-1111-1111-1111-111111111111",
            "uuid": "aaaa",
            "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": role, "content": content},
        })
        .to_string()
    }

    #[rstest]
    fn normalizes_a_user_string_message() {
        let raw = line("user", "user", serde_json::json!("hi there"));
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![Content::Text("hi there".into())]);
    }

    #[rstest]
    fn normalizes_assistant_tool_use_blocks() {
        let raw = line(
            "assistant",
            "assistant",
            serde_json::json!([
                {"type": "text", "text": "running"},
                {"type": "tool_use", "id": "call_1", "name": "Bash", "input": {"cmd": "ls"}},
            ]),
        );
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::Assistant);
        let content = m.content();
        assert_eq!(content.len(), 2);
        assert!(matches!(content[1], Content::ToolUse(_)));
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_root() {
        let sessions = CcodeSessions::builder().root(PathBuf::from("/no/such/claude")).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    fn listener_opens_an_existing_root() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = CcodeSessions::builder().root(dir.path().to_path_buf()).build();
        assert!(sessions.listener().is_ok());
    }

    #[rstest]
    #[tokio::test]
    async fn messages_streams_each_turn_of_a_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let body = [
            line("user", "user", serde_json::json!("first")),
            line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "second"}])),
        ]
        .join("\n");
        std::fs::write(&path, body).unwrap();

        let session = CcodeSession::open(
            SessionId::from("11111111-1111-1111-1111-111111111111".to_owned()),
            path,
        );
        let got: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(got, vec![Role::User, Role::Assistant]);
    }

    #[rstest]
    #[tokio::test]
    #[ignore = "pending TreeWatcher->stream bridge; watch() is a placeholder in this draft"]
    async fn watch_emits_sessions_as_files_appear() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("project-a");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("22222222-2222-2222-2222-222222222222.jsonl"),
            line("user", "user", serde_json::json!("hi")),
        )
        .unwrap();

        let listener =
            CcodeSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        let seen: Vec<SessionId> =
            listener.watch().take(1).map_ok(|s| s.id()).try_collect().await.unwrap();
        assert_eq!(seen.len(), 1);
    }

    #[rstest]
    #[case(include_str!("../../../res/harnesstools/fixtures/ccode/session1.jsonl"))]
    #[case(include_str!("../../../res/harnesstools/fixtures/ccode/session2.jsonl"))]
    #[case(include_str!("../../../res/harnesstools/fixtures/ccode/session3.jsonl"))]
    fn normalizes_a_real_redacted_session(#[case] jsonl: &str) {
        let msgs: Vec<CcodeMessage> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<CcodeMessage>(l).expect("fixture record parses"))
            .collect();
        assert!(msgs.len() >= 20);

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
                        assert!(!u.name.is_empty(), "tool_use normalized to an empty name");
                        assert!(!u.id.to_string().is_empty(), "tool_use normalized to an empty id");
                        tool_uses += 1;
                    }
                    Content::ToolResult(r) => {
                        assert!(!r.call.to_string().is_empty(), "tool_result lost its call id");
                        tool_results += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(saw_user && saw_assistant, "expected both user and assistant turns");
        assert!(tool_uses >= 1, "expected at least one normalized tool_use");
        assert!(tool_results >= 1, "expected at least one normalized tool_result");
    }
}
