use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    #[builder(default = Replay::All)]
    replay: Replay,
}

impl OpencodeSessions {
    fn data_dir() -> PathBuf {
        env_nonempty("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".local").join("share"))
            .join("opencode")
    }

    fn wal_recency(path: &Path) -> Option<SystemTime> {
        let mtime = |p: &Path| std::fs::metadata(p).ok()?.modified().ok();
        let mut newest = mtime(path);
        for suffix in ["-wal", "-shm"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            newest = newest.max(mtime(Path::new(&name)));
        }
        newest
    }

    fn discover(data: &Path) -> Option<PathBuf> {
        std::fs::read_dir(data)
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("opencode") && name.ends_with(".db")
            })
            .map(|entry| entry.path())
            .max_by_key(|path| Self::wal_recency(path))
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
        if env_nonempty("OPENCODE_DISABLE_CHANNEL_DB")
            .is_some_and(|v| matches!(v.to_str(), Some("1" | "true")))
        {
            return Some(data.join("opencode.db"));
        }
        Some(Self::discover(&data).unwrap_or_else(|| data.join("opencode.db")))
    }
}

impl Sessions for OpencodeSessions {
    type Listener = OpencodeListener;

    fn listener(&self) -> Result<OpencodeListener, RuntimeError> {
        match self.resolve_db() {
            Some(db) if db.is_file() => Ok(OpencodeListener {
                db,
                replay: self.replay,
            }),
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
    replay: Replay,
}

const DEMUX_CAP: usize = 64;

impl Listener for OpencodeListener {
    type Session = OpencodeSession;

    fn watch(self) -> impl Stream<Item = Result<OpencodeSession, WatchError>> + Send + 'static {
        let db = self.db;
        let replay = self.replay;
        async_stream::try_stream! {
            let observer = SqliteObserver::new(&db);
            let mut events =
                observer.append::<EventRow>(ObserveConfig::builder().replay(replay).build()).await?;
            let mut live: HashMap<String, flume::Sender<EventRow>> = HashMap::new();
            while let Some(next) = events.next().await {
                let Appended(row) = next?;
                let kind = EventRow::classify(&row.kind);
                if let Some(tx) = live.get(row.aggregate_id.as_str()) {
                    if kind.forwarded()
                        && let Err(flume::SendError(row)) = tx.send_async(row).await
                    {
                        live.remove(row.aggregate_id.as_str());
                    }
                } else {
                    let id = row.aggregate_id.clone();
                    let (tx, rx) = flume::bounded(DEMUX_CAP);
                    if kind.forwarded() {
                        let _ = tx.send(row);
                    }
                    live.insert(id.clone(), tx);
                    yield OpencodeSession { id: SessionId::from(id), events: rx };
                }
            }
        }
    }
}

#[derive(Debug)]
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
                let kind = EventRow::classify(&row.kind);
                if matches!(kind, EventKind::Ignored) {
                    continue;
                }
                let mut data: serde_json::Value = match serde_json::from_str(&row.data) {
                    Ok(data) => data,
                    Err(err) => {
                        yield Err(MessageError::from(err));
                        continue;
                    }
                };
                match kind {
                    EventKind::Role => {
                        let info = &data["info"];
                        if let Some(id) = info["id"].as_str() {
                            roles.insert(
                                id.to_owned(),
                                OpencodeMessage::role_of(info["role"].as_str().unwrap_or_default()),
                            );
                        }
                    }
                    EventKind::Part => {
                        let time = data["time"].as_i64();
                        let part = data["part"].take();
                        let role = part["messageID"]
                            .as_str()
                            .and_then(|id| roles.get(id).cloned())
                            .unwrap_or(Role::Assistant);
                        yield Ok(OpencodeMessage::part(role, part, time));
                    }
                    EventKind::Unmapped => {
                        let time = data["time"].as_i64();
                        yield Ok(OpencodeMessage::raw(data, time));
                    }
                    EventKind::Ignored => {}
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum EventKind {
    Role,
    Part,
    Unmapped,
    Ignored,
}

impl EventKind {
    const fn forwarded(self) -> bool {
        matches!(self, Self::Role | Self::Part | Self::Unmapped)
    }
}

#[derive(Clone, sqlx::FromRow)]
struct EventRow {
    rowid: i64,
    aggregate_id: String,
    #[sqlx(rename = "type")]
    kind: String,
    data: String,
}

impl EventRow {
    fn classify(kind: &str) -> EventKind {
        match kind {
            "message.updated.1" => EventKind::Role,
            "message.part.updated.1" => EventKind::Part,
            k if k.starts_with("session.next.") => EventKind::Unmapped,
            k if k.starts_with("message.updated.") => EventKind::Unmapped,
            k if k.starts_with("message.part.updated.") => EventKind::Unmapped,
            _ => EventKind::Ignored,
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
    fn part(role: Role, part: serde_json::Value, time: Option<i64>) -> Self {
        Self { role, part, time }
    }

    fn raw(data: serde_json::Value, time: Option<i64>) -> Self {
        Self {
            role: Role::Assistant,
            part: data,
            time,
        }
    }

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
                        output: state["output"].clone(),
                        error: false,
                    })),
                    Some("error") => content.push(Content::ToolResult(ToolResult {
                        call,
                        output: state["error"].clone(),
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
    use std::collections::HashSet;
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
    fn preserves_a_structured_tool_output() {
        let m = part_message(serde_json::json!({
            "type": "tool",
            "callID": "call_1",
            "tool": "bash",
            "state": {
                "status": "completed",
                "input": {},
                "output": {"stdout": "files", "exit": 0},
            },
        }));
        assert!(matches!(
            m.content().as_slice(),
            [Content::ToolUse(_), Content::ToolResult(r)]
                if r.output == serde_json::json!({"stdout": "files", "exit": 0})
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
    #[case("message.updated.1", EventKind::Role)]
    #[case("message.part.updated.1", EventKind::Part)]
    #[case("message.part.updated.2", EventKind::Unmapped)]
    #[case("session.next.tool.called.1", EventKind::Unmapped)]
    #[case("session.created.1", EventKind::Ignored)]
    #[case("message.part.delta.1", EventKind::Ignored)]
    fn classify_routes_events(#[case] kind: &str, #[case] expected: EventKind) {
        assert!(EventRow::classify(kind) == expected);
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
    fn discover_prefers_the_wal_aware_newest_db() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        for name in
            ["opencode.db", "opencode2.db", "opencode-old.db", "notopencode.db", "opencode.txt"]
        {
            std::fs::write(base.join(name), b"").unwrap();
        }
        let set_mtime = |name: &str, secs: u64| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(base.join(name))
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        set_mtime("opencode.db", 3000);
        set_mtime("opencode-old.db", 1000);
        set_mtime("opencode2.db", 2000);
        std::fs::write(base.join("opencode2.db-wal"), b"").unwrap();
        set_mtime("opencode2.db-wal", 9000);

        assert_eq!(OpencodeSessions::discover(base), Some(base.join("opencode2.db")));
    }

    async fn drive_demux() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;

        insert_event(
            &db,
            "e1",
            "ses_1",
            "message.updated.1",
            &serde_json::json!({"info": {"id": "msg_1", "role": "assistant"}}).to_string(),
        )
        .await;
        insert_event(
            &db,
            "e2",
            "ses_2",
            "message.updated.1",
            &serde_json::json!({"info": {"id": "msg_2", "role": "user"}}).to_string(),
        )
        .await;
        insert_event(
            &db,
            "e3",
            "ses_1",
            "message.part.updated.1",
            &serde_json::json!({
                "part": {"id": "prt_1", "messageID": "msg_1", "type": "text", "text": "hello"},
                "time": 1_700_000_000_000i64,
            })
            .to_string(),
        )
        .await;
        insert_event(
            &db,
            "e4",
            "ses_2",
            "message.part.updated.1",
            &serde_json::json!({
                "part": {"id": "prt_2", "messageID": "msg_2", "type": "text", "text": "world"},
                "time": 1_700_000_000_000i64,
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn events_demultiplex_sessions_multi_thread() {
        drive_demux().await;
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn events_demultiplex_sessions_current_thread() {
        tokio::time::timeout(Duration::from_secs(10), drive_demux())
            .await
            .expect("bounded demux deadlocked on a current-thread runtime");
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn bounded_demux_drains_past_capacity_on_one_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        insert_event(
            &db,
            "seed",
            "ses_1",
            "message.updated.1",
            &serde_json::json!({"info": {"id": "msg_1", "role": "assistant"}}).to_string(),
        )
        .await;
        let parts = DEMUX_CAP + 8;
        for i in 0..parts {
            insert_event(
                &db,
                &format!("p{i}"),
                "ses_1",
                "message.part.updated.1",
                &serde_json::json!({
                    "part": {"id": format!("prt_{i}"), "messageID": "msg_1", "type": "text", "text": i.to_string()},
                    "time": 1_700_000_000_000i64,
                })
                .to_string(),
            )
            .await;
        }

        let listener = OpencodeSessions::builder().db(path).build().listener().unwrap();
        let messages = tokio::time::timeout(
            Duration::from_secs(10),
            listener
                .events()
                .filter_map(|event| async move {
                    match event.unwrap().kind {
                        SessionEventKind::Message(message) => Some(message),
                        SessionEventKind::Started => None,
                    }
                })
                .take(parts)
                .collect::<Vec<_>>(),
        )
        .await
        .expect("bounded demux stalled at capacity on a current-thread runtime");

        assert_eq!(messages.len(), parts);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_from_now_skips_preexisting_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        insert_event(
            &db,
            "e1",
            "ses_pre",
            "message.updated.1",
            &serde_json::json!({"info": {"id": "m1", "role": "user"}}).to_string(),
        )
        .await;

        let listener = OpencodeSessions::builder()
            .db(path)
            .replay(Replay::FromNow)
            .build()
            .listener()
            .unwrap();
        let stream = listener.watch();
        futures::pin_mut!(stream);

        assert!(
            tokio::time::timeout(Duration::from_millis(500), stream.next()).await.is_err(),
            "FromNow replayed a pre-existing session"
        );

        insert_event(
            &db,
            "e2",
            "ses_new",
            "message.updated.1",
            &serde_json::json!({"info": {"id": "m2", "role": "user"}}).to_string(),
        )
        .await;

        let session = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("watch() did not emit within 10s")
            .expect("stream ended")
            .unwrap();
        assert_eq!(session.id(), SessionId::from("ses_new".to_owned()));
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_all_backfills_preexisting_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        insert_event(
            &db,
            "e1",
            "ses_pre",
            "message.updated.1",
            &serde_json::json!({"info": {"id": "m1", "role": "user"}}).to_string(),
        )
        .await;

        let listener = OpencodeSessions::builder().db(path).build().listener().unwrap();
        let stream = listener.watch();
        futures::pin_mut!(stream);
        let session = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("watch() did not emit within 10s")
            .expect("stream ended")
            .unwrap();
        assert_eq!(session.id(), SessionId::from("ses_pre".to_owned()));
    }

    #[rstest]
    #[tokio::test]
    async fn unmapped_event_surfaces_as_other() {
        let (tx, rx) = flume::unbounded();
        tx.send(EventRow {
            rowid: 1,
            aggregate_id: "ses_1".to_owned(),
            kind: "session.next.tool.called.1".to_owned(),
            data: serde_json::json!({"foo": "bar"}).to_string(),
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
            [Ok(m)] if matches!(m.content().as_slice(), [Content::Other(v)] if v["foo"] == "bar")
        ));
    }

    #[rstest]
    #[tokio::test]
    async fn ignored_event_is_never_parsed() {
        let (tx, rx) = flume::unbounded();
        tx.send(EventRow {
            rowid: 1,
            aggregate_id: "ses_1".to_owned(),
            kind: "session.created.1".to_owned(),
            data: "not json".to_owned(),
        })
        .unwrap();
        drop(tx);

        let session = OpencodeSession {
            id: SessionId::from("ses_1".to_owned()),
            events: rx,
        };
        let results: Vec<_> = session.messages().collect().await;
        assert!(results.is_empty());
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
            kind: "message.part.updated.1".to_owned(),
            data: "not json".to_owned(),
        })
        .unwrap();
        tx.send(EventRow {
            rowid: 2,
            aggregate_id: "ses_1".to_owned(),
            kind: "message.part.updated.1".to_owned(),
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

    async fn load_fixture(db: &Sqlite, jsonl: &str) {
        for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
            let row: serde_json::Value = serde_json::from_str(line).expect("fixture line parses");
            insert_event(
                db,
                row["id"].as_str().unwrap(),
                row["aggregate_id"].as_str().unwrap(),
                row["type"].as_str().unwrap(),
                &row["data"].to_string(),
            )
            .await;
        }
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reconstructs_two_interleaved_sessions_from_a_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        load_fixture(&db, include_str!("../../../tests/fixtures/opencode/session1.jsonl")).await;

        let listener = OpencodeSessions::builder().db(path).build().listener().unwrap();
        let events = tokio::time::timeout(
            Duration::from_secs(10),
            listener.events().take(10).collect::<Vec<_>>(),
        )
        .await
        .expect("events() did not emit 10 events within 10s");

        let mut started: HashSet<String> = HashSet::new();
        let mut by_session: HashMap<String, Vec<OpencodeMessage>> = HashMap::new();
        for event in events {
            let event = event.unwrap();
            let sid = event.session.to_string();
            match event.kind {
                SessionEventKind::Started => {
                    started.insert(sid);
                }
                SessionEventKind::Message(message) => {
                    by_session.entry(sid).or_default().push(message);
                }
            }
        }

        assert_eq!(started, HashSet::from(["ses_A".to_owned(), "ses_B".to_owned()]));

        let a = &by_session["ses_A"];
        assert!(a.iter().all(|m| m.timestamp().is_some()), "a part is missing its timestamp");

        let a1 = a
            .iter()
            .find(|m| m.id() == Some(MessageId::from("prtA1".to_owned())))
            .expect("prtA1 missing");
        assert_eq!(a1.role(), Role::User);
        assert_eq!(a1.content(), vec![Content::Text("hello from A".into())]);

        let a2_final = a
            .iter()
            .rfind(|m| m.id() == Some(MessageId::from("prtA2".to_owned())))
            .expect("prtA2 missing");
        assert_eq!(a2_final.role(), Role::Assistant);
        assert_eq!(a2_final.content(), vec![Content::Text("final answer A".into())]);

        let tool = a
            .iter()
            .find_map(|m| {
                m.content().into_iter().find_map(|c| match c {
                    Content::ToolResult(r) => Some(r),
                    _ => None,
                })
            })
            .expect("no completed tool result");
        assert_eq!(tool.output, serde_json::json!({"stdout": "listing", "exit": 0}));

        let b = &by_session["ses_B"];
        assert_eq!(b.len(), 2, "the ignored session.created.1 row leaked a message");
        let b1 = b
            .iter()
            .find(|m| m.id() == Some(MessageId::from("prtB1".to_owned())))
            .expect("prtB1 missing");
        assert_eq!(b1.role(), Role::User);
        assert_eq!(b1.content(), vec![Content::Text("hi from B".into())]);
        assert!(
            b.iter().any(|m| matches!(m.content().as_slice(), [Content::Other(v)] if !v.is_null())),
            "expected an Other message from the unmapped version"
        );
    }
}
