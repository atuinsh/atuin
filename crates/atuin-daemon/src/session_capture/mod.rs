mod engine;
mod import;
mod message_enricher;

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{
    AiSessionDatabase, AiSessionStore, Appended, DbError, HarnessKind, HarnessSession, Message,
    PushError, Session, SessionMatch,
};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::encryption::paseto_v4::Key;
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::sync::BlockingPool;
use atuin_domain::record::HostId;
use engine::SessionCaptureEngine;
use futures::{Stream, StreamExt};
pub use import::ImportProgress;
use import::SessionImporter;
use tokio::sync::{Mutex, broadcast};
use tokio_stream::wrappers::BroadcastStream;

const NOP_STORE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub enum SessionTailEvent {
    SessionStarted(Session),
    SessionUpdated(Session),
    Message(Message),
}

#[derive(Debug, thiserror::Error)]
pub enum AppendError {
    #[error(transparent)]
    Sidecar(#[from] DbError),
    #[error(transparent)]
    Push(#[from] PushError),
}

pub(crate) struct Sink {
    records: AiSessionStore,
    sidecar: AiSessionDatabase,
    tail: broadcast::Sender<SessionTailEvent>,
    // Serialize count lookup + persistence across live capture and import. At most one record
    // can be waiting for projection; repair it before admitting another capture.
    pending_projection: Mutex<Option<Message>>,
}

impl Sink {
    pub(crate) fn new(records: AiSessionStore, sidecar: AiSessionDatabase) -> Self {
        let (tail, _) = broadcast::channel(128);
        Self {
            records,
            sidecar,
            tail,
            pending_projection: Mutex::new(None),
        }
    }

    pub(crate) fn subscribe(&self) -> BroadcastStream<SessionTailEvent> {
        BroadcastStream::new(self.tail.subscribe())
    }

    pub(crate) async fn append(&self, mut msg: Message) -> Result<Appended, AppendError> {
        // Apply capture policy before either persistence path or the live tail. Keep structural
        // rows (even with no content) so parent links and usage accounting remain intact.
        sanitize_message(&mut msg);
        let mut pending = self.pending_projection.lock().await;
        if let Some(previous) = pending.as_ref() {
            self.project_and_broadcast(previous).await?;
            *pending = None;
        }
        // Dedup gate: if this logical message is already projected it is already in the record
        // store too, so there is nothing to do. Stable source ids (see MessageEnricher::source_id)
        // make this reliable across re-captures and keep the record store free of duplicates.
        if self.sidecar.contains_message(&msg.session, &msg.source_id).await? {
            return Ok(Appended::Duplicate);
        }

        // Only persisted counts suppress later rows. This works for interleaved calls,
        // replays and restarts, without keeping every observed call ID in memory.
        let has_tokens = msg
            .content
            .iter()
            .any(|block| matches!(block, Content::ReasoningSummary { tokens: Some(_) }));
        let mut counted = if has_tokens && let Some(turn) = &msg.turn_id {
            self.sidecar.has_reasoning_tokens(&msg.session, turn).await?
        } else {
            false
        };
        for block in &mut msg.content {
            if let Content::ReasoningSummary {
                tokens: Some(_), ..
            } = block
            {
                if counted {
                    *block = Content::ReasoningSummary { tokens: None };
                }
                counted = true;
            }
        }

        // Write the record store first: it is the synced source of truth and the sidecar is a
        // pure projection of it. If the sidecar write fails afterwards a later rebuild repairs it;
        // the reverse ordering could strand a message in the sidecar only -- lost on rebuild and
        // never synced.
        self.records.push(&msg).await?;

        *pending = Some(msg.clone());
        let appended = self.project_and_broadcast(&msg).await?;
        *pending = None;
        drop(pending);
        Ok(appended)
    }

    async fn project_and_broadcast(&self, msg: &Message) -> Result<Appended, AppendError> {
        let started = self.sidecar.get_session(&msg.session).await?.is_none();
        let appended = self.sidecar.append(msg).await?;
        if self.tail.receiver_count() > 0 {
            if let Some(session) = self.sidecar.get_session(&msg.session).await? {
                let event = if started {
                    SessionTailEvent::SessionStarted(session)
                } else {
                    SessionTailEvent::SessionUpdated(session)
                };
                let _ = self.tail.send(event);
            }
            let _ = self.tail.send(SessionTailEvent::Message(msg.clone()));
        }

        Ok(appended)
    }
}

/// Retain conversation text and payload-free tool breadcrumbs, never execution payloads.
/// Null payloads preserve the existing wire format without storing arguments or results.
/// This only affects new captures; existing synced records are not rewritten.
fn sanitize_message(msg: &mut Message) {
    let conversation = matches!(msg.role, Role::User | Role::Assistant);
    msg.content.retain_mut(|block| match block {
        Content::Text(text) if conversation => {
            *text = atuin_common::secrets::redact(text).into_owned();
            true
        }
        Content::ToolUse(tool) => {
            tool.input = serde_json::Value::Null;
            true
        }
        Content::ToolResult(result) => {
            result.output = serde_json::Value::Null;
            true
        }
        Content::Reasoning(_) => {
            *block = Content::ReasoningSummary { tokens: None };
            true
        }
        Content::ReasoningSummary { .. } => true,
        Content::Text(_) | Content::Other(_) => false,
    });
    if let Some(title) = &mut msg.session_title {
        *title = atuin_common::secrets::redact(title).into_owned();
    }
}

pub struct AiHarnessSessionCapture {
    sink: Arc<Sink>,
    /// Runs the harness session file reads of capture and import.
    pool: BlockingPool,
    persistent: bool,
    _engine: SessionCaptureEngine,
}

impl AiHarnessSessionCapture {
    #[must_use]
    /// `recovered` must only be true after the record store has successfully rebuilt the sidecar.
    /// Failed recovery leaves existing sessions readable, but disables capture and import until
    /// restart so missing projections cannot cause duplicate records or reasoning counts.
    pub fn open(
        records: AiSessionStore,
        sidecar: AiSessionDatabase,
        capture: bool,
        recovered: bool,
        pool: BlockingPool,
    ) -> Self {
        let sink = Arc::new(Sink::new(records, sidecar));
        // Capture is opt-in. When disabled we still open the sidecar and serve existing sessions,
        // but never spawn the listeners that copy new transcripts into the synced record store.
        let engine = if capture && recovered {
            SessionCaptureEngine::spawn(&sink, &pool)
        } else {
            SessionCaptureEngine::nop()
        };
        Self {
            sink,
            pool,
            persistent: recovered,
            _engine: engine,
        }
    }

    pub async fn nop() -> Self {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT)
            .await
            .expect("in-memory sqlite store must open for the nop ai-session capture facade");
        let records = AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::in_memory().await.expect(
            "in-memory ai-session database must open for the nop ai-session capture facade",
        );

        Self {
            sink: Arc::new(Sink::new(records, sidecar)),
            // Never runs anything: without a persistent store there is no capture or import.
            pool: BlockingPool::new(NonZeroUsize::MIN),
            persistent: false,
            _engine: SessionCaptureEngine::nop(),
        }
    }

    /// Whether the persistent session store is ready for capture and import. `false` means
    /// opening or recovering the store failed; any available projected sessions remain readable.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.persistent
    }

