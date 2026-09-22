use std::path::{Path, PathBuf};

use futures::{Stream, TryStreamExt};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{NodeContext, TreeWatcher};
use crate::harnesstools::codex::Codex;
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
    /// The session for an accepted file, paired with the change signal the watcher keeps alive
    /// for as long as the file exists.
    fn accept(ctx: &NodeContext) -> Option<(CodexSession, watch::Sender<()>)> {
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
        let (signal, rx) = watch::channel(());
        let session = CodexSession {
            id: SessionId::from(id),
            path: path.to_path_buf(),
            changes: Some(rx),
        };
        Some((session, signal))
    }
}

impl Listener for CodexListener {
    type Session = CodexSession;

    fn watch(self) -> impl Stream<Item = Result<CodexSession, WatchError>> + Send + 'static {
        let root = self.root;
        async_stream::stream! {
            let (tx, rx) = flume::unbounded::<CodexSession>();
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
pub struct CodexSession {
    id: SessionId,
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the file; `None` reads it once.
    changes: Option<watch::Receiver<()>>,
}

impl CodexSession {
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

impl Session for CodexSession {
    type Message = CodexMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages(self) -> impl Stream<Item = Result<CodexMessage, MessageError>> + Send + 'static {
        jsonl::follow::<CodexMessage>(self.path, self.changes).map_err(MessageError::from)
    }

    fn meta(&self) -> impl std::future::Future<Output = Result<SessionMeta, MessageError>> + Send {
        let path = self.path.clone();
        async move {
            let messages: Vec<CodexMessage> =
                jsonl::read_all(path).map_err(MessageError::from).try_collect().await?;
            let cwd = messages.iter().find_map(Message::cwd);
            let model = messages.iter().find_map(Message::model);
            Ok(SessionMeta {
                cwd,
                model,
                ..SessionMeta::default()
            })
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodexMessage {
    #[serde(rename = "type")]
    kind: String,
    timestamp: Option<String>,
    payload: Option<serde_json::Value>,
}

/// Whether a command output reports a non-zero exit, in either shape Codex has used: the
/// `Process exited with code N` header, or an `"exit_code":N` field in a JSON envelope.
fn codex_output_failed(output: &serde_json::Value) -> bool {
    let texts: Vec<&str> = match output {
        serde_json::Value::String(s) => vec![s],
        serde_json::Value::Array(blocks) => {
            blocks.iter().filter_map(|b| b["text"].as_str()).collect()
        }
        _ => Vec::new(),
    };
    // The last occurrence: the header follows the output, which may quote an earlier one.
    texts.iter().any(|text| {
        ["Process exited with code ", "\"exit_code\":"].iter().any(|marker| {
            text.rfind(marker).is_some_and(|at| {
                let code: String = text[at + marker.len()..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-')
                    .collect();
                code.parse::<i64>().is_ok_and(|c| c != 0)
            })
        })
    })
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
        // Prefer the per-record `id` (ctc_/ctco_/msg_...) over `call_id`: a tool call and its
        // output share one `call_id`, so keying identity on it would collide the two records and
        // the dedup gate would drop the output. `call_id` linkage lives in the content, not here.
        self.payload
            .as_ref()
            .and_then(|p| p["id"].as_str().or_else(|| p["call_id"].as_str()))
            .map(|s| MessageId::from(s.to_owned()))
    }

    fn role(&self) -> Role {
        let payload = self.payload.as_ref();
        match payload.and_then(|p| p["type"].as_str()) {
            Some("function_call" | "custom_tool_call") => Role::Assistant,
            Some("function_call_output" | "custom_tool_call_output") => Role::Tool,
            Some("reasoning") => Role::Assistant,
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
                    error: codex_output_failed(&payload["output"]),
                })]
            }
            _ => match &payload["content"] {
                serde_json::Value::Array(blocks) => blocks.iter().map(Self::block).collect(),
                serde_json::Value::String(text) => vec![Content::Text(text.clone())],
                _ => Vec::new(),
            },
        }
    }

    fn model(&self) -> Option<String> {
        self.payload.as_ref()?.get("model")?.as_str().map(str::to_owned)
    }

    fn usage(&self) -> Option<Usage> {
        let usage = self.payload.as_ref()?.get("usage")?;
        if usage.is_null() {
            return None;
        }
        Some(Usage {
            input: usage.get("input_tokens").and_then(serde_json::Value::as_u64),
            output: usage.get("output_tokens").and_then(serde_json::Value::as_u64),
            cache_read: usage.get("cached_input_tokens").and_then(serde_json::Value::as_u64),
            cache_write: usage.get("cache_write_input_tokens").and_then(serde_json::Value::as_u64),
        })
    }

    fn stop_reason(&self) -> Option<StopReason> {
        (self.kind == "event_msg" && self.payload.as_ref()?["type"] == "turn_aborted")
            .then_some(StopReason::Aborted)
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.payload.as_ref()?.get("cwd")?.as_str().map(PathBuf::from)
    }

    /// Codex names the model call only on the accounting line it writes after each response;
    /// items carry no response id, and `turn_id` there is the whole agent turn, so `None`.
    fn turn_id(&self) -> Option<String> {
        if self.kind != "token_usage_record" {
            return None;
        }
        self.payload.as_ref()?.get("response_id")?.as_str().map(str::to_owned)
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
    fn turn_context_exposes_model_and_cwd() {
        let raw = serde_json::json!({
            "type": "turn_context",
            "payload": {"model": "gpt-5.6-terra", "effort": "medium", "cwd": "/work/atuin"},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), Some("gpt-5.6-terra".to_owned()));
        assert_eq!(m.cwd(), Some(PathBuf::from("/work/atuin")));
    }

    #[rstest]
    #[case(serde_json::json!({"type": "token_usage_record", "payload": {"turn_id": "t1", "response_id": "r1"}}), Some("r1"))]
    #[case(serde_json::json!({"type": "turn_context", "payload": {"turn_id": "t1"}}), None)]
    #[case(serde_json::json!({"type": "event_msg", "payload": {"type": "task_started", "turn_id": "t1"}}), None)]
    #[case(serde_json::json!({"type": "response_item", "payload": {"type": "message", "id": "m1"}}), None)]
    fn turn_id_is_the_response_id_on_accounting_lines(
        #[case] raw: serde_json::Value,
        #[case] expected: Option<&str>,
    ) {
        let m: CodexMessage = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(m.turn_id().as_deref(), expected);
    }

    #[rstest]
    fn token_usage_record_exposes_usage() {
        let raw = serde_json::json!({
            "type": "token_usage_record",
            "payload": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "cached_input_tokens": 20,
                    "cache_write_input_tokens": 0,
                },
            },
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            m.usage(),
            Some(Usage {
                input: Some(100),
                output: Some(50),
                cache_read: Some(20),
                cache_write: Some(0)
            })
        );
    }

    #[rstest]
    fn enrichment_is_none_when_the_harness_did_not_provide_it() {
        let raw = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hi"}]},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), None);
        assert_eq!(m.usage(), None);
        assert_eq!(m.stop_reason(), None);
        assert_eq!(m.git_branch(), None);
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
    fn tool_call_and_output_have_distinct_ids_despite_shared_call_id() {
        let call = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_x", "name": "sh", "input": "ls"},
        })
        .to_string();
        let output = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call_output", "id": "ctco_1", "call_id": "call_x", "output": "files"},
        })
        .to_string();
        let call: CodexMessage = serde_json::from_str(&call).unwrap();
        let output: CodexMessage = serde_json::from_str(&output).unwrap();
        assert_ne!(call.id(), output.id(), "call and its output must not share a source id");
        assert_eq!(call.id(), Some(MessageId::from("ctc_1".to_owned())));
        assert_eq!(output.id(), Some(MessageId::from("ctco_1".to_owned())));
    }

    #[rstest]
    #[case(serde_json::json!("ok\nProcess exited with code 0"), false)]
    #[case(serde_json::json!("boom\nProcess exited with code 2"), true)]
    #[case(serde_json::json!([{"type": "output_text", "text": "{\"output\":\"x\",\"exit_code\":0}"}]), false)]
    #[case(serde_json::json!([{"type": "output_text", "text": "{\"output\":\"x\",\"exit_code\":1}"}]), true)]
    #[case(serde_json::json!("plain text"), false)]
    #[case(serde_json::json!("killed\nProcess exited with code -9"), true)]
    #[case(serde_json::json!("log: Process exited with code 1\nProcess exited with code 0"), false)]
    #[case(serde_json::json!([{"type": "output_text", "text": "{\"output\": \"x\", \"exit_code\": 3}"}]), true)]
    fn tool_output_error_is_derived_from_exit_code(
        #[case] output: serde_json::Value,
        #[case] error: bool,
    ) {
        let m: CodexMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "response_item",
                "payload": {"type": "function_call_output", "call_id": "c1", "output": output},
            })
            .to_string(),
        )
        .unwrap();
        assert!(matches!(m.content().as_slice(), [Content::ToolResult(r)] if r.error == error));
    }

    #[rstest]
    #[case("turn_aborted", Some(StopReason::Aborted))]
    #[case("token_count", None)]
    fn turn_aborted_events_end_the_turn(#[case] kind: &str, #[case] expected: Option<StopReason>) {
        let m: CodexMessage = serde_json::from_str(
            &serde_json::json!({"type": "event_msg", "payload": {"type": kind}}).to_string(),
        )
        .unwrap();
        assert_eq!(m.stop_reason(), expected);
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
        // Trailing newline required: messages() withholds an unterminated final line until a
        // later write completes it (a real session ends every record with a newline).
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = CodexSession::open(SessionId::from("th1".to_owned()), path);
        let roles: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(roles.last(), Some(&Role::User));
    }

    #[rstest]
    #[tokio::test]
    async fn meta_reads_cwd_from_session_meta_and_model_from_turn_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-2026-09-18-th1.jsonl");
        let body = [
            serde_json::json!({
                "type": "session_meta",
                "payload": {"id": "th1", "cwd": "/work/atuin"},
            })
            .to_string(),
            serde_json::json!({
                "type": "turn_context",
                "payload": {"model": "gpt-5.6-terra", "cwd": "/work/atuin"},
            })
            .to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = CodexSession::open(SessionId::from("th1".to_owned()), path);
        let meta = session.meta().await.unwrap();
        assert_eq!(meta.cwd, Some(PathBuf::from("/work/atuin")));
        assert_eq!(meta.model, Some("gpt-5.6-terra".to_owned()));
        assert_eq!(meta.git_branch, None);
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

    fn rollout(dir: &Path) -> PathBuf {
        let path =
            dir.join("rollout-2026-09-19T00-00-00-0a1b2c3d-4e5f-6789-abcd-ef0123456789.jsonl");
        std::fs::write(
            &path,
            serde_json::json!({"type": "session_meta", "payload": {"id": "th1"}}).to_string()
                + "\n",
        )
        .unwrap();
        path
    }

    #[rstest]
    #[tokio::test]
    async fn messages_yields_lines_appended_after_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = rollout(dir.path());

        let listener =
            CodexSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
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
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            })
            .to_string()
                + "\n")
                .as_bytes(),
        )
        .unwrap();
        drop(file);
        assert_eq!(timed_next(&mut messages, 10).await.unwrap().unwrap().role(), Role::User);
    }

    #[rstest]
    #[tokio::test]
    async fn messages_ends_when_the_session_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = rollout(dir.path());

        let listener =
            CodexSessions::builder().root(dir.path().to_path_buf()).build().listener().unwrap();
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
