use std::path::{Path, PathBuf};

use futures::{Stream, TryStreamExt};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{NodeContext, TreeWatcher};
use crate::harnesstools::pi::Pi;
use crate::harnesstools::session::model::{
    Content, MessageId, Role, SessionMeta, StopReason, ToolCallId, ToolResult, ToolUse, Usage,
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
    /// The session for an accepted file, paired with the change signal the watcher keeps alive
    /// for as long as the file exists.
    fn accept(ctx: &NodeContext) -> Option<(PiSession, watch::Sender<()>)> {
        let path = ctx.path();
        if !ctx.is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
            return None;
        }
        let stem = path.file_stem()?.to_string_lossy();
        let id = stem.split_once('_').map_or(stem.as_ref(), |(_, id)| id).to_owned();
        let (signal, rx) = watch::channel(());
        let session = PiSession {
            id: SessionId::from(id),
            path: path.to_path_buf(),
            changes: Some(rx),
        };
        Some((session, signal))
    }
}

impl Listener for PiListener {
    type Session = PiSession;

    fn watch(self) -> impl Stream<Item = Result<PiSession, WatchError>> + Send + 'static {
        let root = self.root;
        async_stream::stream! {
            let (tx, rx) = flume::unbounded::<PiSession>();
            let _watcher = match TreeWatcher::builder().recursive(true).watch(&root, move |ctx| {
                let (session, signal) = Self::accept(&ctx)?;
                let _ = tx.send(session);
                Some(signal)
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
pub struct PiSession {
    id: SessionId,
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the file; `None` reads it once.
    changes: Option<watch::Receiver<()>>,
}

impl PiSession {
    /// A session over the file as it stands: [`messages`](Session::messages) ends at its end.
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf) -> Self {
        Self {
            id,
            path,
            changes: None,
        }
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
        jsonl::follow::<PiMessage>(self.path, self.changes).map_err(MessageError::from)
    }

    fn meta(&self) -> impl std::future::Future<Output = Result<SessionMeta, MessageError>> + Send {
        let path = self.path.clone();
        async move {
            let messages: Vec<PiMessage> =
                jsonl::read_all(path).map_err(MessageError::from).try_collect().await?;
            let cwd = messages.iter().find_map(|m| m.cwd.clone());
            let model = messages.iter().rev().find_map(|m| m.model_id.clone());
            let parent = messages
                .iter()
                .find(|m| m.kind == "session")
                .and_then(|m| m.parent_session.clone())
                .map(SessionId::from);
            Ok(SessionMeta {
                cwd,
                model,
                parent,
                ..SessionMeta::default()
            })
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PiMessage {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    #[serde(rename = "parentId")]
    parent_id: Option<String>,
    timestamp: Option<String>,
    message: Option<serde_json::Value>,
    cwd: Option<PathBuf>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    /// On `session_info`: a name the user gave the session.
    name: Option<String>,
    /// On the `session` header: the session this one was forked from.
    #[serde(rename = "parentSession")]
    parent_session: Option<String>,
}

impl PiMessage {
    fn block(value: &serde_json::Value) -> Content {
        match value["type"].as_str() {
            Some("text") => Content::Text(value["text"].as_str().unwrap_or_default().to_owned()),
            Some("toolCall") => Content::ToolUse(ToolUse {
                id: ToolCallId::from(value["id"].as_str().unwrap_or_default().to_owned()),
                name: value["name"].as_str().unwrap_or_default().to_owned(),
                input: value["arguments"].clone(),
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
            "bashExecution" => Role::User,
            other => Role::Other(other.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    }

    fn content(&self) -> Vec<Content> {
        let Some(message) = self.message.as_ref() else {
            return Vec::new();
        };
        if message["role"].as_str() == Some("toolResult") {
            return vec![Content::ToolResult(ToolResult {
                call: ToolCallId::from(
                    message["toolCallId"].as_str().unwrap_or_default().to_owned(),
                ),
                output: message["content"].clone(),
                error: message["isError"].as_bool().unwrap_or(false),
            })];
        }
        // A `!command` the user ran in pi's own shell, with what it printed.
        if message["role"].as_str() == Some("bashExecution") {
            return vec![
                Content::Text(format!("!{}", message["command"].as_str().unwrap_or_default())),
                Content::ToolResult(ToolResult {
                    call: ToolCallId::from(self.id.clone().unwrap_or_default()),
                    output: message["output"].clone(),
                    error: message["exitCode"].as_i64().is_some_and(|c| c != 0),
                }),
            ];
        }
        match &message["content"] {
            serde_json::Value::String(text) => vec![Content::Text(text.clone())],
            serde_json::Value::Array(blocks) => blocks.iter().map(Self::block).collect(),
            _ => Vec::new(),
        }
    }

    fn model(&self) -> Option<String> {
        self.message.as_ref()?.get("model")?.as_str().map(str::to_owned)
    }

    fn usage(&self) -> Option<Usage> {
        let usage = self.message.as_ref()?.get("usage")?;
        if usage.is_null() {
            return None;
        }
        Some(Usage {
            input: usage.get("input").and_then(serde_json::Value::as_u64),
            output: usage.get("output").and_then(serde_json::Value::as_u64),
            cache_read: usage.get("cacheRead").and_then(serde_json::Value::as_u64),
            cache_write: usage.get("cacheWrite").and_then(serde_json::Value::as_u64),
        })
    }

    fn stop_reason(&self) -> Option<StopReason> {
        let raw = self.message.as_ref()?.get("stopReason")?.as_str()?;
        Some(match raw {
            "stop" => StopReason::EndTurn,
            "toolUse" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            "aborted" => StopReason::Aborted,
            "error" => StopReason::Error,
            other => StopReason::Other(other.to_owned()),
        })
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent_id.clone().map(MessageId::from)
    }

    fn title(&self) -> Option<String> {
        (self.kind == "session_info").then(|| self.name.clone()).flatten()
    }

    fn turn_id(&self) -> Option<String> {
        self.message.as_ref()?.get("responseId")?.as_str().map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use futures::{StreamExt, TryStreamExt};
    use rstest::rstest;

    use super::*;
    use crate::futures::stream::timed_next;
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
    fn normalizes_pi_tool_call_and_result() {
        let call: PiMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": [
                    {"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"cmd": "ls"}}
                ]},
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            matches!(call.content().as_slice(), [Content::ToolUse(u)] if u.name == "bash" && u.id.as_ref() == "c1")
        );

        let result: PiMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "message",
                "id": "m2",
                "message": {"role": "toolResult", "toolCallId": "c1", "isError": false, "content": "done"},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(result.role(), Role::Tool);
        assert!(
            matches!(result.content().as_slice(), [Content::ToolResult(r)] if r.call.as_ref() == "c1" && !r.error)
        );
    }

    #[rstest]
    fn normalizes_pi_assistant_enrichment_fields() {
        let raw = serde_json::json!({
            "type": "message",
            "id": "m1",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "model": "claude-opus-4-8",
                "stopReason": "stop",
                "usage": {"input": 10, "output": 20, "cacheRead": 5, "cacheWrite": 2},
            },
        })
        .to_string();
        let m: PiMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), Some("claude-opus-4-8".to_owned()));
        assert_eq!(m.stop_reason(), Some(StopReason::EndTurn));
        assert_eq!(
            m.usage(),
            Some(Usage {
                input: Some(10),
                output: Some(20),
                cache_read: Some(5),
                cache_write: Some(2)
            })
        );
    }

    #[rstest]
    #[case("toolUse", StopReason::ToolUse)]
    #[case("aborted", StopReason::Aborted)]
    #[case("length", StopReason::MaxTokens)]
    #[case("error", StopReason::Error)]
    #[case("weird", StopReason::Other("weird".to_owned()))]
    fn maps_pi_stop_reason_vocabulary(#[case] raw: &str, #[case] expected: StopReason) {
        let m: PiMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": [], "stopReason": raw},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.stop_reason(), Some(expected));
    }

    #[rstest]
    fn enrichment_is_none_when_the_harness_did_not_provide_it() {
        let raw = serde_json::json!({
            "type": "message",
            "id": "m1",
            "message": {"role": "user", "content": "hi"},
        })
        .to_string();
        let m: PiMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), None);
        assert_eq!(m.usage(), None);
        assert_eq!(m.stop_reason(), None);
        assert_eq!(m.cwd(), None);
        assert_eq!(m.git_branch(), None);
    }

    #[rstest]
    #[case(0, false)]
    #[case(1, true)]
    fn bash_execution_is_a_user_command_with_its_output(#[case] exit: i64, #[case] error: bool) {
        let m: PiMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "message",
                "id": "m9",
                "message": {"role": "bashExecution", "command": "ls", "output": "a\nb", "exitCode": exit},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![
            Content::Text("!ls".into()),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("m9".to_owned()),
                output: serde_json::json!("a\nb"),
                error,
            }),
        ]);
    }

    #[rstest]
    fn exposes_parent_id_and_session_info_title() {
        let m: PiMessage = serde_json::from_str(
            &serde_json::json!({"type": "message", "id": "m2", "parentId": "m1",
                "message": {"role": "user", "content": "hi"}})
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.parent_id(), Some(MessageId::from("m1".to_owned())));
        assert_eq!(m.title(), None);

        let info: PiMessage = serde_json::from_str(
            &serde_json::json!({"type": "session_info", "id": "m3", "parentId": "m2", "name": "my session"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(info.title().as_deref(), Some("my session"));
    }

    #[rstest]
    fn assistant_turn_id_is_the_response_id() {
        let m: PiMessage = serde_json::from_str(
            &serde_json::json!({"type": "message", "id": "m2",
                "message": {"role": "assistant", "content": [], "responseId": "r1"}})
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.turn_id().as_deref(), Some("r1"));
    }

    #[rstest]
    #[tokio::test]
    async fn meta_reads_parent_session_from_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1700000000_s2.jsonl");
        std::fs::write(
            &path,
            serde_json::json!({"type": "session", "id": "s2", "cwd": "/w", "parentSession": "s1"})
                .to_string()
                + "\n",
        )
        .unwrap();

        let meta = PiSession::open(SessionId::from("s2".to_owned()), path).meta().await.unwrap();
        assert_eq!(meta.parent, Some(SessionId::from("s1".to_owned())));
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
        // Trailing newline required: messages() withholds an unterminated final line until a
        // later write completes it (a real session ends every record with a newline).
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = PiSession::open(SessionId::from("s1".to_owned()), path);
        let roles: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(roles.last(), Some(&Role::Assistant));
    }

    #[rstest]
    #[tokio::test]
    async fn meta_reads_cwd_from_session_header_and_latest_model_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1700000000_s1.jsonl");
        let body = [
            serde_json::json!({"type": "session", "id": "s1", "cwd": "/w"}).to_string(),
            serde_json::json!({
                "type": "model_change",
                "id": "c1",
                "provider": "anthropic",
                "modelId": "claude-sonnet-4",
            })
            .to_string(),
            serde_json::json!({
                "type": "model_change",
                "id": "c2",
                "provider": "anthropic",
                "modelId": "claude-opus-4-8",
            })
            .to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = PiSession::open(SessionId::from("s1".to_owned()), path);
        let meta = session.meta().await.unwrap();
        assert_eq!(meta.cwd, Some(PathBuf::from("/w")));
        assert_eq!(meta.model, Some("claude-opus-4-8".to_owned()));
        assert_eq!(meta.git_branch, None);
    }

    #[rstest]
    #[tokio::test]
    async fn watch_emits_sessions_as_files_appear() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("--proj--");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("1700000000_s1.jsonl"),
            serde_json::json!({"type": "session", "id": "s1"}).to_string(),
        )
        .unwrap();

        let listener =
            PiSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        let seen: Vec<SessionId> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            listener.watch().take(1).map_ok(|s| s.id()).try_collect(),
        )
        .await
        .expect("watch() did not emit a session within 10s")
        .unwrap();
        assert_eq!(seen, vec![SessionId::from("s1".to_owned())]);
    }

