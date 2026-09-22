use std::collections::HashMap;

use atuin_client::ai_session::{HarnessKind, HarnessSession, Message, NativeSessionId, SourceId};
use atuin_common::harnesstools::session::{
    AnyMessage, Message as HarnessMessage, SessionId, SessionMeta,
};
use atuin_domain::record::RecordId;
use time::OffsetDateTime;

/// Builds the canonical [`Message`] rows for one harness's lines, carrying the per-session
/// bookkeeping (title, timestamps, parent, usage dedupe) that a single line cannot resolve on its
/// own. The live capture loop and the backfill importer both drive it, so a line re-read later
/// resolves to the same row (see [`Normalizer::source_id`]).
pub struct Normalizer {
    harness: HarnessKind,
    state: Bookkeeping,
}

/// What [`Normalizer::capture`] produced for one line.
pub struct Captured {
    /// A title this line newly set, for the caller to persist as session metadata.
    pub new_title: Option<String>,
    /// The row for this line, or `None` for a bookkeeping line worth no row.
    pub row: Option<Message>,
}

impl Normalizer {
    pub fn new(harness: HarnessKind) -> Self {
        Self {
            harness,
            state: Bookkeeping::default(),
        }
    }

    pub fn handle(&self, session: &SessionId) -> HarnessSession {
        HarnessSession {
            harness: self.harness,
            session: NativeSessionId::from(session.to_string()),
        }
    }

    /// Fold a session's opening metadata into the bookkeeping, so lines seen before their own
    /// title or parent line still carry them.
    pub fn observe_started(&mut self, session: &SessionId, meta: &SessionMeta) {
        let tracked = self.state.sessions.entry(session.to_string()).or_default();
        if meta.title.is_some() {
            tracked.title.clone_from(&meta.title);
        }
        if let Some(parent) = &meta.parent {
            tracked.parent = Some(NativeSessionId::from(parent.to_string()));
        }
    }

    /// Observe a line, build its row, and drop repeated usage in one step. Reports any title the
    /// line set alongside the row (`None` row for a bookkeeping line worth no row).
    pub fn capture(&mut self, session: &SessionId, m: &AnyMessage) -> Captured {
        let seen = self.observe(session, m);
        let new_title = seen.new_title.clone();
        let mut row = self.build(session, m, &seen);
        if seen.repeat
            && let Some(msg) = &mut row
        {
            msg.usage = None;
        }
        Captured { new_title, row }
    }

    /// [`capture`](Self::capture) for callers that do not record session titles separately (the
    /// backfill importer). `None` for a bookkeeping line worth no row.
    pub fn enrich(&mut self, session: &SessionId, m: &AnyMessage) -> Option<Message> {
        self.capture(session, m).row
    }

    /// Advance the bookkeeping for one line and report what it resolved (see [`Observed`]).
    fn observe(&mut self, session: &SessionId, m: &AnyMessage) -> Observed {
        let state = self.state.sessions.entry(session.to_string()).or_default();
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

    /// `None` for a line that carries nothing worth a row: harness bookkeeping (Claude Code mode
    /// switches, Codex `item_completed` twins) with no content, usage, stop reason, model or
    /// title. A line with its own id is a node of the transcript tree and keeps an empty row
    /// (Claude Code attachments), so no kept line's parent link dangles.
    fn build(&self, session: &SessionId, m: &AnyMessage, seen: &Observed) -> Option<Message> {
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
                .session(self.handle(session))
                .source_id(Self::source_id(session, m))
                .parent(seen.parent.clone().map(|session| HarnessSession {
                    harness: self.harness,
                    session,
                }))
                .parent_source_id(m.parent_id().map(|id| SourceId::from(String::from(id))))
                .turn_id(m.turn_id())
                .timestamp(seen.timestamp)
                .role(m.role())
                .content(content)
                .model(model)
                .usage(usage)
                .stop_reason(stop_reason)
                .cwd(m.cwd())
                .git_branch(m.git_branch())
                .session_title(seen.title.clone())
                .build(),
        )
    }

