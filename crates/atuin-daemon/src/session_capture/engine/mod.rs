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
                            if let Err(e) = sink.record_session_meta(&handle, &meta).await {
                                tracing::warn!(?e, "failed to record ai-session metadata");
                            }
                        }
                        Ok(SessionEvent {
                            session,
                            kind: SessionEventKind::Message(m),
                        }) => {
                            let msg = Self::enrich(kind, &session, &m);
                            if let Err(e) = sink.append(msg).await {
                                tracing::warn!(?e, "failed to capture ai-session message");
                            }
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
            .source_id(Self::source_id(session, m))
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

    /// A stable per-message identity for dedup. Harnesses that carry a native message id use it
    /// directly; otherwise we content-address the message so re-capturing it (daemon restart,
    /// transcript re-read) resolves to the same id instead of minting a fresh one each time.
    fn source_id(session: &SessionId, m: &AnyMessage) -> SourceId {
        if let Some(id) = m.id() {
            return SourceId::from(String::from(id));
        }

        let ts = m.timestamp().map_or(0, |t| t.unix_timestamp_nanos());
        let role = serde_json::to_string(&m.role()).unwrap_or_default();
        let content = serde_json::to_string(&m.content()).unwrap_or_default();
        let canonical = format!("{session}\u{1f}{ts}\u{1f}{role}\u{1f}{content}");
        SourceId::from(format!("syn-{:016x}", xxhash_rust::xxh3::xxh3_64(canonical.as_bytes())))
    }
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}