    fn session_file(dir: &Path) -> PathBuf {
        let path = dir.join("1700000000_s1.jsonl");
        std::fs::write(
            &path,
            serde_json::json!({"type": "session", "id": "s1", "cwd": "/w"}).to_string() + "\n",
        )
        .unwrap();
        path
    }

    #[rstest]
    #[tokio::test]
    async fn messages_yields_lines_appended_after_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_file(dir.path());

        let listener =
            PiSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        // The watch stream owns the watcher: it must outlive the message stream.
        let mut sessions = std::pin::pin!(listener.watch());
        let session = timed_next(&mut sessions, 10).await.unwrap().unwrap();
        let mut messages = std::pin::pin!(session.messages());
        assert!(timed_next(&mut messages, 10).await.unwrap().is_ok());

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(
            &mut file,
            (serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": "hi"},
            })
            .to_string()
                + "\n")
                .as_bytes(),
        )
        .unwrap();
        drop(file);
        assert_eq!(timed_next(&mut messages, 10).await.unwrap().unwrap().role(), Role::Assistant);
    }

    #[rstest]
    #[tokio::test]
    async fn messages_ends_when_the_session_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_file(dir.path());

        let listener =
            PiSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        let mut sessions = std::pin::pin!(listener.watch());
        let session = timed_next(&mut sessions, 10).await.unwrap().unwrap();
        let mut messages = std::pin::pin!(session.messages());
        assert!(timed_next(&mut messages, 10).await.unwrap().is_ok());

        std::fs::remove_file(&path).unwrap();
        // A change signalled for the vanished path may surface as an I/O error first; the
        // stream must still end once the watcher drops the file's handler.
        loop {
            match timed_next(&mut messages, 10).await {
                None => break,
                Some(Err(_)) => {}
                Some(Ok(m)) => panic!("unexpected message after removal: {m:?}"),
            }
        }
    }

    #[rstest]
    #[case(include_str!("../../../tests/fixtures/pi/session1.jsonl"))]
    #[case(include_str!("../../../tests/fixtures/pi/session2.jsonl"))]
    fn normalizes_a_real_redacted_session(#[case] jsonl: &str) {
        let msgs: Vec<PiMessage> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<PiMessage>(l).expect("fixture record parses"))
            .collect();
        assert!(msgs.len() >= 10);

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
                        assert!(!u.name.is_empty(), "toolCall normalized to an empty name");
                        assert!(!u.id.to_string().is_empty(), "toolCall normalized to an empty id");
                        tool_uses += 1;
                    }
                    Content::ToolResult(r) => {
                        assert!(!r.call.to_string().is_empty(), "toolResult lost its call id");
                        tool_results += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(saw_user && saw_assistant, "expected both user and assistant turns");
        assert!(tool_uses >= 1, "expected at least one normalized toolCall");
        assert!(tool_results >= 1, "expected at least one normalized toolResult");
    }
}
