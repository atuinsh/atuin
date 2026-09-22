use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{HarnessKind, HarnessSession, Message, NativeSessionId, SourceId};
use atuin_common::harnesstools::AnyHarness;
use atuin_common::harnesstools::session::{
    AnyMessage, Message as HarnessMessage, RuntimeError, SessionEvent, SessionEventKind, SessionId,
    SessionMeta,
};
use atuin_domain::record::RecordId;
use futures::StreamExt;
use time::OffsetDateTime;
use tokio::task::JoinHandle;

/// Backoff bounds for retrying a harness listener whose session directory does not exist yet.
const LISTENER_RETRY_START: Duration = Duration::from_secs(2);
const LISTENER_RETRY_MAX: Duration = Duration::from_secs(60);

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
                // Latest known title per native session id, stamped onto each captured message so
                // session-level metadata rides the synced records (see `Message::session_title`).
                let mut titles: HashMap<String, String> = HashMap::new();
                // Lines without a timestamp (Claude Code `ai-title` and friends) take the
                // previous line's, so a replayed session is not stamped with capture time.
                let mut last_ts: HashMap<String, OffsetDateTime> = HashMap::new();
                // Session each one was spawned from, when it has one (subagents, forks).
                let mut parents: HashMap<String, NativeSessionId> = HashMap::new();
                // Last model call seen per session: later rows of the same call repeat its usage,
                // so only the first keeps it.
                let mut last_turn: HashMap<String, String> = HashMap::new();

                while let Some(ev) = events.next().await {
                    match ev {
                        Ok(SessionEvent {
                            session,
                            kind: SessionEventKind::Started(meta),
                        }) => {
                            if let Some(title) = &meta.title {
                                titles.insert(session.to_string(), title.clone());
                            }
                            if let Some(parent) = &meta.parent {
                                parents.insert(
                                    session.to_string(),
                                    NativeSessionId::from(parent.to_string()),
                                );
                            }
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
                            let key = session.to_string();
                            let timestamp = match m.timestamp() {
                                Some(ts) => *last_ts.entry(key.clone()).insert_entry(ts).get(),
                                None => last_ts
                                    .get(&key)
                                    .copied()
                                    .unwrap_or_else(OffsetDateTime::now_utc),
                            };
                            // ponytail: newest title wins; a hand-set title is not ranked above a
                            // later generated one.
                            if let Some(title) = m.title() {
                                titles.insert(key.clone(), title.clone());
                                let handle = HarnessSession {
                                    harness: kind,
                                    session: NativeSessionId::from(key.clone()),
                                };
                                let meta = SessionMeta {
                                    title: Some(title),
                                    ..SessionMeta::default()
                                };
                                if let Err(e) = sink.record_session_meta(&handle, &meta).await {
                                    tracing::warn!(?e, "failed to record ai-session title");
                                }
                            }
                            if let Some(parent) = m.parent_session().filter(|p| *p != session) {
                                parents
                                    .insert(key.clone(), NativeSessionId::from(parent.to_string()));
                            }
                            let repeat = m.turn_id().is_some_and(|t| {
                                last_turn.insert(key.clone(), t.clone()).as_ref() == Some(&t)
                            });
                            let title = titles.get(&key).cloned();
                            let parent = parents.get(&key).cloned();
                            let Some(mut msg) =
                                Self::enrich(kind, &session, &m, title, timestamp, parent)
                            else {
                                continue;
                            };
                            if repeat {
                                msg.usage = None;
                            }
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

    /// `None` for a line that carries nothing worth a row: harness bookkeeping (Claude Code
    /// attachments and mode switches, Codex `item_completed` twins) with no content, usage,
    /// stop reason, model or title.
    fn enrich(
        kind: HarnessKind,
        session: &SessionId,
        m: &AnyMessage,
        session_title: Option<String>,
        timestamp: OffsetDateTime,
        parent: Option<NativeSessionId>,
    ) -> Option<Message> {
        let content = m.content();
        let usage = m.usage();
        let stop_reason = m.stop_reason();
        let model = m.model();
        if content.is_empty()
            && usage.is_none()
            && stop_reason.is_none()
            && model.is_none()
            && m.title().is_none()
        {
            return None;
        }

        Some(
            Message::builder()
                .id(RecordId(atuin_common::utils::uuid_v7()))
                .session(HarnessSession {
                    harness: kind,
                    session: NativeSessionId::from(session.to_string()),
                })
                .source_id(Self::source_id(session, m))
                .parent(parent.map(|session| HarnessSession {
                    harness: kind,
                    session,
                }))
                .parent_source_id(m.parent_id().map(|id| SourceId::from(String::from(id))))
                .turn_id(m.turn_id())
                .timestamp(timestamp)
                .role(m.role())
                .content(content)
                .model(model)
                .usage(usage)
                .stop_reason(stop_reason)
                .cwd(m.cwd())
                .git_branch(m.git_branch())
                .session_title(session_title)
                .build(),
        )
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
        let title = m.title().unwrap_or_default();
        let canonical = format!("{session}\u{1f}{ts}\u{1f}{role}\u{1f}{content}\u{1f}{title}");
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

#[cfg(test)]
mod tests {
    use atuin_common::harnesstools::ccode::session::CcodeMessage;
    use atuin_common::harnesstools::session::{Content, Role};
    use rstest::rstest;

    use super::*;

    fn ccode(raw: &serde_json::Value) -> AnyMessage {
        AnyMessage::Ccode(serde_json::from_str::<CcodeMessage>(&raw.to_string()).unwrap())
    }

    fn session() -> SessionId {
        SessionId::from("s1".to_owned())
    }

    #[rstest]
    #[case(serde_json::json!({"type": "attachment", "uuid": "a1", "cwd": "/x", "attachment": {"type": "date"}}))]
    #[case(serde_json::json!({"type": "mode", "mode": "default"}))]
    #[case(serde_json::json!({"type": "last-prompt", "leafUuid": "a1"}))]
    fn bookkeeping_lines_produce_no_row(#[case] raw: serde_json::Value) {
        let m = ccode(&raw);
        assert!(
            SessionCaptureEngine::enrich(
                HarnessKind::ClaudeCode,
                &session(),
                &m,
                None,
                OffsetDateTime::UNIX_EPOCH,
                None,
            )
            .is_none()
        );
    }

    #[rstest]
    fn title_lines_keep_a_row_stamped_with_the_given_timestamp() {
        let m = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "Fix it"}));
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let msg = SessionCaptureEngine::enrich(
            HarnessKind::ClaudeCode,
            &session(),
            &m,
            Some("Fix it".to_owned()),
            ts,
            None,
        )
        .unwrap();
        assert_eq!(msg.timestamp, ts);
        assert_eq!(msg.session_title.as_deref(), Some("Fix it"));
        assert!(msg.content.is_empty());
    }

    #[rstest]
    fn distinct_titles_get_distinct_source_ids() {
        let a = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "one"}));
        let b = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "two"}));
        assert_ne!(
            SessionCaptureEngine::source_id(&session(), &a),
            SessionCaptureEngine::source_id(&session(), &b)
        );
    }

    #[rstest]
    #[case(None, None)]
    #[case(Some("p"), Some("p"))]
    fn tree_fields_ride_the_message(#[case] parent: Option<&str>, #[case] expected: Option<&str>) {
        let m = ccode(&serde_json::json!({
            "type": "assistant", "uuid": "u2", "parentUuid": "u1", "sessionId": "p",
            "message": {"role": "assistant", "id": "msg_01", "content": [{"type": "text", "text": "hi"}]},
        }));
        let msg = SessionCaptureEngine::enrich(
            HarnessKind::ClaudeCode,
            &session(),
            &m,
            None,
            OffsetDateTime::UNIX_EPOCH,
            parent.map(|p| NativeSessionId::from(p.to_owned())),
        )
        .unwrap();
        assert_eq!(msg.parent_source_id, Some(SourceId::from("u1".to_owned())));
        assert_eq!(
            msg.parent.map(|p| p.session),
            expected.map(|p| NativeSessionId::from(p.to_owned()))
        );
        assert_eq!(msg.turn_id.as_deref(), Some("msg_01"));
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, vec![Content::Text("hi".into())]);
    }

    /// The spawn loop's per-session bookkeeping, driven directly: the second row of one model
    /// call drops its repeated usage but is still a row.
    #[rstest]
    fn repeated_turn_keeps_the_row_and_drops_the_usage() {
        let line = |uuid: &str, block: serde_json::Value| {
            ccode(&serde_json::json!({
                "type": "assistant", "uuid": uuid, "sessionId": "s1",
                "message": {"role": "assistant", "id": "msg_01", "content": [block],
                    "usage": {"input_tokens": 1, "output_tokens": 2}},
            }))
        };
        let first = line("u1", serde_json::json!({"type": "text", "text": "hi"}));
        let second = line(
            "u2",
            serde_json::json!({"type": "tool_use", "id": "t", "name": "Bash", "input": {}}),
        );

        let mut last_turn: HashMap<String, String> = HashMap::new();
        let mut rows = Vec::new();
        for m in [first, second] {
            let repeat = m
                .turn_id()
                .is_some_and(|t| last_turn.insert("s1".to_owned(), t.clone()).as_ref() == Some(&t));
            let mut msg = SessionCaptureEngine::enrich(
                HarnessKind::ClaudeCode,
                &session(),
                &m,
                None,
                OffsetDateTime::UNIX_EPOCH,
                None,
            )
            .unwrap();
            if repeat {
                msg.usage = None;
            }
            rows.push(msg);
        }
        assert!(rows[0].usage.is_some());
        assert!(rows[1].usage.is_none());
        assert_eq!(rows[1].content.len(), 1);
    }
}
