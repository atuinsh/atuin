use std::collections::HashMap;
use std::collections::hash_map::Entry;

use atuin_client::ai_session::{
    HarnessKind, HarnessSession, Message, NativeSessionId, Session, SourceId,
};
use atuin_common::harnesstools::session::{AnyMessage, Message as HarnessMessage, SessionId};
use atuin_domain::record::RecordId;
use time::OffsetDateTime;

/// Builds the canonical [`Message`] rows for one harness's lines, carrying the per-session
/// bookkeeping (title, timestamps, parent, usage dedupe) that a single line cannot resolve on its
/// own. The live capture loop and the backfill importer both drive it, so a line re-read later
/// resolves to the same row (see [`MessageEnricher::source_id`]).
pub struct MessageEnricher {
    harness: HarnessKind,
    state: Bookkeeping,
}

impl MessageEnricher {
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

    /// Whether no line of `session` has been observed this run.
    pub fn is_new(&self, session: &SessionId) -> bool {
        !self.state.sessions.contains_key(session.as_ref())
    }

    /// Warm a session's bookkeeping from what the sidecar holds, for a transcript resumed past
    /// its start. A session already observed this run is left as it is.
    pub fn seed(&mut self, session: &SessionId, row: Option<&Session>, last: Option<&Message>) {
        let Entry::Vacant(slot) = self.state.sessions.entry(session.to_string()) else {
            return;
        };
        slot.insert(SessionState {
            title: row.and_then(|r| r.title.clone()),
            last_ts: last.map(|m| m.timestamp),
            parent: row.and_then(|r| r.parent.as_ref().map(|p| p.session.clone())),
            last_turn: last.and_then(|m| m.turn_id.clone()),
        });
    }

    /// Observe a line, build its row, and drop repeated usage in one step. `None` for a
    /// bookkeeping line worth no row.
    pub fn capture(&mut self, session: &SessionId, m: &AnyMessage) -> Option<Message> {
        let seen = self.observe(session, m);
        let mut row = self.build(session, m, &seen);
        if seen.repeat
            && let Some(msg) = &mut row
        {
            msg.usage = None;
        }
        row
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
        if let Some(title) = m.title() {
            state.title = Some(title);
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
            title: state.title.clone(),
            parent: state.parent.clone(),
            repeat,
        }
    }

