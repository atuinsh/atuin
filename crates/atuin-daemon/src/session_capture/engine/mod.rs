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
                let mut state = Bookkeeping::default();

                while let Some(ev) = events.next().await {
                    match ev {
                        Ok(SessionEvent {
                            session,
                            kind: SessionEventKind::Started(meta),
                        }) => {
                            let tracked = state.sessions.entry(session.to_string()).or_default();
                            if meta.title.is_some() {
                                tracked.title.clone_from(&meta.title);
                            }
                            if let Some(parent) = &meta.parent {
                                tracked.parent = Some(NativeSessionId::from(parent.to_string()));
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
                            let seen = state.observe(&session, &m);
                            if let Some(title) = seen.new_title {
                                let handle = HarnessSession {
                                    harness: kind,
                                    session: NativeSessionId::from(session.to_string()),
                                };
                                let meta = SessionMeta {
                                    title: Some(title),
                                    ..SessionMeta::default()
                                };
                                if let Err(e) = sink.record_session_meta(&handle, &meta).await {
                                    tracing::warn!(?e, "failed to record ai-session title");
                                }
                            }
                            let Some(mut msg) = Self::enrich(
                                kind,
                                &session,
                                &m,
                                seen.title,
                                seen.timestamp,
                                seen.parent,
                            ) else {
                                continue;
                            };
                            if seen.repeat {
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

    /// `None` for a line that carries nothing worth a row: harness bookkeeping (Claude Code mode
    /// switches, Codex `item_completed` twins) with no content, usage, stop reason, model or
    /// title. A line with its own id is a node of the transcript tree and keeps an empty row
    /// (Claude Code attachments), so no kept line's parent link dangles.
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
        if m.id().is_none()
            && content.is_empty()
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

/// What the capture loop carries from one line to the next, per native session id.
#[derive(Default)]
struct Bookkeeping {
    sessions: HashMap<String, SessionState>,
}

#[derive(Default)]
struct SessionState {
    /// Latest known title, stamped onto each captured message so session-level metadata rides
    /// the synced records (see `Message::session_title`).
    title: Option<String>,
    /// Timestamp of the last line that had one. Lines without (Claude Code `ai-title` and
    /// friends) take it, so a replayed session is not stamped with capture time.
    last_ts: Option<OffsetDateTime>,
    /// Session this one was spawned from, when it has one (subagents, forks).
    parent: Option<NativeSessionId>,
    /// Last model call that reported usage: later rows of the same call repeat its usage, so
    /// only the first keeps it.
    last_turn: Option<String>,
}

/// What [`Bookkeeping::observe`] resolved for one line.
struct Observed {
    timestamp: OffsetDateTime,
    /// The title this line set, if it set one.
    new_title: Option<String>,
    title: Option<String>,
    parent: Option<NativeSessionId>,
    /// This row repeats the usage an earlier row of the same model call already carried.
    repeat: bool,
}

impl Bookkeeping {
    fn observe(&mut self, session: &SessionId, m: &AnyMessage) -> Observed {
        let state = self.sessions.entry(session.to_string()).or_default();
        let timestamp = match m.timestamp() {
            Some(ts) => {
                state.last_ts = Some(ts);
                ts
            }
            None => state.last_ts.unwrap_or_else(OffsetDateTime::now_utc),
        };
        // ponytail: newest title wins; a hand-set title is not ranked above a later generated one.
        let new_title = m.title();
        if new_title.is_some() {
            state.title.clone_from(&new_title);
        }
        if let Some(parent) = m.parent_session().filter(|p| p != session) {
            state.parent = Some(NativeSessionId::from(parent.to_string()));
        }
        // Only a line that reports usage moves the marker: bookkeeping lines can share the turn
        // id (Codex `task_started`) while carrying nothing to dedupe.
        let repeat = m.usage().is_some()
            && m.turn_id().is_some_and(|t| state.last_turn.replace(t.clone()) == Some(t));

        Observed {
            timestamp,
            new_title,
            title: state.title.clone(),
            parent: state.parent.clone(),
            repeat,
        }
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
    use atuin_common::harnesstools::codex::session::CodexMessage;
    use atuin_common::harnesstools::session::{Content, Role};
    use rstest::rstest;

    use super::*;

    fn ccode(raw: &serde_json::Value) -> AnyMessage {
        AnyMessage::Ccode(serde_json::from_str::<CcodeMessage>(&raw.to_string()).unwrap())
    }

    fn session() -> SessionId {
        SessionId::from("s1".to_owned())
    }

    fn codex(raw: &serde_json::Value) -> AnyMessage {
        AnyMessage::Codex(serde_json::from_str::<CodexMessage>(&raw.to_string()).unwrap())
    }

    /// One line through the spawn loop's message arm: bookkeeping, then enrich, then the usage
    /// dedupe.
    fn capture(state: &mut Bookkeeping, kind: HarnessKind, m: &AnyMessage) -> Option<Message> {
        let seen = state.observe(&session(), m);
        let mut msg = SessionCaptureEngine::enrich(
            kind,
            &session(),
            m,
            seen.title,
            seen.timestamp,
            seen.parent,
        )?;
        if seen.repeat {
            msg.usage = None;
        }
        Some(msg)
    }

    #[rstest]
    fn attachment_lines_keep_an_empty_row_so_the_tree_stays_linked() {
        let m = ccode(&serde_json::json!({
            "type": "attachment", "uuid": "a1", "parentUuid": "u0", "cwd": "/x",
            "attachment": {"type": "hook_success", "stdout": "secret"},
        }));
        let msg = SessionCaptureEngine::enrich(
            HarnessKind::ClaudeCode,
            &session(),
            &m,
            None,
            OffsetDateTime::UNIX_EPOCH,
            None,
        )
        .unwrap();
        assert_eq!(msg.source_id, SourceId::from("a1".to_owned()));
        assert_eq!(msg.parent_source_id, Some(SourceId::from("u0".to_owned())));
        assert_eq!(msg.role, Role::Other("attachment".to_owned()));
        assert!(msg.content.is_empty());
    }

    #[rstest]
    #[case(serde_json::json!({"type": "mode", "mode": "default"}))]
    #[case(serde_json::json!({"type": "last-prompt", "leafUuid": "a1"}))]
    #[case(serde_json::json!({"type": "file-history-snapshot", "messageId": "m1", "snapshot": {}}))]
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

    /// A subagent line names the parent session; a main-session line names its own and gets
    /// no parent.
    #[rstest]
    #[case("s1", None)]
    #[case("p", Some("p"))]
    fn tree_fields_ride_the_message(#[case] line_session: &str, #[case] parent: Option<&str>) {
        let m = ccode(&serde_json::json!({
            "type": "assistant", "uuid": "u2", "parentUuid": "u1", "sessionId": line_session,
            "message": {"role": "assistant", "id": "msg_01", "content": [{"type": "text", "text": "hi"}]},
        }));
        let msg = capture(&mut Bookkeeping::default(), HarnessKind::ClaudeCode, &m).unwrap();
        assert_eq!(msg.parent_source_id, Some(SourceId::from("u1".to_owned())));
        assert_eq!(
            msg.parent.map(|p| p.session),
            parent.map(|p| NativeSessionId::from(p.to_owned()))
        );
        assert_eq!(msg.turn_id.as_deref(), Some("msg_01"));
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, vec![Content::Text("hi".into())]);
    }

    /// The second row of one model call drops its repeated usage but is still a row.
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

        let mut state = Bookkeeping::default();
        let rows: Vec<Message> = [first, second]
            .iter()
            .filter_map(|m| capture(&mut state, HarnessKind::ClaudeCode, m))
            .collect();
        assert!(rows[0].usage.is_some());
        assert!(rows[1].usage.is_none());
        assert_eq!(rows[1].content.len(), 1);
    }

    /// Codex names the turn on bookkeeping lines that carry no usage; they must not make the
    /// accounting line that follows look like a repeat, and each response keeps its own usage.
    #[rstest]
    fn codex_accounting_lines_keep_their_usage() {
        let started = codex(&serde_json::json!({
            "type": "event_msg", "payload": {"type": "task_started", "turn_id": "t1"},
        }));
        let usage = |response: &str, output: u64| {
            codex(&serde_json::json!({
                "type": "token_usage_record", "timestamp": "2026-09-18T10:00:00Z",
                "payload": {"turn_id": "t1", "response_id": response,
                    "usage": {"input_tokens": 1, "output_tokens": output}},
            }))
        };

        let mut state = Bookkeeping::default();
        assert!(capture(&mut state, HarnessKind::Codex, &started).is_none());
        let first = capture(&mut state, HarnessKind::Codex, &usage("r1", 5)).unwrap();
        let second = capture(&mut state, HarnessKind::Codex, &usage("r2", 7)).unwrap();
        assert_eq!(first.usage.and_then(|u| u.output), Some(5));
        assert_eq!(second.usage.and_then(|u| u.output), Some(7));
    }
}
