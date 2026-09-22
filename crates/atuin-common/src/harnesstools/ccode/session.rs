use std::path::{Path, PathBuf};

use futures::{Stream, TryStreamExt};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{NodeContext, TreeWatcher};
use crate::harnesstools::ccode::Ccode;
use crate::harnesstools::session::model::{
    Content, MessageId, Role, SessionMeta, StopReason, ToolCallId, ToolResult, ToolUse, Usage,
};
use crate::harnesstools::session::{
    Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId, Sessions,
    WatchError, scan_sessions,
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

    fn existing(
        &self,
    ) -> Result<impl Stream<Item = Result<CcodeSession, RuntimeError>> + Send + 'static, RuntimeError>
    {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(async_stream::stream! {
            let scan = tokio::task::spawn_blocking(move || {
                scan_sessions(root, CcodeListener::open_session)
            })
            .await;
            match scan {
                Ok(items) => {
                    for item in items {
                        yield item;
                    }
                }
                Err(join) => yield Err(RuntimeError::Io(std::io::Error::other(join))),
            }
        })
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
    /// Build a read-once session for an accepted `jsonl` file (no change signal), or `None`.
    fn open_session(path: &Path, is_file: bool) -> Option<CcodeSession> {
        if !is_file || path.extension().is_none_or(|ext| ext != "jsonl") {
            return None;
        }
        let id = path.file_stem()?.to_string_lossy().into_owned();
        Some(CcodeSession::open(SessionId::from(id), path.to_path_buf()))
    }

    /// The session for an accepted file, paired with the change signal the watcher keeps alive
    /// for as long as the file exists.
    fn accept(ctx: &NodeContext) -> Option<(CcodeSession, watch::Sender<()>)> {
        let mut session = Self::open_session(ctx.path(), ctx.is_file())?;
        let (signal, rx) = watch::channel(());
        session.changes = Some(rx);
        Some((session, signal))
    }
}

impl Listener for CcodeListener {
    type Session = CcodeSession;

    fn watch(self) -> impl Stream<Item = Result<CcodeSession, WatchError>> + Send + 'static {
        let root = self.root;
        async_stream::stream! {
            let (tx, rx) = flume::unbounded::<CcodeSession>();
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
pub struct CcodeSession {
    id: SessionId,
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the file; `None` reads it once.
    changes: Option<watch::Receiver<()>>,
}

impl CcodeSession {
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

impl Session for CcodeSession {
    type Message = CcodeMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages(self) -> impl Stream<Item = Result<CcodeMessage, MessageError>> + Send + 'static {
        jsonl::follow::<CcodeMessage>(self.path, self.changes).map_err(MessageError::from)
    }

    fn read(&self) -> impl Stream<Item = Result<CcodeMessage, MessageError>> + Send + 'static {
        jsonl::read_all::<CcodeMessage>(self.path.clone()).map_err(MessageError::from)
    }

    fn meta(&self) -> impl std::future::Future<Output = Result<SessionMeta, MessageError>> + Send {
        let path = self.path.clone();
        async move {
            let messages: Vec<CcodeMessage> =
                jsonl::read_all(path).map_err(MessageError::from).try_collect().await?;
            let title = messages.iter().rev().find_map(Message::title);
            let cwd = messages.iter().find_map(|m| m.cwd.clone());
            let git_branch = messages.iter().find_map(|m| m.git_branch.clone());
            Ok(SessionMeta {
                cwd,
                git_branch,
                title,
                ..SessionMeta::default()
            })
        }
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
    cwd: Option<PathBuf>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    #[serde(rename = "parentUuid")]
    parent_uuid: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "isCompactSummary")]
    is_compact_summary: Option<bool>,
}

fn ccode_stop_reason(raw: &str) -> StopReason {
    match raw {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::Refusal,
        other => StopReason::Other(other.to_owned()),
    }
}

impl CcodeMessage {
    fn block(value: &serde_json::Value) -> Content {
        match value["type"].as_str() {
            Some("text") => Content::Text(value["text"].as_str().unwrap_or_default().to_owned()),
            Some("thinking" | "redacted_thinking") => Content::ReasoningSummary { tokens: None },
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
        if self.is_compact_summary == Some(true) {
            return Role::System;
        }
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
        let mut content: Vec<_> = match raw {
            Some(serde_json::Value::String(text)) => vec![Content::Text(text.clone())],
            Some(serde_json::Value::Array(blocks)) => blocks.iter().map(Self::block).collect(),
            _ => Vec::new(),
        };
        // Usage may arrive on a later text/tool row, independently of the thinking block.
        // The capture engine deduplicates these model-call totals across split rows.
        let reported = self
            .message
            .as_ref()
            .and_then(|m| m["usage"]["output_tokens_details"]["thinking_tokens"].as_u64());
        if let Some(Content::ReasoningSummary { tokens }) =
            content.iter_mut().find(|block| matches!(block, Content::ReasoningSummary { .. }))
        {
            *tokens = reported;
        } else if reported.is_some_and(|n| n > 0) {
            content.push(Content::ReasoningSummary { tokens: reported });
        }
        content
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
            input: usage.get("input_tokens").and_then(serde_json::Value::as_u64),
            output: usage.get("output_tokens").and_then(serde_json::Value::as_u64),
            cache_read: usage.get("cache_read_input_tokens").and_then(serde_json::Value::as_u64),
            cache_write: usage
                .get("cache_creation_input_tokens")
                .and_then(serde_json::Value::as_u64),
        })
    }

    fn stop_reason(&self) -> Option<StopReason> {
        // An interrupt is recorded as a user line; it is the turn that it ends. Peeked from the
        // raw JSON rather than `content()`, which would clone every block to read one string.
        let message = self.message.as_ref()?;
        let first_text = match &message["content"] {
            serde_json::Value::String(text) => Some(text.as_str()),
            serde_json::Value::Array(blocks) => blocks.first().and_then(|b| b["text"].as_str()),
            _ => None,
        };
        if first_text.is_some_and(|t| t.trim_start().starts_with("[Request interrupted by user")) {
            return Some(StopReason::Aborted);
        }
        Some(ccode_stop_reason(message.get("stop_reason")?.as_str()?))
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    fn git_branch(&self) -> Option<String> {
        self.git_branch.clone()
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent_uuid.clone().map(MessageId::from)
    }

    fn parent_session(&self) -> Option<SessionId> {
        self.session_id.clone().map(SessionId::from)
    }

    fn turn_id(&self) -> Option<String> {
        self.message.as_ref()?.get("id")?.as_str().map(str::to_owned)
    }

    fn title(&self) -> Option<String> {
        self.custom_title.clone().or_else(|| self.ai_title.clone())
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
    use crate::harnesstools::session::{
        Message, Session, SessionEvent, SessionEventKind, Sessions,
    };

    #[allow(clippy::needless_pass_by_value)]
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
    fn normalizes_assistant_enrichment_fields() {
        let raw = serde_json::json!({
            "type": "assistant",
            "uuid": "aaaa",
            "cwd": "/work/atuin",
            "gitBranch": "main",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "model": "claude-opus-4-8",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 20,
                    "cache_read_input_tokens": 5,
                    "cache_creation_input_tokens": 2,
                },
            },
        })
        .to_string();
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), Some("claude-opus-4-8".to_owned()));
        assert_eq!(m.stop_reason(), Some(StopReason::EndTurn));
        assert_eq!(m.cwd(), Some(PathBuf::from("/work/atuin")));
        assert_eq!(m.git_branch(), Some("main".to_owned()));
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
    #[case("max_tokens", StopReason::MaxTokens)]
    #[case("tool_use", StopReason::ToolUse)]
    #[case("stop_sequence", StopReason::StopSequence)]
    #[case("refusal", StopReason::Refusal)]
    #[case("weird", StopReason::Other("weird".to_owned()))]
    fn maps_stop_reason_vocabulary(#[case] raw: &str, #[case] expected: StopReason) {
        let m: CcodeMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "assistant",
                "message": {"role": "assistant", "content": [], "stop_reason": raw},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.stop_reason(), Some(expected));
    }

    #[rstest]
    fn enrichment_is_none_when_the_harness_did_not_provide_it() {
        let raw = line("user", "user", serde_json::json!("hi there"));
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), None);
        assert_eq!(m.usage(), None);
        assert_eq!(m.stop_reason(), None);
        assert_eq!(m.cwd(), None);
        assert_eq!(m.git_branch(), None);
    }

    #[rstest]
    fn exposes_parent_line_parent_session_and_turn() {
        let m: CcodeMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "assistant",
                "uuid": "bbbb",
                "parentUuid": "aaaa",
                "sessionId": "p",
                "message": {"role": "assistant", "id": "msg_01", "content": []},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.parent_id(), Some(MessageId::from("aaaa".to_owned())));
        assert_eq!(m.parent_session(), Some(SessionId::from("p".to_owned())));
        assert_eq!(m.turn_id().as_deref(), Some("msg_01"));
    }

    #[rstest]
    #[case(serde_json::json!({"type": "ai-title", "aiTitle": "generated"}), "generated")]
    #[case(serde_json::json!({"type": "custom-title", "customTitle": "by hand"}), "by hand")]
    fn title_lines_expose_the_title(#[case] raw: serde_json::Value, #[case] expected: &str) {
        let m: CcodeMessage = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(m.title().as_deref(), Some(expected));
        assert!(m.content().is_empty());
    }

    #[rstest]
    #[case(
        serde_json::json!({"type": "user", "isCompactSummary": true,
            "message": {"role": "user", "content": "summary"}}),
    )]
    #[case(
        serde_json::json!({"type": "system", "subtype": "compact_boundary",
            "compactMetadata": {"trigger": "auto"}, "content": "boundary"}),
    )]
    fn compaction_lines_are_system(#[case] raw: serde_json::Value) {
        let m: CcodeMessage = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(m.role(), Role::System);
    }

    #[rstest]
    #[case("[Request interrupted by user]", Some(StopReason::Aborted))]
    #[case("[Request interrupted by user for tool use]", Some(StopReason::Aborted))]
    #[case("please continue", None)]
    fn interrupt_lines_end_the_turn(#[case] text: &str, #[case] expected: Option<StopReason>) {
        let raw = line("user", "user", serde_json::json!([{"type": "text", "text": text}]));
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.stop_reason(), expected);
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
        // Trailing newline required: messages() withholds an unterminated final line until a
        // later write completes it (a real session ends every record with a newline).
        let body = [
            line("user", "user", serde_json::json!("first")),
            line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "second"}])),
        ]
        .join("\n")
            + "\n";
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
    async fn meta_reads_the_title_and_first_cwd_and_branch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let body = [
            line("user", "user", serde_json::json!("first")),
            serde_json::json!({
                "type": "ai-title",
                "aiTitle": "Fix the flaky test",
                "sessionId": "11111111-1111-1111-1111-111111111111",
            })
            .to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = CcodeSession::open(
            SessionId::from("11111111-1111-1111-1111-111111111111".to_owned()),
            path,
        );
        let meta = session.meta().await.unwrap();
        assert_eq!(meta.title, Some("Fix the flaky test".to_owned()));
        assert_eq!(meta.cwd, None);
        assert_eq!(meta.git_branch, None);
    }

    #[rstest]
    #[tokio::test]
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
        let seen: Vec<SessionId> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            listener.watch().take(1).map_ok(|s| s.id()).try_collect(),
        )
        .await
        .expect("watch() did not emit a session within 10s")
        .unwrap();
        assert_eq!(seen.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn events_yields_started_then_messages() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("project-a");
        std::fs::create_dir_all(&sub).unwrap();
        // Trailing newline required: each session's messages() withholds an unterminated final
        // line until a later write completes it (a real session ends every record with a newline).
        std::fs::write(
            sub.join("33333333-3333-3333-3333-333333333333.jsonl"),
            [
                line("user", "user", serde_json::json!("hi")),
                line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "yo"}])),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();

        let listener =
            CcodeSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        let events: Vec<SessionEvent<CcodeMessage>> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            listener.events().take(3).try_collect(),
        )
        .await
        .expect("events() did not produce within 10s")
        .unwrap();

        assert!(matches!(events[0].kind, SessionEventKind::Started(_)));
        let sid = SessionId::from("33333333-3333-3333-3333-333333333333".to_owned());
        assert!(events.iter().all(|event| event.session == sid));
        let roles: Vec<Role> = events
            .iter()
            .filter_map(|event| match &event.kind {
                SessionEventKind::Message(message) => Some(message.role()),
                SessionEventKind::Started(_) => None,
            })
            .collect();
        assert_eq!(roles, vec![Role::User, Role::Assistant]);
    }

    #[rstest]
    #[tokio::test]
    async fn messages_yields_lines_appended_after_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("44444444-4444-4444-4444-444444444444.jsonl");
        std::fs::write(&path, line("user", "user", serde_json::json!("hi")) + "\n").unwrap();

        let listener =
            CcodeSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        // The watch stream owns the watcher: it must outlive the message stream.
        let mut sessions = std::pin::pin!(listener.watch());
        let session = timed_next(&mut sessions, 10).await.unwrap().unwrap();
        let mut messages = std::pin::pin!(session.messages());
        assert_eq!(timed_next(&mut messages, 10).await.unwrap().unwrap().role(), Role::User);

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(
            &mut file,
            (line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "yo"}]))
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
        let path = dir.path().join("55555555-5555-5555-5555-555555555555.jsonl");
        std::fs::write(&path, line("user", "user", serde_json::json!("hi")) + "\n").unwrap();

        let listener =
            CcodeSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
        let mut sessions = std::pin::pin!(listener.watch());
        let session = timed_next(&mut sessions, 10).await.unwrap().unwrap();
        let mut messages = std::pin::pin!(session.messages());
        assert!(timed_next(&mut messages, 10).await.unwrap().is_ok());

        std::fs::remove_file(&path).unwrap();
        // A change signalled for the vanished path may surface as an I/O error first; the
        // stream must still end once the watcher drops the file's handler. Removal is detected
        // by a filesystem event or, if that is missed, by the periodic full scan (the content
        // poll cannot see a vanished file), so the timeout must exceed the scan interval: a
        // missed event then falls back to the scan instead of flaking.
        loop {
            match timed_next(&mut messages, 45).await {
                None => break,
                Some(Err(_)) => {}
                Some(Ok(m)) => panic!("unexpected message after removal: {m:?}"),
            }
        }
    }

    #[rstest]
    #[case(include_str!("../../../tests/fixtures/ccode/session1.jsonl"))]
    #[case(include_str!("../../../tests/fixtures/ccode/session2.jsonl"))]
    #[case(include_str!("../../../tests/fixtures/ccode/session3.jsonl"))]
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