    /// `None` for a line that carries nothing worth a row: harness bookkeeping (Claude Code mode
    /// switches, Codex `item_completed` twins) with no content, usage, stop reason, title or
    /// session context (cwd, branch, model). Session context is kept because the session row is
    /// projected from rows alone (Codex `session_meta`, the Pi header). A line with its own id is
    /// a node of the transcript tree and keeps an empty row (Claude Code attachments), so no kept
    /// line's parent link dangles.
    fn build(&self, session: &SessionId, m: &AnyMessage, seen: &Observed) -> Option<Message> {
        let content = m.content();
        let usage = m.usage();
        let stop_reason = m.stop_reason();
        let model = m.model();
        let cwd = m.cwd();
        let git_branch = m.git_branch();
        if m.id().is_none()
            && content.is_empty()
            && usage.is_none()
            && stop_reason.is_none()
            && model.is_none()
            && cwd.is_none()
            && git_branch.is_none()
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
                .revision(m.revision())
                .timestamp(seen.timestamp)
                .role(m.role())
                .content(content)
                .model(model)
                .usage(usage)
                .stop_reason(stop_reason)
                .cwd(cwd)
                .git_branch(git_branch)
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
///
/// Warm for any session whose lines have all streamed past this run. A session resumed past its
/// start (the engine's checkpoints) is seeded from the sidecar first, via
/// [`MessageEnricher::seed`], or titles and usage dedupe would silently break after a restart.
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

/// What [`MessageEnricher::observe`] resolved for one line.
struct Observed {
    timestamp: OffsetDateTime,
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
        assert_eq!(String::from(MessageEnricher::source_id(&session(), &m)), "msg-1".to_string());
    }

    #[rstest]
    fn synthetic_source_id_is_stable_and_prefixed() {
        let m = scripted_message_without_id();
        let a = MessageEnricher::source_id(&session(), &m);
        let b = MessageEnricher::source_id(&session(), &m);
        assert_eq!(a, b);
        assert!(String::from(a).starts_with("syn-"));
    }

    #[rstest]
    fn enrich_stamps_handle_from_harness_kind() {
        let mut n = MessageEnricher::new(HarnessKind::Pi);
        let msg = n.capture(&session(), &scripted_message_without_id()).unwrap();
        assert_eq!(msg.session, n.handle(&session()));
        assert_eq!(msg.session.harness, HarnessKind::Pi);
    }

    #[rstest]
    fn attachment_lines_keep_an_empty_row_so_the_tree_stays_linked() {
        let m = ccode(&serde_json::json!({
            "type": "attachment", "uuid": "a1", "parentUuid": "u0", "cwd": "/x",
            "attachment": {"type": "hook_success", "stdout": "secret"},
        }));
        let msg = MessageEnricher::new(HarnessKind::ClaudeCode).capture(&session(), &m).unwrap();
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
        assert!(MessageEnricher::new(HarnessKind::ClaudeCode).capture(&session(), &m).is_none());
    }

    #[rstest]
    fn title_lines_keep_a_row_stamped_with_the_given_timestamp() {
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let m = ccode(&serde_json::json!({
            "type": "ai-title", "aiTitle": "Fix it", "timestamp": "2023-11-14T22:13:20Z",
        }));
        let msg = MessageEnricher::new(HarnessKind::ClaudeCode).capture(&session(), &m).unwrap();
        assert_eq!(msg.timestamp, ts);
        assert_eq!(msg.session_title.as_deref(), Some("Fix it"));
        assert!(msg.content.is_empty());
    }

    /// A line carrying only session context (Codex `session_meta`, the Pi header) is a row by
    /// rule: the session row is projected from rows alone.
    #[rstest]
    fn session_context_lines_keep_a_row() {
        let m = codex(&serde_json::json!({
            "type": "session_meta", "payload": {"cwd": "/work/atuin"},
        }));
        let msg = MessageEnricher::new(HarnessKind::Codex).capture(&session(), &m).unwrap();
        assert_eq!(msg.cwd.as_deref(), Some(std::path::Path::new("/work/atuin")));
        assert!(msg.content.is_empty());
    }

    /// The session row is a projection of rows: for every fixture, folding the captured rows the
    /// way the sessions upsert does yields what the (since deleted) per-file `meta()` read
    /// returned. Values are what `meta()` produced on these redacted fixtures before deletion.
    ///
    /// Known difference: `meta()` never set a model for Claude Code (rows carry it from each
    /// assistant line), and Pi's model came from the header's `modelId` where rows carry the
    /// assistant line's `message.model`; on these fixtures both redact to the same string.
    #[rstest]
    #[case::ccode1(
        include_str!("../../../atuin-common/tests/fixtures/ccode/session1.jsonl"),
        HarnessKind::ClaudeCode, "fe23275d-ec6d-fbd0-dff0-69a857f5a8a4",
        Some("<path>"), Some("<redacted>"), Some("<redacted>"), Some("<redacted>")
    )]
    #[case::ccode2(
        include_str!("../../../atuin-common/tests/fixtures/ccode/session2.jsonl"),
        HarnessKind::ClaudeCode, "b6edc00c-95da-8ccc-5d2f-a8202b668926",
        Some("<path>"), Some("<redacted>"), Some("<redacted>"), Some("<redacted>")
    )]
    #[case::ccode3(
        include_str!("../../../atuin-common/tests/fixtures/ccode/session3.jsonl"),
        HarnessKind::ClaudeCode, "ad2b77ac-8b01-ecc3-a4fb-608c770200bf",
        Some("<path>"), Some("<redacted>"), Some("<redacted>"), Some("<redacted>")
    )]
    #[case::codex1(
        include_str!("../../../atuin-common/tests/fixtures/codex/session1.jsonl"),
        HarnessKind::Codex, "fbf768ea-a58c-ab8f-a29f-6a640df5ba7b",
        Some("<path>"), None, Some("<redacted>"), None
    )]
    #[case::pi1(
        include_str!("../../../atuin-common/tests/fixtures/pi/session1.jsonl"),
        HarnessKind::Pi, "372bddb6-5d16-1553-fe79-3ffc84a47b20",
        Some("<path>"), None, Some("<redacted>"), None
    )]
    #[case::pi2(
        include_str!("../../../atuin-common/tests/fixtures/pi/session2.jsonl"),
        HarnessKind::Pi, "7b496138-2a3f-3cb1-279a-4cdb9915fa98",
        Some("<path>"), None, Some("<redacted>"), None
    )]
    fn rows_project_the_session_that_meta_used_to_read(
        #[case] jsonl: &str,
        #[case] harness: HarnessKind,
        #[case] session_id: &str,
        #[case] cwd: Option<&str>,
        #[case] git_branch: Option<&str>,
        #[case] model: Option<&str>,
        #[case] title: Option<&str>,
    ) {
        let sid = SessionId::from(session_id.to_owned());
        let mut n = MessageEnricher::new(harness);
        let lines = jsonl.lines().filter(|l| !l.trim().is_empty());
        let rows: Vec<Message> = lines
            .map(|l| match harness {
                HarnessKind::ClaudeCode => ccode(&serde_json::from_str(l).unwrap()),
                HarnessKind::Codex => codex(&serde_json::from_str(l).unwrap()),
                HarnessKind::Pi => AnyMessage::from(serde_json::from_str::<PiMessage>(l).unwrap()),
                _ => unreachable!(),
            })
            .filter_map(|m| n.capture(&sid, &m))
            .collect();
        assert!(!rows.is_empty());

        // Latest non-null wins, as the sessions upsert's COALESCE(excluded.x, sessions.x) does.
        let last = |pick: fn(&Message) -> Option<String>| rows.iter().rev().find_map(pick);
        assert_eq!(
            last(|m| m.cwd.as_ref().map(|p| p.to_string_lossy().into_owned())).as_deref(),
            cwd
        );
        assert_eq!(last(|m| m.git_branch.clone()).as_deref(), git_branch);
        assert_eq!(last(|m| m.model.clone()).as_deref(), model);
        assert_eq!(last(|m| m.session_title.clone()).as_deref(), title);
        assert!(rows.iter().all(|m| m.parent.is_none()), "no fixture is a fork or subagent");
    }

    #[rstest]
    fn distinct_titles_get_distinct_source_ids() {
        let a = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "one"}));
        let b = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "two"}));
        assert_ne!(
            MessageEnricher::source_id(&session(), &a),
            MessageEnricher::source_id(&session(), &b)
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
        let msg = MessageEnricher::new(HarnessKind::ClaudeCode).capture(&session(), &m).unwrap();
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

        let mut n = MessageEnricher::new(HarnessKind::ClaudeCode);
        let rows: Vec<Message> =
            [first, second].iter().filter_map(|m| n.capture(&session(), m)).collect();
        assert!(rows[0].usage.is_some());
        assert!(rows[1].usage.is_none());
        assert_eq!(rows[1].content.len(), 1);
    }

    /// Seeded state stands in for the lines a resumed session did not replay: the next row of
    /// the same model call is a repeat, an id-less line takes the last stored timestamp, and
    /// rows carry the stored title and parent.
    #[rstest]
    fn seeded_state_carries_over_a_restart() {
        use atuin_common::harnesstools::session::Usage;

        let mut n = MessageEnricher::new(HarnessKind::ClaudeCode);
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let parent = HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("p".to_owned()),
        };
        let row = Session::builder()
            .handle(n.handle(&session()))
            .parent(Some(parent.clone()))
            .title(Some("Seeded".to_owned()))
            .started_at(ts)
            .updated_at(ts)
            .usage(Usage::default())
            .build();
        let last = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(n.handle(&session()))
            .source_id(SourceId::from("u0".to_owned()))
            .timestamp(ts)
            .role(Role::Assistant)
            .content(vec![])
            .turn_id(Some("msg_01".to_owned()))
            .build();
        n.seed(&session(), Some(&row), Some(&last));

        let untimed = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "Renamed"}));
        let msg = n.capture(&session(), &untimed).unwrap();
        assert_eq!(msg.timestamp, ts);

        let next = ccode(&serde_json::json!({
            "type": "assistant", "uuid": "u1", "sessionId": "s1",
            "message": {"role": "assistant", "id": "msg_01",
                "content": [{"type": "text", "text": "more"}],
                "usage": {"input_tokens": 1, "output_tokens": 2}},
        }));
        let msg = n.capture(&session(), &next).unwrap();
        assert!(msg.usage.is_none(), "same model call as the last stored row");
        assert_eq!(msg.session_title.as_deref(), Some("Renamed"));
        assert_eq!(msg.parent, Some(parent));

        // Seeding never clobbers a session already observed this run.
        n.seed(&session(), None, None);
        let again = n.capture(&session(), &untimed).unwrap();
        assert_eq!(again.session_title.as_deref(), Some("Renamed"));
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

        let mut n = MessageEnricher::new(HarnessKind::Codex);
        assert!(n.capture(&session(), &started).is_none());
        let first = n.capture(&session(), &usage("r1", 5)).unwrap();
        let second = n.capture(&session(), &usage("r2", 7)).unwrap();
        assert_eq!(first.usage.and_then(|u| u.output), Some(5));
        assert_eq!(second.usage.and_then(|u| u.output), Some(7));
    }
}
