use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use atuin_client::ai_session::{HarnessKind, HarnessSession, Message, NativeSessionId, SourceId};
use atuin_common::harnesstools::session::{
    Appearance, Message as HMessage, MessageError, ReadFrom, Session as HSession, SessionMeta,
};
use atuin_domain::record::RecordId;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio::time::timeout;

use crate::session_capture::Sink;

const FOLLOW_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const RECENT_ACTIVITY_WINDOW: time::Duration = time::Duration::minutes(15);

pub(super) struct SessionActor<S> {
    kind: HarnessKind,
    sink: Arc<Sink>,
    followers: Arc<AtomicUsize>,
    appearance: Appearance<S>,
}

struct FollowerGuard(Arc<AtomicUsize>);

impl FollowerGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for FollowerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<S> SessionActor<S>
where
    S: HSession + Send + 'static,
{
    pub(super) fn new(
        kind: HarnessKind,
        sink: Arc<Sink>,
        followers: Arc<AtomicUsize>,
        appearance: Appearance<S>,
    ) -> Self {
        Self {
            kind,
            sink,
            followers,
            appearance,
        }
    }

    pub(super) async fn run(self) {
        let SessionActor {
            kind,
            sink,
            followers,
            appearance,
        } = self;

        let (session, follow, meta) = match appearance {
            Appearance::Appeared(session) => {
                let meta = session.meta().await.ok();
                (session, true, meta)
            }
            Appearance::Existing(session) => {
                let meta = session.meta().await.ok();
                let recent = meta.as_ref().is_none_or(Self::is_recent);
                (session, recent, meta)
            }
        };

        let native = NativeSessionId::from(String::from(session.id()));
        let from = sink.sidecar.checkpoint(kind, &native).await.unwrap_or(ReadFrom::Beginning);

        if follow {
            let _guard = FollowerGuard::new(followers.clone());
            let mut stream = session.messages_from(from);
            while let Ok(Some((offset, result))) = timeout(FOLLOW_IDLE_TIMEOUT, stream.next()).await
            {
                Self::ingest(kind, &native, meta.as_ref(), &sink, offset, result).await;
            }
        } else {
            let mut stream = session.messages_once_from(from);
            while let Some((offset, result)) = stream.next().await {
                Self::ingest(kind, &native, meta.as_ref(), &sink, offset, result).await;
            }
        }
    }

    fn is_recent(meta: &SessionMeta) -> bool {
        OffsetDateTime::now_utc() - meta.started_at < RECENT_ACTIVITY_WINDOW
    }

    async fn ingest(
        kind: HarnessKind,
        native: &NativeSessionId,
        meta: Option<&SessionMeta>,
        sink: &Sink,
        offset: u64,
        result: Result<S::Message, MessageError>,
    ) {
        if let Ok(message) = result {
            let _ = sink.append(Self::enrich(kind, native, meta, offset, &message)).await;
        }
        let _ = sink.sidecar.set_checkpoint(kind, native, offset).await;
    }

    fn enrich(
        kind: HarnessKind,
        native: &NativeSessionId,
        meta: Option<&SessionMeta>,
        offset: u64,
        message: &S::Message,
    ) -> Message {
        let source_id = message
            .id()
            .map(|id| SourceId::from(String::from(id)))
            .unwrap_or_else(|| SourceId::from(offset.to_string()));

        let model = message.model().or_else(|| meta.and_then(|meta| meta.model.clone()));

        let parent = meta.and_then(|meta| meta.parent.as_ref()).map(|parent| HarnessSession {
            harness: HarnessKind::from(parent.harness),
            session: NativeSessionId::from(String::from(parent.session.clone())),
        });

        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(HarnessSession {
                harness: kind,
                session: native.clone(),
            })
            .source_id(source_id)
            .parent(parent)
            .timestamp(message.timestamp().unwrap_or_else(OffsetDateTime::now_utc))
            .role(message.role())
            .content(message.content())
            .cwd(meta.and_then(|meta| meta.cwd.clone()))
            .git_branch(meta.and_then(|meta| meta.git_branch.clone()))
            .model(model)
            .usage(message.usage())
            .stop_reason(message.stop_reason())
            .build()
    }
}
