use std::sync::Arc;

use atuin_client::ai_session::{HarnessKind, HarnessSession, Message, NativeSessionId, SourceId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{
    AnyMessage, Message as HarnessMessage, SessionEvent, SessionEventKind, SessionId,
};
use atuin_domain::record::RecordId;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio::task::JoinHandle;

use super::Sink;

pub struct SessionCaptureEngine {
    listeners: Vec<JoinHandle<()>>,
}

impl SessionCaptureEngine {
    pub fn nop() -> Self {
        Self {
            listeners: Vec::new(),
        }
    }

    pub fn spawn(sink: &Arc<Sink>) -> Self {
        let mut listeners = Vec::new();

        for harness in AnyHarness::all() {
            let Some(sessions) = harness.sessions() else {
                continue;
            };
            let Ok(listener) = sessions.listener() else {
                continue;
            };
            let kind = HarnessKind::from(harness);
            let sink = sink.clone();

            listeners.push(tokio::spawn(async move {
                let mut events = listener.events();
                while let Some(ev) = events.next().await {
                    match ev {
                        Ok(SessionEvent {
                            session,
                            kind: SessionEventKind::Started(meta),
                        }) => {
                            let handle = HarnessSession {
                                harness: kind,
                                session: NativeSessionId::from(session.to_string()),
                            };
                            let _ = sink.record_session_meta(&handle, &meta).await;
                        }
                        Ok(SessionEvent {
                            session,
                            kind: SessionEventKind::Message(m),
                        }) => {
                            let msg = Self::enrich(kind, &session, &m);
                            let _ = sink.append(msg).await;
                        }
                        Err(e) => tracing::warn!(?e, "capture error"),
                    }
                }
            }));
        }

        Self { listeners }
    }

    fn enrich(kind: HarnessKind, session: &SessionId, m: &AnyMessage) -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(HarnessSession {
                harness: kind,
                session: NativeSessionId::from(session.to_string()),
            })
            .source_id(
                m.id()
                    .map(|id| SourceId::from(String::from(id)))
                    .unwrap_or_else(|| SourceId::from(atuin_common::utils::uuid_v7().to_string())),
            )
            .timestamp(m.timestamp().unwrap_or_else(OffsetDateTime::now_utc))
            .role(m.role())
            .content(m.content())
            .model(m.model())
            .usage(m.usage())
            .stop_reason(m.stop_reason())
            .cwd(m.cwd())
            .git_branch(m.git_branch())
            .build()
    }
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}
