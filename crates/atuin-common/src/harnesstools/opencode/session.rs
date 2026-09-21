use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use futures::{Stream, StreamExt};
use time::OffsetDateTime;
use typed_builder::TypedBuilder;

use crate::db::sqlite::observe::{
    Appended, ObserveConfig, Replay, SqliteObserver, TableSchema, Tailable,
};
use crate::harnesstools::opencode::Opencode;
use crate::harnesstools::session::model::{
    Content, MessageId, Role, ToolCallId, ToolResult, ToolUse,
};
use crate::harnesstools::session::{
    Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId, Sessions,
    WatchError,
};
use crate::utils::{env_nonempty, home_dir};

#[derive(Debug, Clone, TypedBuilder)]
pub struct OpencodeSessions {
    #[builder(default, setter(strip_option, into))]
    db: Option<PathBuf>,
}

impl OpencodeSessions {
    fn data_dir() -> PathBuf {
        env_nonempty("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".local").join("share"))
            .join("opencode")
    }

    fn newest_db(data: &Path) -> Option<PathBuf> {
        std::fs::read_dir(data)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("opencode") && name.ends_with(".db")
            })
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .max_by_key(|(mtime, _)| *mtime)
            .map(|(_, path)| path)
    }

    fn resolve_db(&self) -> Option<PathBuf> {
        if let Some(db) = &self.db {
            return Some(db.clone());
        }
        let data = Self::data_dir();
        if let Some(env) = env_nonempty("OPENCODE_DB") {
            let path = PathBuf::from(env);
            if path.as_os_str() == OsStr::new(":memory:") {
                return None;
            }
            return Some(if path.is_absolute() {
                path
            } else {
                data.join(path)
            });
        }
        let default = data.join("opencode.db");
        if default.exists() {
            return Some(default);
        }
        Some(Self::newest_db(&data).unwrap_or(default))
    }
}

impl Sessions for OpencodeSessions {
    type Listener = OpencodeListener;

    fn listener(&self) -> Result<OpencodeListener, RuntimeError> {
        match self.resolve_db() {
            Some(db) if db.is_file() => Ok(OpencodeListener { db }),
            Some(db) => Err(RuntimeError::NotFound(db)),
            None => Err(RuntimeError::NotFound(Self::data_dir())),
        }
    }
}

impl Observable for Opencode {
    type Sessions = OpencodeSessions;

    fn sessions(&self) -> OpencodeSessions {
        OpencodeSessions::builder().build()
    }
}

#[derive(Debug, Clone)]
pub struct OpencodeListener {
    db: PathBuf,
}

impl Listener for OpencodeListener {
    type Session = OpencodeSession;

