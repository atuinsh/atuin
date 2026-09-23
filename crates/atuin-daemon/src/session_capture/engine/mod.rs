use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::HarnessKind;
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{RuntimeError, SessionEvent};
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

    pub fn spawn(sink: &Arc<Sink>) -> Self {
        let mut listeners = Vec::new();

        for harness in AnyHarness::all() {
            let Some(sessions) = harness.sessions() else {
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

                let mut events = listener.events();
                let mut enricher = MessageEnricher::new(kind);

                while let Some(ev) = events.next().await {
                    match ev {
                        Ok(SessionEvent { session, message }) => {
                            let Some(msg) = enricher.capture(&session, &message) else {
                                continue;
                            };
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
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}
