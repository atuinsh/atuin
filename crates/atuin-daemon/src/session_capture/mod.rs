mod engine;
pub(crate) mod proto;

use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{
    AiSessionDatabase, AiSessionStore, Appended, DbError, HarnessKind, HarnessSession, Message,
    PushError, Session,
};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::encryption::paseto_v4::Key;
use atuin_domain::record::HostId;
use engine::SessionCaptureEngine;
use futures::Stream;
use tokio::sync::broadcast;
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
}

impl Sink {
    pub(crate) fn new(records: AiSessionStore, sidecar: AiSessionDatabase) -> Self {
        let (tail, _) = broadcast::channel(128);
        Self {
            records,
            sidecar,
            tail,
        }
    }

    pub(crate) fn subscribe(&self) -> BroadcastStream<SessionTailEvent> {
        BroadcastStream::new(self.tail.subscribe())
    }

    pub(crate) async fn append(&self, msg: Message) -> Result<(), AppendError> {
        let started = self.sidecar.get_session(&msg.session).await?.is_none();
        let appended = self.sidecar.append(&msg).await?;

        if appended != Appended::New {
            return Ok(());
        }

        self.records.push(&msg).await?;

        if self.tail.receiver_count() > 0 {
            if let Some(session) = self.sidecar.get_session(&msg.session).await? {
                let event = if started {
                    SessionTailEvent::SessionStarted(session)
                } else {
                    SessionTailEvent::SessionUpdated(session)
                };
                let _ = self.tail.send(event);
            }
            let _ = self.tail.send(SessionTailEvent::Message(msg));
        }

        Ok(())
    }
}

pub struct AiHarnessSessionCapture {
    sink: Arc<Sink>,
    _engine: SessionCaptureEngine,
}

impl AiHarnessSessionCapture {
    pub fn open(records: AiSessionStore, sidecar: AiSessionDatabase) -> Self {
        let sink = Arc::new(Sink::new(records, sidecar));
        let engine = SessionCaptureEngine::spawn(sink.clone());
        Self {
            sink,
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
            _engine: SessionCaptureEngine::detached(),
        }
    }

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
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::{Content, Role};
    use atuin_domain::record::RecordId;
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
    #[tokio::test]
    async fn append_without_subscriber_does_not_error() {
        let sink = Sink::new(mem_store().await, AiSessionDatabase::in_memory().await.unwrap());

        sink.append(sample_message()).await.unwrap();

        let session = sink.sidecar.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(session.message_count, 1);
    }
}