    /// A stable per-message identity for dedup. Harnesses that carry a native message id use it
    /// directly; otherwise we content-address the message so re-capturing it (daemon restart,
    /// transcript re-read) resolves to the same id instead of minting a fresh one each time.
    pub fn source_id(session: &SessionId, m: &AnyMessage) -> SourceId {
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

/// What the capture pipeline carries from one line to the next, per native session id.
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

/// What [`Normalizer::observe`] resolved for one line.
struct Observed {
    timestamp: OffsetDateTime,
    /// The title this line set, if it set one.
    new_title: Option<String>,
    title: Option<String>,
    parent: Option<NativeSessionId>,
    /// This row repeats the usage an earlier row of the same model call already carried.
    repeat: bool,
}

#[cfg(test)]
mod tests {
    use atuin_common::harnesstools::ccode::session::CcodeMessage;
    use atuin_common::harnesstools::codex::session::CodexMessage;
    use atuin_common::harnesstools::pi::session::PiMessage;
    use atuin_common::harnesstools::session::{Content, Role};
    use rstest::rstest;

    use super::*;

    fn ccode(raw: &serde_json::Value) -> AnyMessage {
        AnyMessage::Ccode(serde_json::from_str::<CcodeMessage>(&raw.to_string()).unwrap())
    }

    fn codex(raw: &serde_json::Value) -> AnyMessage {
        AnyMessage::Codex(serde_json::from_str::<CodexMessage>(&raw.to_string()).unwrap())
    }

    fn session() -> SessionId {
        SessionId::from("s1".to_owned())
    }

    fn scripted_message_with_id(id: &str) -> AnyMessage {
        let raw = serde_json::json!({
            "type": "message",
            "id": id,
            "message": {"role": "user", "content": "hi"},
        })
        .to_string();
        AnyMessage::from(serde_json::from_str::<PiMessage>(&raw).unwrap())
    }

    fn scripted_message_without_id() -> AnyMessage {
        let raw = serde_json::json!({
            "type": "message",
            "message": {"role": "user", "content": "hi"},
        })
        .to_string();
        AnyMessage::from(serde_json::from_str::<PiMessage>(&raw).unwrap())
    }

    #[rstest]
    fn source_id_prefers_native_message_id() {
        let m = scripted_message_with_id("msg-1");
        assert_eq!(String::from(Normalizer::source_id(&session(), &m)), "msg-1".to_string());
    }

    #[rstest]
    fn synthetic_source_id_is_stable_and_prefixed() {
        let m = scripted_message_without_id();
        let a = Normalizer::source_id(&session(), &m);
        let b = Normalizer::source_id(&session(), &m);
        assert_eq!(a, b);
        assert!(String::from(a).starts_with("syn-"));
    }

    #[rstest]
    fn enrich_stamps_handle_from_harness_kind() {
        let mut n = Normalizer::new(HarnessKind::Pi);
        let msg = n.enrich(&session(), &scripted_message_without_id()).unwrap();
        assert_eq!(msg.session, n.handle(&session()));
        assert_eq!(msg.session.harness, HarnessKind::Pi);
    }

    #[rstest]
    fn attachment_lines_keep_an_empty_row_so_the_tree_stays_linked() {
        let m = ccode(&serde_json::json!({
            "type": "attachment", "uuid": "a1", "parentUuid": "u0", "cwd": "/x",
            "attachment": {"type": "hook_success", "stdout": "secret"},
        }));
        let msg = Normalizer::new(HarnessKind::ClaudeCode).enrich(&session(), &m).unwrap();
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
        assert!(Normalizer::new(HarnessKind::ClaudeCode).enrich(&session(), &m).is_none());
    }

    #[rstest]
    fn title_lines_keep_a_row_stamped_with_the_given_timestamp() {
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let m = ccode(&serde_json::json!({
            "type": "ai-title", "aiTitle": "Fix it", "timestamp": "2023-11-14T22:13:20Z",
        }));
        let captured = Normalizer::new(HarnessKind::ClaudeCode).capture(&session(), &m);
        let msg = captured.row.unwrap();
        assert_eq!(msg.timestamp, ts);
        assert_eq!(msg.session_title.as_deref(), Some("Fix it"));
        assert_eq!(captured.new_title.as_deref(), Some("Fix it"));
        assert!(msg.content.is_empty());
    }

    #[rstest]
    fn distinct_titles_get_distinct_source_ids() {
        let a = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "one"}));
        let b = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "two"}));
        assert_ne!(Normalizer::source_id(&session(), &a), Normalizer::source_id(&session(), &b));
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
        let msg = Normalizer::new(HarnessKind::ClaudeCode).enrich(&session(), &m).unwrap();
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

        let mut n = Normalizer::new(HarnessKind::ClaudeCode);
        let rows: Vec<Message> =
            [first, second].iter().filter_map(|m| n.enrich(&session(), m)).collect();
        assert!(rows[0].usage.is_some());
        assert!(rows[1].usage.is_none());
        assert_eq!(rows[1].content.len(), 1);
    }

    /// Codex has no turn id: a bookkeeping line must not make the accounting line that follows
    /// look like a repeat, and every accounting line keeps its own usage.
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

        let mut n = Normalizer::new(HarnessKind::Codex);
        assert!(n.enrich(&session(), &started).is_none());
        let first = n.enrich(&session(), &usage("r1", 5)).unwrap();
        let second = n.enrich(&session(), &usage("r2", 7)).unwrap();
        assert_eq!(first.usage.and_then(|u| u.output), Some(5));
        assert_eq!(second.usage.and_then(|u| u.output), Some(7));
    }
}
