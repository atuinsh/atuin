use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{HarnessKind, HarnessSession, NativeSessionId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{Checkpoint, RuntimeError, SessionEvent, SessionId};
use atuin_common::sync::BlockingPool;
use futures::StreamExt;
use tokio::task::JoinHandle;

use super::Sink;
use super::message_enricher::MessageEnricher;

/// Backoff bounds for retrying a harness listener whose session directory does not exist yet.
const LISTENER_RETRY_START: Duration = Duration::from_secs(2);
const LISTENER_RETRY_MAX: Duration = Duration::from_secs(60);

pub struct SessionCaptureEngine {
    listeners: Vec<JoinHandle<()>>,
}

impl SessionCaptureEngine {
    pub fn nop() -> Self {
        Self {
            listeners: Vec::new(),
        }
    }

    pub fn spawn(sink: &Arc<Sink>, pool: &BlockingPool) -> Self {
        let mut listeners = Vec::new();

        for harness in AnyHarness::all() {
            let Some(sessions) = harness.sessions(pool) else {
                continue;
            };
            let kind = HarnessKind::from(harness);
            let sink = sink.clone();

            listeners.push(tokio::spawn(async move {
                // A harness whose session directory does not exist yet (not installed, or never
                // run) must not be dropped for the life of the daemon: retry with capped backoff
                // until it appears, so capture starts without a restart.
                let mut delay = LISTENER_RETRY_START;
                let listener = loop {
                    match sessions.listener() {
                        Ok(listener) => break listener,
                        Err(RuntimeError::NotFound(_)) => {
                            tokio::time::sleep(delay).await;
                            delay = (delay * 2).min(LISTENER_RETRY_MAX);
                        }
                        Err(e) => {
                            tracing::warn!(
                                ?e,
                                ?kind,
                                "ai-session listener failed; capture disabled for this harness"
                            );
                            return;
                        }
                    }
                };

                let checkpoint = {
                    let sink = sink.clone();
                    move |id: &SessionId| {
                        let (sink, id) = (sink.clone(), id.clone());
                        async move { checkpoint_of(&sink, kind, &id).await }
                    }
                };
                let mut events = listener.events(checkpoint);
                let mut enricher = MessageEnricher::new(kind);
                // Sessions with a failed append: their checkpoint must not move past the line
                // that was lost, or a restart would never re-read it.
                let mut stuck: HashSet<SessionId> = HashSet::new();

                while let Some(ev) = events.next().await {
                    match ev {
                        Ok(SessionEvent {
                            session,
                            checkpoint,
                            message,
                        }) => {
                            // A session's first line this run: warm its bookkeeping from the
                            // sidecar so a transcript resumed past its start keeps its title,
                            // parent and usage dedupe. Harmless for one read from the start,
                            // since every replayed line is a duplicate until the new ones.
                            if enricher.is_new(&session) {
                                let handle = enricher.handle(&session);
                                let row = sink.sidecar.get_session(&handle).await.unwrap_or_else(
                                    |e| {
                                        tracing::warn!(?e, %session, "failed to load the ai-session row");
                                        None
                                    },
                                );
                                let last = sink.sidecar.last_message(&handle).await.unwrap_or_else(
                                    |e| {
                                        tracing::warn!(?e, %session, "failed to load the last ai-session message");
                                        None
                                    },
                                );
                                enricher.seed(&session, row.as_ref(), last.as_ref());
                            }
                            let Some(msg) = enricher.capture(&session, &message) else {
                                continue;
                            };
                            if let Err(e) = sink.append(msg).await {
                                tracing::warn!(?e, "failed to capture ai-session message");
                                stuck.insert(session);
                                continue;
                            }
                            if stuck.contains(&session) {
                                continue;
                            }
                            // ponytail: one checkpoint write per row; batch per session on idle
                            // if it shows up in profiles.
                            let handle = enricher.handle(&session);
                            if let Err(e) = sink.sidecar.set_checkpoint(&handle, checkpoint).await {
                                tracing::warn!(?e, "failed to record ai-session checkpoint");
                            }
                        }
                        Err(e) => tracing::warn!(?e, "capture error"),
                    }
                }
            }));
        }

        Self { listeners }
    }
}

/// The checkpoint stored for a session, if any; the session checks it against its source itself.
async fn checkpoint_of(sink: &Sink, kind: HarnessKind, session: &SessionId) -> Option<Checkpoint> {
    sink.sidecar.checkpoint(&handle_of(kind, session)).await.unwrap_or_else(|e| {
        tracing::warn!(?e, %session, "failed to read ai-session checkpoint; reading from the start");
        None
    })
}

fn handle_of(kind: HarnessKind, session: &SessionId) -> HarnessSession {
    HarnessSession {
        harness: kind,
        session: NativeSessionId::from(session.to_string()),
    }
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}