    pub fn import(
        &self,
        harness: Option<HarnessKind>,
    ) -> impl Stream<Item = ImportProgress> + Send + 'static {
        if self.persistent {
            SessionImporter::new(self.sink.clone(), self.pool.clone()).run(harness).right_stream()
        } else {
            futures::stream::once(async {
                ImportProgress::Finished {
                    sessions: 0,
                    imported: 0,
                    skipped: 0,
                    failed: 0,
                }
            })
            .left_stream()
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> BroadcastStream<SessionTailEvent> {
        self.sink.subscribe()
    }

    pub async fn list_sessions(
        &self,
        harness: Option<HarnessKind>,
    ) -> Result<Vec<Session>, DbError> {
        self.sink.sidecar.list_sessions(harness).await
    }

    pub async fn get_session(&self, session: &HarnessSession) -> Result<Option<Session>, DbError> {
        self.sink.sidecar.get_session(session).await
    }

    pub fn messages(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<Message, DbError>> + Send + 'static {
        self.sink.sidecar.messages(session)
    }

    pub fn transcript(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<String, DbError>> + Send + 'static {
        self.sink.sidecar.transcript(session)
    }

    pub fn search(
        &self,
        query: &str,
        harness: Option<HarnessKind>,
        limit: u32,
    ) -> impl Stream<Item = Result<SessionMatch, DbError>> + Send + 'static {
        self.sink.sidecar.search(query, harness, limit)
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::{ToolCallId, ToolResult, ToolUse, Usage};
    use atuin_domain::record::{RecordId, RecordTag};
    use futures::StreamExt;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;

    async fn mem_store() -> AiSessionStore {
        let store = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build()
    }

    fn sample_handle() -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("native-session".to_owned()),
        }
    }

    fn sample_message() -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(sample_handle())
            .source_id(SourceId::from("source-id".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(vec![Content::Text("hello".to_owned())])
            .build()
    }

    #[rstest]
    #[tokio::test]
    async fn append_emits_started_then_message_to_subscriber() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let mut sub = sink.subscribe();

        sink.append(sample_message()).await.unwrap();

        assert!(matches!(sub.next().await.unwrap().unwrap(), SessionTailEvent::SessionStarted(_)));
        assert!(matches!(sub.next().await.unwrap().unwrap(), SessionTailEvent::Message(_)));
    }

    #[rstest]
    #[case(Role::User)]
    #[case(Role::Assistant)]
    #[tokio::test]
    async fn capture_sanitizes_records_sidecar_and_tail(#[case] role: Role) {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let mut sub = sink.subscribe();
        let mut msg = sample_message();
        msg.role = role;
        msg.parent_source_id = Some("parent".to_owned().into());
        msg.turn_id = Some("turn".to_owned());
        msg.usage = Some(Usage {
            input: Some(42),
            output: Some(0),
            cache_read: Some(0),
            cache_write: Some(0),
        });
        msg.session_title = Some("AWS_SECRET_ACCESS_KEY=TITLESECRET".to_owned());
        msg.content = vec![
            Content::Text("AWS_SECRET_ACCESS_KEY=TEXTSECRET".to_owned()),
            Content::ToolUse(ToolUse {
                id: ToolCallId::from("call".to_owned()),
                name: "Bash".to_owned(),
                input: serde_json::json!({"command": "PRIVATE_INPUT"}),
            }),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("call".to_owned()),
                output: serde_json::json!({"text": "PRIVATE_OUTPUT"}),
                error: true,
            }),
            Content::Reasoning("PRIVATE_REASONING".to_owned()),
            Content::Other(serde_json::json!({"attachment": "PRIVATE_ATTACHMENT"})),
        ];
        sink.append(msg.clone()).await.unwrap();
        sanitize_message(&mut msg);
        assert_eq!(msg.content.len(), 4);
        assert_eq!(msg.content[3], Content::ReasoningSummary { tokens: None });
        assert_eq!(msg.content[0], Content::Text("AWS_SECRET_ACCESS_KEY=****".to_owned()));
        assert!(matches!(&msg.content[1], Content::ToolUse(t)
            if t.name == "Bash" && t.id.as_ref() == "call" && t.input.is_null()));
        assert!(matches!(&msg.content[2], Content::ToolResult(t)
            if t.call.as_ref() == "call" && t.error && t.output.is_null()));
        assert_eq!(msg.session_title.as_deref(), Some("AWS_SECRET_ACCESS_KEY=****"));

        let event = sub.next().await.unwrap().unwrap();
        assert!(matches!(event, SessionTailEvent::SessionStarted(_)));
        let SessionTailEvent::Message(tail) = sub.next().await.unwrap().unwrap() else {
            panic!("expected message");
        };
        assert_eq!(tail, msg);
        assert_eq!(
            sink.sidecar.get_session(&msg.session).await.unwrap().unwrap().title,
            msg.session_title,
        );
        // The projection keeps titles on sessions rather than individual messages.
        let title = msg.session_title.take();
        let mut messages = Box::pin(sink.sidecar.messages(&msg.session));
        assert_eq!(messages.next().await.unwrap().unwrap(), msg);

        // Rebuilding from encrypted records must not restore discarded payloads.
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        sink.records.build(&rebuilt).await.unwrap();
        assert_eq!(rebuilt.get_session(&msg.session).await.unwrap().unwrap().title, title);
        let mut messages = Box::pin(rebuilt.messages(&msg.session));
        assert_eq!(messages.next().await.unwrap().unwrap(), msg);
        for query in [
            "PRIVATE_INPUT",
            "PRIVATE_OUTPUT",
            "PRIVATE_REASONING",
            "PRIVATE_ATTACHMENT",
            "TEXTSECRET",
        ] {
            let mut matches = Box::pin(sink.sidecar.search(query, None, 10));
            assert!(matches.next().await.is_none(), "sensitive content indexed: {query}");
        }
    }

    #[rstest]
    #[case(None, "Reasoned")]
    #[case(Some(185), "Reasoning · 185 tokens")]
    #[case(Some(0), "Reasoning · 0 tokens")]
    #[tokio::test]
    async fn reasoning_metadata_survives_storage_and_rendering(
        #[case] tokens: Option<u64>,
        #[case] label: &str,
    ) {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let mut msg = sample_message();
        msg.role = Role::Assistant;
        msg.content = vec![Content::ReasoningSummary { tokens }];
        sink.append(msg.clone()).await.unwrap();
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        sink.records.build(&rebuilt).await.unwrap();
        let mut messages = Box::pin(rebuilt.messages(&msg.session));
        let stored = messages.next().await.unwrap().unwrap();
        assert_eq!(stored.content, msg.content);
        let block = crate::grpc::ai::agent::pb::ContentBlock::from(stored.content[0].clone());
        assert_eq!(
            block.block,
            Some(crate::grpc::ai::agent::pb::content_block::Block::Thinking(label.to_owned()))
        );
        let mut transcript = Box::pin(rebuilt.transcript(&msg.session));
        assert_eq!(transcript.next().await.unwrap().unwrap(), format!("assistant: {label}\n"));
    }

    #[rstest]
    #[case(false)]
    #[case(true)]
    #[tokio::test]
    async fn reasoning_counts_survive_failed_writes_and_restart(#[case] fail_projection: bool) {
        let dir = tempfile::tempdir().unwrap();
        let records_path = dir.path().join("records.db");
        let sidecar_path = dir.path().join("sessions.db");
        let store = SqliteStore::new(records_path.as_os_str(), NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sidecar = AiSessionDatabase::open(&sidecar_path).await.unwrap();
        let sink = Sink::new(records.clone(), sidecar.clone());
        let mut tail = sink.tail.subscribe();
        let path = if fail_projection {
            &sidecar_path
        } else {
            &records_path
        };
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        let sql = if fail_projection {
            "CREATE TRIGGER fail_write BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL, \
             'injected failure'); END"
        } else {
            "CREATE TRIGGER fail_write BEFORE INSERT ON store BEGIN SELECT RAISE(FAIL, 'injected \
             failure'); END"
        };
        atuin_common::db::query(sql).execute(fault.pool()).await.unwrap();
        let message = |source: &str| {
            let mut msg = sample_message();
            msg.id = atuin_domain::record::RecordId(atuin_common::utils::uuid_v7());
            msg.source_id = source.to_owned().into();
            msg.turn_id = Some("call".to_owned());
            // Exercise the compressed-content lookup too.
            msg.content = vec![Content::Text("hello ".repeat(100)), Content::ReasoningSummary {
                tokens: Some(185),
            }];
            msg
        };
        assert!(sink.append(message("first")).await.is_err());
        atuin_common::db::query("DROP TRIGGER fail_write").execute(fault.pool()).await.unwrap();
        sink.append(message("second")).await.unwrap();
        let mut delivered = Vec::new();
        while let Ok(event) = tail.try_recv() {
            if let SessionTailEvent::Message(msg) = event {
                delivered.push(msg.source_id.to_string());
            }
        }
        let expected = if fail_projection {
            vec!["first", "second"]
        } else {
            vec!["second"]
        };
        assert_eq!(delivered, expected);
        drop(sink);
        // No in-memory dedup state survives this restart.
        let sink = Sink::new(records.clone(), sidecar);
        sink.append(message("third")).await.unwrap();
        // A replay is also harmless.
        sink.append(message("second")).await.unwrap();
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        records.build(&rebuilt).await.unwrap();
        let mut rows = Box::pin(rebuilt.messages(&sample_handle()));
        let mut counts = Vec::new();
        while let Some(row) = rows.next().await {
            for block in row.unwrap().content {
                if let Content::ReasoningSummary { tokens: Some(n) } = block {
                    counts.push(n);
                }
            }
        }
        assert_eq!(counts, vec![185]);
    }

    #[rstest]
    #[tokio::test]
    async fn incomplete_startup_recovery_disables_writers_until_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let records = mem_store().await;
        let sidecar = AiSessionDatabase::open(&path).await.unwrap();
        let mut msg = sample_message();
        msg.turn_id = Some("call".to_owned());
        msg.content = vec![Content::ReasoningSummary { tokens: Some(42) }];
        // Simulate a crash after the record commit but before sidecar projection.
        records.push(&msg).await.unwrap();
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query(
            "CREATE TRIGGER fail_write BEFORE INSERT ON messages BEGIN SELECT RAISE(FAIL, \
             'injected failure'); END",
        )
        .execute(fault.pool())
        .await
        .unwrap();
        let recovered = records.build(&sidecar).await.is_ok();
        assert!(!recovered);
        let capture = AiHarnessSessionCapture::open(
            records.clone(),
            sidecar.clone(),
            false,
            recovered,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        assert!(!capture.persistent);
        let mut import = Box::pin(capture.import(None));
        assert!(matches!(import.next().await.unwrap(), ImportProgress::Finished {
            imported: 0,
            ..
        }));
        assert!(import.next().await.is_none());
        drop(capture);
        atuin_common::db::query("DROP TRIGGER fail_write").execute(fault.pool()).await.unwrap();
        records.build(&sidecar).await.unwrap();
        let capture = AiHarnessSessionCapture::open(
            records,
            sidecar,
            false,
            true,
            BlockingPool::new(NonZeroUsize::MIN),
        );
        msg.id = RecordId(atuin_common::utils::uuid_v7());
        msg.source_id = "later".to_owned().into();
        capture.sink.append(msg.clone()).await.unwrap();
        let mut rows = Box::pin(capture.messages(&msg.session));
        let mut counts = Vec::new();
        while let Some(row) = rows.next().await {
            for block in row.unwrap().content {
                if let Content::ReasoningSummary { tokens: Some(n) } = block {
                    counts.push(n);
                }
            }
        }
        assert_eq!(counts, vec![42]);
    }

    #[rstest]
    #[tokio::test]
    async fn concurrent_rows_do_not_duplicate_reasoning_counts() {
        let records = mem_store().await;
        let sink = Sink::new(records.clone(), AiSessionDatabase::in_memory().await.unwrap());
        let mut first = sample_message();
        first.turn_id = Some("call".to_owned());
        first.content = vec![Content::ReasoningSummary { tokens: Some(42) }; 2];
        let mut second = first.clone();
        second.id = atuin_domain::record::RecordId(atuin_common::utils::uuid_v7());
        second.source_id = "second".to_owned().into();
        let (a, b) = tokio::join!(sink.append(first), sink.append(second));
        a.unwrap();
        b.unwrap();
        let rebuilt = AiSessionDatabase::in_memory().await.unwrap();
        records.build(&rebuilt).await.unwrap();
        let mut rows = Box::pin(rebuilt.messages(&sample_handle()));
        let mut counts = Vec::new();
        while let Some(row) = rows.next().await {
            for block in row.unwrap().content {
                if let Content::ReasoningSummary { tokens: Some(n) } = block {
                    counts.push(n);
                }
            }
        }
        assert_eq!(counts, vec![42]);
    }

    #[rstest]
    #[tokio::test]
    async fn split_and_interleaved_adapter_rows_report_counts_once() {
        use atuin_common::harnesstools::session::{AnyMessage, SessionId};
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let mut enricher = super::message_enricher::MessageEnricher::new(HarnessKind::ClaudeCode);
        let session = SessionId::from("native-session".to_owned());
        for (index, (turn, thinking, reported, expected)) in [
            ("a", true, None, None),
            ("a", false, Some(185), Some(185)),
            ("b", true, Some(42), Some(42)),
            ("a", false, Some(185), None),
        ]
        .into_iter()
        .enumerate()
        {
            let block = if thinking {
                serde_json::json!({"type": "redacted_thinking", "data": "PRIVATE"})
            } else {
                serde_json::json!({"type": "text", "text": "hello"})
            };
            let m = AnyMessage::Ccode(
                serde_json::from_value(serde_json::json!({
                    "type": "assistant", "uuid": format!("u{index}"),
                    "message": {"role": "assistant", "id": turn, "content": [block],
                        "usage": {"output_tokens": 999,
                            "output_tokens_details": {"thinking_tokens": reported}}}
                }))
                .unwrap(),
            );
            let msg = enricher.capture(&session, &m).unwrap();
            sink.append(msg.clone()).await.unwrap();
            let mut rows = Box::pin(sink.sidecar.messages(&msg.session));
            let mut found = false;
            while let Some(row) = rows.next().await {
                let row = row.unwrap();
                if row.source_id == msg.source_id {
                    assert!(row.content.contains(&Content::ReasoningSummary { tokens: expected }));
                    found = true;
                }
            }
            assert!(found);
        }
    }

    #[rstest]
    fn adapters_keep_reasoning_presence_without_payloads() {
        use atuin_common::harnesstools::session::{AnyMessage, Message as _};
        let claude = AnyMessage::Ccode(
            serde_json::from_value(serde_json::json!({
                "type": "assistant", "message": {"role": "assistant",
                    "content": [{"type": "thinking", "thinking": "PRIVATE_REASONING"}],
                    "usage": {"output_tokens": 999}}
            }))
            .unwrap(),
        );
        let pi = AnyMessage::Pi(
            serde_json::from_value(serde_json::json!({
                "type": "message", "id": "p1", "message": {"role": "assistant",
                    "content": [{"type": "thinking", "thinking": "PRIVATE_REASONING"}]}
            }))
            .unwrap(),
        );
        let codex = AnyMessage::Codex(
            serde_json::from_value(serde_json::json!({
                "type": "response_item", "payload": {"type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "PRIVATE_REASONING"}],
                    "encrypted_content": "PRIVATE_ENCRYPTED"}
            }))
            .unwrap(),
        );
        for m in [claude, pi, codex] {
            assert_eq!(m.content(), vec![Content::ReasoningSummary { tokens: None }]);
        }
    }

    #[rstest]
    #[case(Role::System)]
    #[case(Role::Tool)]
    #[case(Role::Other("custom".to_owned()))]
    fn non_conversation_text_is_omitted(#[case] role: Role) {
        let mut msg = sample_message();
        msg.role = role;
        sanitize_message(&mut msg);
        assert!(msg.content.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn append_returns_new_then_duplicate() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());
        let msg = sample_message();

        assert_eq!(sink.append(msg.clone()).await.unwrap(), Appended::New);
        assert_eq!(sink.append(msg).await.unwrap(), Appended::Duplicate);
    }

    #[rstest]
    #[tokio::test]
    async fn append_without_subscriber_does_not_error() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());

        sink.append(sample_message()).await.unwrap();

        let session = sink.sidecar.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(session.message_count, 1);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_appends_of_one_message_write_a_single_record() {
        let raw = SqliteStore::in_memory(NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(raw.clone())
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        let sink = Arc::new(Sink::new(records, AiSessionDatabase::in_memory().await.unwrap()));

        let outcomes = futures::future::join_all((0..8).map(|_| {
            let sink = Arc::clone(&sink);
            let msg = sample_message();
            async move { sink.append(msg).await.unwrap() }
        }))
        .await;

        assert_eq!(outcomes.iter().filter(|a| matches!(a, Appended::New)).count(), 1);
        assert_eq!(raw.all_tagged(&RecordTag::AiSession).await.unwrap().len(), 1);
        assert_eq!(
            sink.sidecar.get_session(&sample_handle()).await.unwrap().unwrap().message_count,
            1
        );
    }
}
