use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{HarnessKind, HarnessSession, NativeSessionId};
use atuin_common::fs::lines::line_ending_at;
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::ccode::session::CcodeMessage;
use atuin_common::harnesstools::codex::session::CodexMessage;
use atuin_common::harnesstools::pi::session::PiMessage;
use atuin_common::harnesstools::session::{AnyMessage, RuntimeError, SessionEvent, SessionId};
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

                let resume_from = {
                    let sink = sink.clone();
                    move |id: &SessionId, path: &Path| {
                        let sink = sink.clone();
                        let (id, path) = (id.clone(), path.to_path_buf());
                        async move { resume_point(&sink, kind, &id, &path).await }
                    }
                };
                let mut events = listener.events(resume_from);
                let mut enricher = MessageEnricher::new(kind);
                // Sessions with a failed append: their checkpoint must not move past the line
                // that was lost, or a restart would never re-read it.
                let mut stuck: HashSet<SessionId> = HashSet::new();

                while let Some(ev) = events.next().await {
                    match ev {
                        Ok(SessionEvent {
                            session,
                            offset,
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
                            if let Err(e) = sink.sidecar.set_checkpoint(&handle, offset).await {
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

/// Where to resume a session's transcript.
///
/// A saved offset is trusted only if the line ending exactly there is a message the sidecar
/// holds: a transcript rewritten in place to the same or a greater length would otherwise resume
/// mid-content and silently skip the changed lines, which the dedup gate cannot notice. Anything
/// else (no checkpoint, a shorter file, an unparseable or unknown line) reads from the start,
/// which only costs work.
async fn resume_point(sink: &Sink, kind: HarnessKind, session: &SessionId, path: &Path) -> u64 {
    let handle = HarnessSession {
        harness: kind,
        session: NativeSessionId::from(session.to_string()),
    };
    let offset = match sink.sidecar.checkpoint(&handle).await {
        Ok(Some(offset)) if offset > 0 => offset,
        Ok(_) => return 0,
        Err(e) => {
            tracing::warn!(?e, %session, "failed to read ai-session checkpoint; reading from the start");
            return 0;
        }
    };

    let line = match line_ending_at(path, offset).await {
        Ok(line) => line,
        Err(e) => {
            tracing::debug!(?e, %session, "could not read the transcript at its checkpoint");
            None
        }
    };
    let parsed: Option<AnyMessage> = line.and_then(|line| match kind {
        HarnessKind::ClaudeCode => {
            serde_json::from_slice::<CcodeMessage>(&line).ok().map(AnyMessage::from)
        }
        HarnessKind::Codex => {
            serde_json::from_slice::<CodexMessage>(&line).ok().map(AnyMessage::from)
        }
        HarnessKind::Pi => serde_json::from_slice::<PiMessage>(&line).ok().map(AnyMessage::from),
        _ => None,
    });
    let known = match parsed {
        Some(m) => sink
            .sidecar
            .contains_message(&handle, &MessageEnricher::source_id(session, &m))
            .await
            .unwrap_or(false),
        None => false,
    };
    if !known {
        tracing::debug!(
            %session,
            offset,
            "checkpoint does not end on a stored message; reading from the start"
        );
        return 0;
    }
    offset
}

impl Drop for SessionCaptureEngine {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{AiSessionDatabase, AiSessionStore, SourceId};
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_domain::record::HostId;
    use rstest::rstest;

    use super::*;

    async fn sink() -> Sink {
        let store = SqliteStore::in_memory(super::super::NOP_STORE_TIMEOUT).await.unwrap();
        let records = AiSessionStore::builder()
            .store(store)
            .host_id(HostId(atuin_common::utils::uuid_v7()))
            .key(Key::generate())
            .build();
        Sink::new(records, AiSessionDatabase::in_memory().await.unwrap())
    }

    fn line(uuid: &str) -> String {
        serde_json::json!({
            "type": "user", "uuid": uuid, "sessionId": "s1",
            "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": "user", "content": "hi"},
        })
        .to_string()
            + "\n"
    }

    /// A checkpoint is honoured only while the line it follows is still a stored message: a
    /// transcript rewritten in place to the same length, or cut shorter, reads from the start.
    #[rstest]
    #[tokio::test]
    async fn resume_point_validates_the_checkpoint_against_the_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.jsonl");
        let body = line("u1") + &line("u2");
        std::fs::write(&path, &body).unwrap();

        let sink = sink().await;
        let session = SessionId::from("s1".to_owned());
        let mut enricher = MessageEnricher::new(HarnessKind::ClaudeCode);
        for raw in [line("u1"), line("u2")] {
            let m: CcodeMessage = serde_json::from_str(raw.trim_end()).unwrap();
            let msg = enricher.capture(&session, &AnyMessage::from(m)).unwrap();
            sink.append(msg).await.unwrap();
        }
        let handle = enricher.handle(&session);
        let len = u64::try_from(body.len()).unwrap();
        sink.sidecar.set_checkpoint(&handle, len).await.unwrap();

        assert_eq!(resume_point(&sink, HarnessKind::ClaudeCode, &session, &path).await, len);
        assert_eq!(
            sink.sidecar.last_message(&handle).await.unwrap().unwrap().source_id,
            SourceId::from("u2".to_owned())
        );

        std::fs::write(&path, line("v1") + &line("v2")).unwrap();
        assert_eq!(
            resume_point(&sink, HarnessKind::ClaudeCode, &session, &path).await,
            0,
            "rewritten to the same length"
        );

        std::fs::write(&path, line("u1")).unwrap();
        assert_eq!(
            resume_point(&sink, HarnessKind::ClaudeCode, &session, &path).await,
            0,
            "shorter than the checkpoint"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn resume_point_starts_from_zero_without_a_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.jsonl");
        std::fs::write(&path, line("u1")).unwrap();
        let sink = sink().await;
        let session = SessionId::from("s1".to_owned());
        assert_eq!(resume_point(&sink, HarnessKind::ClaudeCode, &session, &path).await, 0);
    }
}
