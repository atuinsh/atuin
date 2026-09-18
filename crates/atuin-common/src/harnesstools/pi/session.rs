use std::path::{Path, PathBuf};

use futures::{Stream, TryStreamExt};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::NodeContext;
use crate::harnesstools::pi::Pi;
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
pub struct PiSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
}

impl PiSessions {
    fn resolve_root(&self) -> PathBuf {
        if let Some(root) = &self.root {
            return root.clone();
        }
        if let Some(dir) = env_nonempty("PI_CODING_AGENT_SESSION_DIR") {
            return PathBuf::from(dir);
        }
        if let Some(dir) = env_nonempty("PI_CODING_AGENT_DIR") {
            return PathBuf::from(dir).join("sessions");
        }
        home_dir().join(".pi").join("agent").join("sessions")
    }
}

impl Sessions for PiSessions {
    type Listener = PiListener;

    fn listener(&self) -> Result<PiListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(PiListener { root })
    }
}

impl Observable for Pi {
    type Sessions = PiSessions;

    fn sessions(&self) -> PiSessions {
        PiSessions::builder().build()
    }
}

#[derive(Debug, Clone)]
pub struct PiListener {
    root: PathBuf,
}

impl PiListener {
    fn accept(ctx: &NodeContext) -> Option<PiSession> {
        let path = ctx.path();
        if !ctx.is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
            return None;
        }
        let stem = path.file_stem()?.to_string_lossy();
        let id = stem.split_once('_').map_or(stem.as_ref(), |(_, id)| id).to_owned();
        Some(PiSession::open(SessionId::from(id), path.to_path_buf()))
    }
}

impl Listener for PiListener {
    type Session = PiSession;

    fn watch(self) -> impl Stream<Item = Result<PiSession, WatchError>> + Send + 'static {
        let _ = (&self.root, PiListener::accept);
        futures::stream::empty()
    }
}

#[derive(Debug, Clone)]
pub struct PiSession {
    id: SessionId,
    path: PathBuf,
}

impl PiSession {
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf) -> Self {
        Self { id, path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Session for PiSession {
    type Message = PiMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages(self) -> impl Stream<Item = Result<PiMessage, MessageError>> + Send + 'static {
        jsonl::tail::from_path::<PiMessage>(self.path).map_err(MessageError::from)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PiMessage {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    timestamp: Option<String>,
    message: Option<serde_json::Value>,
}

impl PiMessage {
    fn block(value: &serde_json::Value) -> Content {
        match value["type"].as_str() {
            Some("text") => Content::Text(value["text"].as_str().unwrap_or_default().to_owned()),
            Some("toolCall") => Content::ToolUse(ToolUse {
                id: ToolCallId::from(value["toolCallId"].as_str().unwrap_or_default().to_owned()),
                name: value["toolName"].as_str().unwrap_or_default().to_owned(),
                input: value["args"].clone(),
            }),
            Some("toolResult") => Content::ToolResult(ToolResult {
                call: ToolCallId::from(value["toolCallId"].as_str().unwrap_or_default().to_owned()),
                output: value["content"].clone(),
                error: false,
            }),
            _ => Content::Other(value.clone()),
        }
    }
}

impl Message for PiMessage {
    fn id(&self) -> Option<MessageId> {
        self.id.clone().map(MessageId::from)
    }

    fn role(&self) -> Role {
        let role =
            self.message.as_ref().and_then(|m| m["role"].as_str()).unwrap_or(self.kind.as_str());
        match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "toolResult" | "tool" => Role::Tool,
            other => Role::Other(other.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    }

    fn content(&self) -> Vec<Content> {
        let raw = self.message.as_ref().map(|m| &m["content"]);
        match raw {
            Some(serde_json::Value::String(text)) => vec![Content::Text(text.clone())],
            Some(serde_json::Value::Array(blocks)) => blocks.iter().map(PiMessage::block).collect(),
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

    #[rstest]
    fn normalizes_a_pi_user_message() {
        let raw = serde_json::json!({
            "type": "message",
            "id": "m1",
            "parentId": null,
            "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": "user", "content": "hello pi"},
        })
        .to_string();
        let m: PiMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![Content::Text("hello pi".into())]);
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_root() {
        let sessions = PiSessions::builder().root(PathBuf::from("/no/such/pi")).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    #[tokio::test]
    async fn messages_streams_turns_from_a_pi_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1700000000_s1.jsonl");
        let body = [
            serde_json::json!({"type": "session", "id": "s1", "cwd": "/w"}).to_string(),
            serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": "hi"},
            })
            .to_string(),
        ]
        .join("\n");
        std::fs::write(&path, body).unwrap();

        let session = PiSession::open(SessionId::from("s1".to_owned()), path);
        let roles: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(roles.last(), Some(&Role::Assistant));
    }
}