    fn watch(self) -> impl Stream<Item = Result<OpencodeSession, WatchError>> + Send + 'static {
        let db = self.db;
        async_stream::try_stream! {
            let observer = SqliteObserver::new(&db);
            let mut events =
                observer.append::<EventRow>(ObserveConfig::builder().replay(Replay::All).build()).await?;
            let mut senders: HashMap<SessionId, flume::Sender<EventRow>> = HashMap::new();
            while let Some(next) = events.next().await {
                let Appended(row) = next?;
                let id = SessionId::from(row.aggregate_id.clone());
                if let Some(tx) = senders.get(&id) {
                    if tx.send(row).is_err() {
                        senders.remove(&id);
                    }
                } else {
                    let (tx, rx) = flume::unbounded();
                    let _ = tx.send(row);
                    senders.insert(id.clone(), tx);
                    yield OpencodeSession { id, events: rx };
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpencodeSession {
    id: SessionId,
    events: flume::Receiver<EventRow>,
}

impl Session for OpencodeSession {
    type Message = OpencodeMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages(
        self,
    ) -> impl Stream<Item = Result<OpencodeMessage, MessageError>> + Send + 'static {
        let events = self.events;
        async_stream::stream! {
            let mut roles: HashMap<String, Role> = HashMap::new();
            while let Ok(row) = events.recv_async().await {
                let mut data: serde_json::Value = match serde_json::from_str(&row.data) {
                    Ok(data) => data,
                    Err(err) => {
                        yield Err(MessageError::from(err));
                        continue;
                    }
                };
                match EventRow::base_type(&row.kind) {
                    "message.updated" => {
                        let info = &data["info"];
                        if let Some(id) = info["id"].as_str() {
                            roles.insert(
                                id.to_owned(),
                                OpencodeMessage::role_of(info["role"].as_str().unwrap_or_default()),
                            );
                        }
                    }
                    "message.part.updated" => {
                        let part = data["part"].take();
                        let role = part["messageID"]
                            .as_str()
                            .and_then(|id| roles.get(id).cloned())
                            .unwrap_or(Role::Assistant);
                        let time = data["time"].as_i64();
                        yield Ok(OpencodeMessage { role, part, time });
                    }
                    _ => {}
                }
            }
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct EventRow {
    rowid: i64,
    aggregate_id: String,
    #[sqlx(rename = "type")]
    kind: String,
    data: String,
}

impl EventRow {
    fn base_type(kind: &str) -> &str {
        match kind.rsplit_once('.') {
            Some((head, tail)) if tail.parse::<i64>().is_ok() => head,
            _ => kind,
        }
    }
}

impl TableSchema for EventRow {
    const TABLE: &'static str = "event";
    const COLUMNS: &'static [&'static str] = &["rowid", "aggregate_id", "type", "data"];
}

impl Tailable for EventRow {
    type Cursor = i64;
    fn cursor(&self) -> i64 {
        self.rowid
    }
}

#[derive(Debug, Clone)]
pub struct OpencodeMessage {
    role: Role,
    part: serde_json::Value,
    time: Option<i64>,
}

impl OpencodeMessage {
    fn role_of(role: &str) -> Role {
        match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "tool" => Role::Tool,
            other => Role::Other(other.to_owned()),
        }
    }
}

impl Message for OpencodeMessage {
    fn id(&self) -> Option<MessageId> {
        self.part["id"].as_str().map(|id| MessageId::from(id.to_owned()))
    }

    fn role(&self) -> Role {
        self.role.clone()
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.time.and_then(|ms| {
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
        })
    }

    fn content(&self) -> Vec<Content> {
        let part = &self.part;
        match part["type"].as_str() {
            Some("text") => {
                vec![Content::Text(part["text"].as_str().unwrap_or_default().to_owned())]
            }
            Some("reasoning") => {
                vec![Content::Reasoning(part["text"].as_str().unwrap_or_default().to_owned())]
            }
            Some("tool") => {
                let call = ToolCallId::from(part["callID"].as_str().unwrap_or_default().to_owned());
                let state = &part["state"];
                let mut content = vec![Content::ToolUse(ToolUse {
                    id: call.clone(),
                    name: part["tool"].as_str().unwrap_or_default().to_owned(),
                    input: state["input"].clone(),
                })];
                match state["status"].as_str() {
                    Some("completed") => content.push(Content::ToolResult(ToolResult {
                        call,
                        output: serde_json::Value::String(
                            state["output"].as_str().unwrap_or_default().to_owned(),
                        ),
                        error: false,
                    })),
                    Some("error") => content.push(Content::ToolResult(ToolResult {
                        call,
                        output: serde_json::Value::String(
                            state["error"].as_str().unwrap_or_default().to_owned(),
                        ),
                        error: true,
                    })),
                    _ => {}
                }
                content
            }
            _ => vec![Content::Other(part.clone())],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use futures::StreamExt;
    use rstest::rstest;

    use super::*;
    use crate::db::query;
    use crate::db::sqlite::Sqlite;
    use crate::harnesstools::session::model::{Content, Role};
    use crate::harnesstools::session::{Message, SessionEventKind, Sessions};

    fn part_message(part: serde_json::Value) -> OpencodeMessage {
        OpencodeMessage {
            role: Role::Assistant,
            part,
            time: Some(1_700_000_000_000),
        }
    }

    #[rstest]
    fn normalizes_a_text_part() {
        let m = part_message(serde_json::json!({"id": "prt_1", "type": "text", "text": "hi"}));
        assert_eq!(m.role(), Role::Assistant);
        assert_eq!(m.content(), vec![Content::Text("hi".into())]);
        assert_eq!(m.id(), Some(MessageId::from("prt_1".to_owned())));
        assert_eq!(m.timestamp().unwrap().unix_timestamp(), 1_700_000_000);
    }

    #[rstest]
    fn normalizes_a_reasoning_part() {
        let m = part_message(serde_json::json!({"type": "reasoning", "text": "pondering"}));
        assert_eq!(m.content(), vec![Content::Reasoning("pondering".into())]);
    }

    #[rstest]
    fn normalizes_a_completed_tool_part() {
        let m = part_message(serde_json::json!({
            "type": "tool",
            "callID": "call_1",
            "tool": "bash",
            "state": {"status": "completed", "input": {"command": "ls"}, "output": "files"},
        }));
        let content = m.content();
        assert!(matches!(
            content.as_slice(),
            [Content::ToolUse(u), Content::ToolResult(r)]
                if u.name == "bash"
                    && u.id.as_ref() == "call_1"
                    && u.input == serde_json::json!({"command": "ls"})
                    && r.call.as_ref() == "call_1"
                    && !r.error
                    && r.output == serde_json::Value::String("files".into())
        ));
    }

    #[rstest]
    fn normalizes_an_errored_tool_part() {
        let m = part_message(serde_json::json!({
            "type": "tool",
            "callID": "call_1",
            "tool": "bash",
            "state": {"status": "error", "input": {}, "error": "boom"},
        }));
        assert!(matches!(
            m.content().as_slice(),
            [Content::ToolUse(_), Content::ToolResult(r)]
                if r.error && r.output == serde_json::Value::String("boom".into())
        ));
    }

    #[rstest]
    fn normalizes_a_running_tool_part() {
        let m = part_message(serde_json::json!({
            "type": "tool",
            "callID": "call_1",
            "tool": "bash",
            "state": {"status": "running", "input": {}},
        }));
        assert!(matches!(m.content().as_slice(), [Content::ToolUse(_)]));
    }

    #[rstest]
    #[case("message.part.updated.1", "message.part.updated")]
    #[case("message.part.updated", "message.part.updated")]
    #[case("session.updated", "session.updated")]
    fn strips_only_an_integer_version_suffix(#[case] kind: &str, #[case] expected: &str) {
        assert_eq!(EventRow::base_type(kind), expected);
    }

    async fn event_db(path: &Path) -> Sqlite {
        let sqlite = Sqlite::builder(path.as_os_str()).open().await.unwrap();
        query::<sqlx::Sqlite>(
            "CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT \
             NULL, type TEXT NOT NULL, data TEXT NOT NULL)",
        )
        .execute(&mut *sqlite.pool().acquire().await.unwrap())
        .await
        .unwrap();
        sqlite
    }

    async fn insert_event(sqlite: &Sqlite, id: &str, aggregate: &str, kind: &str, data: &str) {
        query::<sqlx::Sqlite>(
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?1, ?2, 0, ?3, ?4)",
        )
        .bind(id)
        .bind(aggregate)
        .bind(kind)
        .bind(data)
        .execute(&mut *sqlite.pool().acquire().await.unwrap())
        .await
        .unwrap();
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_db() {
        let sessions =
            OpencodeSessions::builder().db(PathBuf::from("/no/such/opencode.db")).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn events_demultiplex_sessions_and_normalize_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;

        insert_event(
            &db,
            "e1",
            "ses_1",
            "message.updated",
            &serde_json::json!({"info": {"id": "msg_1", "role": "assistant"}}).to_string(),
        )
        .await;
        insert_event(
            &db,
            "e2",
            "ses_1",
            "message.part.updated",
            &serde_json::json!({
                "part": {"id": "prt_1", "messageID": "msg_1", "type": "text", "text": "hello"},
                "time": 1_700_000_000_000i64,
            })
            .to_string(),
        )
        .await;
        insert_event(
            &db,
            "e3",
            "ses_2",
            "message.updated",
            &serde_json::json!({"info": {"id": "msg_2", "role": "user"}}).to_string(),
        )
        .await;
        insert_event(
            &db,
            "e4",
            "ses_2",
            "message.part.updated",
            &serde_json::json!({
                "part": {"id": "prt_2", "messageID": "msg_2", "type": "text", "text": "world"},
            })
            .to_string(),
        )
        .await;

        let listener = OpencodeSessions::builder().db(path).build().listener().unwrap();
        let events = tokio::time::timeout(
            Duration::from_secs(10),
            listener.events().take(4).collect::<Vec<_>>(),
        )
        .await
        .expect("events() did not emit within 10s");

        let mut messages: HashMap<String, (Role, Vec<Content>)> = HashMap::new();
        for event in events {
            let event = event.unwrap();
            if let SessionEventKind::Message(message) = event.kind {
                messages.insert(event.session.to_string(), (message.role(), message.content()));
            }
        }

        assert_eq!(
            messages.get("ses_1"),
            Some(&(Role::Assistant, vec![Content::Text("hello".into())]))
        );
        assert_eq!(messages.get("ses_2"), Some(&(Role::User, vec![Content::Text("world".into())])));
    }

    #[rstest]
    #[case("user", Role::User)]
    #[case("assistant", Role::Assistant)]
    #[case("system", Role::System)]
    #[case("tool", Role::Tool)]
    #[case("architect", Role::Other("architect".to_owned()))]
    fn maps_opencode_roles(#[case] raw: &str, #[case] expected: Role) {
        assert_eq!(OpencodeMessage::role_of(raw), expected);
    }

    #[rstest]
    fn falls_back_to_other_for_an_unknown_part_type() {
        let raw = serde_json::json!({"id": "prt_1", "type": "step-start", "step": 1});
        assert_eq!(part_message(raw.clone()).content(), vec![Content::Other(raw)]);
    }

    #[rstest]
    fn has_no_timestamp_without_a_time() {
        let m = OpencodeMessage {
            role: Role::Assistant,
            part: serde_json::json!({"type": "text", "text": "hi"}),
            time: None,
        };
        assert_eq!(m.timestamp(), None);
    }

    #[rstest]
    #[tokio::test]
    async fn surfaces_a_json_error_then_keeps_streaming() {
        let (tx, rx) = flume::unbounded();
        tx.send(EventRow {
            rowid: 1,
            aggregate_id: "ses_1".to_owned(),
            kind: "message.part.updated".to_owned(),
            data: "not json".to_owned(),
        })
        .unwrap();
        tx.send(EventRow {
            rowid: 2,
            aggregate_id: "ses_1".to_owned(),
            kind: "message.part.updated".to_owned(),
            data: serde_json::json!({
                "part": {"id": "prt_1", "messageID": "msg_1", "type": "text", "text": "ok"},
            })
            .to_string(),
        })
        .unwrap();
        drop(tx);

        let session = OpencodeSession {
            id: SessionId::from("ses_1".to_owned()),
            events: rx,
        };
        let results: Vec<_> = session.messages().collect().await;
        assert!(matches!(
            results.as_slice(),
            [Err(MessageError::Json(_)), Ok(m)] if m.content() == vec![Content::Text("ok".into())]
        ));
    }
}
