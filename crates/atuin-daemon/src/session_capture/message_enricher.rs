use std::collections::HashMap;

use atuin_client::ai_session::{
    HarnessKind, HarnessSession, Message, NativeSessionId, Session, SourceId,
};
use atuin_common::harnesstools::session::{AnyMessage, Message as HarnessMessage, SessionId};
use atuin_domain::record::RecordId;
use time::OffsetDateTime;

/// Prefix of a content-addressed [`SourceId`], for lines the harness gave no id.
pub(super) const SYNTHETIC: &str = "syn-";

/// Builds the canonical [`Message`] rows for one harness's lines, carrying the per-session
/// bookkeeping (title, timestamps, parent, synthetic id ordinals) that a single line cannot
/// resolve on its own. The live capture loop and the backfill importer both drive it, so a line
/// re-read later resolves to the same row (see [`MessageEnricher::capture`]).
///
/// Rows carry usage exactly as the harness reported it; counting each model call once is the
/// sidecar's job, since only it sees every session a call was copied into.
pub struct MessageEnricher {
    harness: HarnessKind,
    sessions: HashMap<String, SessionState>,
}

impl MessageEnricher {
    pub fn new(harness: HarnessKind) -> Self {
        Self {
            harness,
            sessions: HashMap::new(),
        }
    }

    pub fn handle(&self, session: &SessionId) -> HarnessSession {
        HarnessSession {
            harness: self.harness,
            session: NativeSessionId::from(session.to_string()),
        }
    }

    /// Forget a session's bookkeeping: its transcript is about to be read from the beginning.
    pub fn restart(&mut self, session: &SessionId) {
        self.sessions.remove(session.as_ref());
    }

    /// Warm a session's bookkeeping from what the sidecar holds, for a transcript about to be
    /// read from past its start: its row (title, parent), its newest stored message (the
    /// timestamp an untimed line takes) and the source ids of its stored content-addressed rows
    /// (so identical id-less lines keep counting where the earlier read left off).
    pub fn resume(
        &mut self,
        session: &SessionId,
        row: Option<&Session>,
        last: Option<&Message>,
        synthetic: &[SourceId],
    ) {
        let mut occurrences = HashMap::new();
        for id in synthetic {
            if let Some((hash, ordinal)) = parse_synthetic(id) {
                let next = occurrences.entry(hash).or_insert(0);
                *next = (*next).max(ordinal + 1);
            }
        }
        self.sessions.insert(session.to_string(), SessionState {
            title: row.and_then(|r| r.title.clone()),
            last_ts: last.map(|m| m.timestamp),
            parent: row.and_then(|r| r.parent.as_ref().map(|p| p.session.clone())),
            occurrences,
            untimed: Vec::new(),
        });
    }

    /// Observe a line and return the rows now ready: none for a bookkeeping line, and none yet
    /// for a row with no timestamp to take (see [`SessionState::untimed`]), which comes out
    /// with the next timestamped line.
    pub fn capture(&mut self, session: &SessionId, m: &AnyMessage) -> Vec<Message> {
        let handle = self.handle(session);
        let state = self.sessions.entry(session.to_string()).or_default();
        if let Some(ts) = m.timestamp() {
            state.last_ts = Some(ts);
        }
        // ponytail: newest title wins; a hand-set title is not ranked above a later generated one.
        if let Some(title) = m.title() {
            state.title = Some(title);
        }
        if let Some(parent) = m.parent_session().filter(|p| p != session) {
            state.parent = Some(NativeSessionId::from(parent.to_string()));
        }

        let row = build(handle, session, m, state);
        let Some(ts) = state.last_ts else {
            state.untimed.extend(row);
            return Vec::new();
        };
        let mut ready = std::mem::take(&mut state.untimed);
        for untimed in &mut ready {
            untimed.timestamp = ts;
        }
        ready.extend(row);
        ready
    }

    /// The transcript has ended: rows still waiting for a timestamp take capture time, as a
    /// session with no timestamped line at all has nothing better.
    pub fn finish(&mut self, session: &SessionId) -> Vec<Message> {
        let Some(state) = self.sessions.get_mut(session.as_ref()) else {
            return Vec::new();
        };
        let now = OffsetDateTime::now_utc();
        let mut rows = std::mem::take(&mut state.untimed);
        for row in &mut rows {
            row.timestamp = now;
        }
        rows
    }

    /// The identity of the first occurrence of a line. Harnesses that carry a native message id
    /// use it directly; otherwise the line is content-addressed over every field a row takes
    /// from it, so re-capturing it (daemon restart, transcript re-read) resolves to the same id
    /// instead of minting a fresh one. Later lines identical to it in every field are told apart
    /// by their ordinal, which only [`Self::capture`] tracks.
    #[cfg(test)]
    pub fn source_id(session: &SessionId, m: &AnyMessage) -> SourceId {
        match m.id() {
            Some(id) => SourceId::from(String::from(id)),
            None => synthetic_id(content_hash(session, m), 0),
        }
    }
}

/// `None` for a line that carries nothing worth a row: harness bookkeeping (Claude Code mode
/// switches, Codex `item_completed` twins) with no content, usage, stop reason, title or session
/// context (cwd, branch, model). Session context is kept because the session row is projected
/// from rows alone (Codex `session_meta`, the Pi header). A line with its own id is a node of the
/// transcript tree and keeps an empty row (Claude Code attachments), so no kept line's parent
/// link dangles.
///
/// A row with no timestamp of its own takes the session's last one; with none yet, the epoch
/// stands in until [`MessageEnricher::capture`] knows better.
fn build(
    handle: HarnessSession,
    session: &SessionId,
    m: &AnyMessage,
    state: &mut SessionState,
) -> Option<Message> {
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

    let source_id = match m.id() {
        Some(id) => SourceId::from(String::from(id)),
        None => {
            let hash = content_hash(session, m);
            let ordinal = state.occurrences.entry(hash).or_insert(0);
            let id = synthetic_id(hash, *ordinal);
            *ordinal += 1;
            id
        }
    };
    let harness = handle.harness;
    Some(
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(handle)
            .source_id(source_id)
            .parent(state.parent.clone().map(|session| HarnessSession { harness, session }))
            .parent_source_id(m.parent_id().map(|id| SourceId::from(String::from(id))))
            .turn_id(m.turn_id())
            .timestamp(state.last_ts.unwrap_or(OffsetDateTime::UNIX_EPOCH))
            .role(m.role())
            .content(content)
            .model(model)
            .usage(usage)
            .stop_reason(stop_reason)
            .cwd(cwd)
            .git_branch(git_branch)
            .session_title(state.title.clone())
            .build(),
    )
}

/// A hash of everything a row takes from an id-less line, so lines that differ in any of it
/// (two usage records written in one millisecond) never share an id.
fn content_hash(session: &SessionId, m: &AnyMessage) -> u64 {
    let canonical = serde_json::json!([
        session.as_ref(),
        m.timestamp().map(OffsetDateTime::unix_timestamp_nanos).map(|ns| ns.to_string()),
        m.role(),
        m.content(),
        m.title(),
        m.model(),
        m.usage(),
        m.stop_reason(),
        m.cwd(),
        m.git_branch(),
        m.parent_id(),
        m.parent_session(),
        m.turn_id(),
    ]);
    xxhash_rust::xxh3::xxh3_64(canonical.to_string().as_bytes())
}

/// The first occurrence keeps the bare hash; the nth identical line after it is `-n`.
fn synthetic_id(hash: u64, ordinal: u32) -> SourceId {
    SourceId::from(match ordinal {
        0 => format!("{SYNTHETIC}{hash:016x}"),
        n => format!("{SYNTHETIC}{hash:016x}-{n}"),
    })
}

fn parse_synthetic(id: &SourceId) -> Option<(u64, u32)> {
    let rest = id.as_ref().strip_prefix(SYNTHETIC)?;
    let (hash, ordinal) = match rest.split_once('-') {
        Some((hash, n)) => (hash, n.parse().ok()?),
        None => (rest, 0),
    };
    Some((u64::from_str_radix(hash, 16).ok()?, ordinal))
}

/// What the capture pipeline carries from one line to the next, per native session id.
///
/// Warm for any session whose lines have all streamed past this run. A session resumed past its
/// start (the engine's checkpoints) is warmed from the sidecar first, via
/// [`MessageEnricher::resume`], or titles and synthetic ids would silently break after a restart.
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
    /// How many rows each content hash has produced, so identical id-less lines (the same
    /// prompt twice in one millisecond) get distinct, re-read-stable ids.
    occurrences: HashMap<u64, u32>,
    /// Rows from before the first timestamped line, waiting to take its timestamp: the session
    /// started no earlier, and capture time would make an old session look new.
    untimed: Vec<Message>,
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

    /// Every row a whole transcript of `lines` produces.
    fn rows(n: &mut MessageEnricher, session: &SessionId, lines: &[AnyMessage]) -> Vec<Message> {
        let mut out: Vec<Message> = lines.iter().flat_map(|m| n.capture(session, m)).collect();
        out.extend(n.finish(session));
        out
    }

    /// The row a one-line transcript produces.
    fn one(harness: HarnessKind, m: &AnyMessage) -> Option<Message> {
        rows(&mut MessageEnricher::new(harness), &session(), std::slice::from_ref(m)).pop()
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
        assert!(String::from(a).starts_with(SYNTHETIC));
    }

    #[rstest]
    #[case(0)]
    #[case(1)]
    #[case(u32::MAX)]
    fn synthetic_ids_parse_back(#[case] ordinal: u32) {
        let id = synthetic_id(0x00ab_cdef_0123_4567, ordinal);
        assert_eq!(parse_synthetic(&id), Some((0x00ab_cdef_0123_4567, ordinal)));
    }

    #[rstest]
    fn enrich_stamps_handle_from_harness_kind() {
        let n = MessageEnricher::new(HarnessKind::Pi);
        let msg = one(HarnessKind::Pi, &scripted_message_without_id()).unwrap();
        assert_eq!(msg.session, n.handle(&session()));
        assert_eq!(msg.session.harness, HarnessKind::Pi);
    }

    #[rstest]
    fn attachment_lines_keep_an_empty_row_so_the_tree_stays_linked() {
        let m = ccode(&serde_json::json!({
            "type": "attachment", "uuid": "a1", "parentUuid": "u0", "cwd": "/x",
            "attachment": {"type": "hook_success", "stdout": "secret"},
        }));
        let msg = one(HarnessKind::ClaudeCode, &m).unwrap();
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
        assert!(one(HarnessKind::ClaudeCode, &ccode(&raw)).is_none());
    }

    #[rstest]
    fn title_lines_keep_a_row_stamped_with_the_given_timestamp() {
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let m = ccode(&serde_json::json!({
            "type": "ai-title", "aiTitle": "Fix it", "timestamp": "2023-11-14T22:13:20Z",
        }));
        let msg = one(HarnessKind::ClaudeCode, &m).unwrap();
        assert_eq!(msg.timestamp, ts);
        assert_eq!(msg.session_title.as_deref(), Some("Fix it"));
        assert!(msg.content.is_empty());
    }

    /// Rows before the first timestamped line wait for it and take its timestamp, rather than
    /// capture time; rows after an untimed line take the last timestamp seen.
    #[rstest]
    fn untimed_rows_take_the_nearest_known_timestamp() {
        let at = |ts: &str| {
            ccode(&serde_json::json!({
                "type": "user", "uuid": ts, "timestamp": ts,
                "message": {"role": "user", "content": "hi"},
            }))
        };
        let title = |t: &str| ccode(&serde_json::json!({"type": "ai-title", "aiTitle": t}));
        let mut n = MessageEnricher::new(HarnessKind::ClaudeCode);
        assert!(n.capture(&session(), &title("first")).is_empty(), "waits for a timestamp");
        let flushed = n.capture(&session(), &at("2020-01-01T00:00:00Z"));
        let later = n.capture(&session(), &title("second"));
        let expected = OffsetDateTime::from_unix_timestamp(1_577_836_800).unwrap();
        assert_eq!(flushed.iter().map(|m| m.timestamp).collect::<Vec<_>>(), vec![
            expected, expected
        ]);
        assert_eq!(flushed[0].session_title.as_deref(), Some("first"));
        assert_eq!(later[0].timestamp, expected);
        assert!(n.finish(&session()).is_empty());
    }

    /// A line carrying only session context (Codex `session_meta`, the Pi header) is a row by
    /// rule: the session row is projected from rows alone.
    #[rstest]
    fn session_context_lines_keep_a_row() {
        let m = codex(&serde_json::json!({
            "type": "session_meta", "payload": {"cwd": "/work/atuin"},
        }));
        let msg = one(HarnessKind::Codex, &m).unwrap();
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
        let lines: Vec<AnyMessage> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| match harness {
                HarnessKind::ClaudeCode => ccode(&serde_json::from_str(l).unwrap()),
                HarnessKind::Codex => codex(&serde_json::from_str(l).unwrap()),
                HarnessKind::Pi => AnyMessage::from(serde_json::from_str::<PiMessage>(l).unwrap()),
                _ => unreachable!(),
            })
            .collect();
        let rows = rows(&mut MessageEnricher::new(harness), &sid, &lines);
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
        let msg = one(HarnessKind::ClaudeCode, &m).unwrap();
        assert_eq!(msg.parent_source_id, Some(SourceId::from("u1".to_owned())));
        assert_eq!(
            msg.parent.map(|p| p.session),
            parent.map(|p| NativeSessionId::from(p.to_owned()))
        );
        assert_eq!(msg.turn_id.as_deref(), Some("msg_01"));
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, vec![Content::Text("hi".into())]);
    }

    /// Every row of one model call keeps the usage it reported: the sidecar, not the enricher,
    /// counts the call once.
    #[rstest]
    fn split_rows_of_a_call_keep_their_reported_usage() {
        let line = |uuid: &str, output: u64| {
            ccode(&serde_json::json!({
                "type": "assistant", "uuid": uuid, "sessionId": "s1",
                "timestamp": "2026-09-18T10:00:00Z",
                "message": {"role": "assistant", "id": "msg_01",
                    "content": [{"type": "text", "text": uuid}],
                    "usage": {"input_tokens": 1, "output_tokens": output}},
            }))
        };
        let mut n = MessageEnricher::new(HarnessKind::ClaudeCode);
        let rows = rows(&mut n, &session(), &[line("u1", 2), line("u2", 20)]);
        let outputs: Vec<_> = rows.iter().map(|m| m.usage.and_then(|u| u.output)).collect();
        assert_eq!(outputs, vec![Some(2), Some(20)]);
        assert!(rows.iter().all(|m| m.turn_id.as_deref() == Some("msg_01")));
    }

    /// Resumed state stands in for the lines a resumed session did not replay: an untimed line
    /// takes the last stored timestamp, rows carry the stored title and parent, and an id-less
    /// line identical to stored ones continues their ordinals.
    #[rstest]
    fn resumed_state_carries_over_a_restart() {
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
            .usage(atuin_common::harnesstools::session::Usage::default())
            .build();
        let last = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(n.handle(&session()))
            .source_id(SourceId::from("u0".to_owned()))
            .timestamp(ts)
            .role(Role::Assistant)
            .content(vec![])
            .build();
        let untimed = ccode(&serde_json::json!({"type": "ai-title", "aiTitle": "Renamed"}));
        let first = MessageEnricher::source_id(&session(), &untimed);
        let (hash, _) = parse_synthetic(&first).unwrap();
        n.resume(&session(), Some(&row), Some(&last), &[first, synthetic_id(hash, 1)]);

        let msg = n.capture(&session(), &untimed).pop().unwrap();
        assert_eq!(msg.timestamp, ts);
        assert_eq!(msg.source_id, synthetic_id(hash, 2));
        assert_eq!(msg.session_title.as_deref(), Some("Renamed"));
        assert_eq!(msg.parent, Some(parent));

        // A transcript read again from the start counts from the first occurrence again.
        n.restart(&session());
        let again = rows(&mut n, &session(), &[untimed]);
        assert_eq!(again[0].source_id, synthetic_id(hash, 0));
        assert_eq!(again[0].parent, None);
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
        let rows = rows(&mut n, &session(), &[started, usage("r1", 5), usage("r2", 7)]);
        let outputs: Vec<_> = rows.iter().map(|m| m.usage.and_then(|u| u.output)).collect();
        assert_eq!(outputs, vec![Some(5), Some(7)]);
    }

    /// An id-less Pi assistant line (a v1 session) reporting `output` tokens.
    fn idless_pi_usage(output: u64) -> AnyMessage {
        AnyMessage::from(
            serde_json::from_value::<PiMessage>(serde_json::json!({
                "type": "message", "timestamp": "2026-09-18T10:00:00.123Z",
                "message": {"role": "assistant", "content": [],
                    "usage": {"input": 10, "output": output, "cacheRead": 0, "cacheWrite": 0}},
            }))
            .unwrap(),
        )
    }

    /// Two model calls recorded in the same millisecond by id-less lines with no content are
    /// told apart by their usage alone: it is part of the hash.
    #[rstest]
    fn idless_lines_differing_only_in_usage_keep_distinct_source_ids() {
        let a = MessageEnricher::source_id(&session(), &idless_pi_usage(5));
        let b = MessageEnricher::source_id(&session(), &idless_pi_usage(7));
        assert!(a.as_ref().starts_with(SYNTHETIC));
        assert_ne!(a, b, "distinct model calls collapse to one row");
    }

    /// Lines identical in every field get consecutive ordinals, so both are rows.
    #[rstest]
    fn identical_idless_lines_get_distinct_source_ids() {
        let line = codex(&serde_json::json!({
            "type": "response_item", "timestamp": "2026-09-18T10:00:00.123Z",
            "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "continue"}]},
        }));
        let mut n = MessageEnricher::new(HarnessKind::Codex);
        let rows = rows(&mut n, &session(), &[line.clone(), line.clone()]);
        let first = MessageEnricher::source_id(&session(), &line);
        let (hash, _) = parse_synthetic(&first).unwrap();
        let ids: Vec<_> = rows.into_iter().map(|m| m.source_id).collect();
        assert_eq!(ids, vec![first, synthetic_id(hash, 1)]);
    }
}

/// What the Claude Code and Pi parsers resolve for a row, as seen through the enricher: fork
/// parents, legacy titles and stable ids.
#[cfg(test)]
mod parser_contract {
    use atuin_common::harnesstools::ccode::session::CcodeMessage;
    use atuin_common::harnesstools::pi::session::PiMessage;
    use rstest::rstest;

    use super::*;

    fn ccode(raw: &serde_json::Value) -> AnyMessage {
        AnyMessage::Ccode(serde_json::from_str::<CcodeMessage>(&raw.to_string()).unwrap())
    }

    /// The row a one-line transcript produces.
    fn capture(harness: HarnessKind, session: &SessionId, m: &AnyMessage) -> Option<Message> {
        let mut n = MessageEnricher::new(harness);
        let mut rows = n.capture(session, m);
        rows.extend(n.finish(session));
        rows.pop()
    }

    fn assistant(uuid: &str, msg_id: &str, output: u64, extra: &serde_json::Value) -> AnyMessage {
        let mut raw = serde_json::json!({
            "type": "assistant", "uuid": uuid, "sessionId": "s1", "requestId": format!("req_{msg_id}"),
            "timestamp": "2026-09-23T22:41:00Z",
            "message": {"role": "assistant", "id": msg_id, "model": "claude-opus-5-5",
                "content": [{"type": "text", "text": uuid}],
                "usage": {"input_tokens": 2, "output_tokens": output,
                    "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 10}},
        });
        raw.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        ccode(&raw)
    }

    #[rstest]
    fn forked_session_rows_link_to_their_origin() {
        let copy = SessionId::from("new".to_owned());
        let row = capture(
            HarnessKind::ClaudeCode,
            &copy,
            &assistant(
                "u1",
                "msg_A",
                100,
                &serde_json::json!({"sessionId": "new",
                    "forkedFrom": {"sessionId": "orig", "messageUuid": "u1"}}),
            ),
        )
        .unwrap();
        assert_eq!(row.parent.map(|p| p.session), Some(NativeSessionId::from("orig".to_owned())));
    }

    /// A legacy `summary` line is a row carrying the session title.
    #[rstest]
    fn summary_line_sets_the_session_title() {
        let m = ccode(&serde_json::json!({
            "type": "summary", "summary": "Fix the flaky sync test", "leafUuid": "u9",
        }));
        let row = capture(HarnessKind::ClaudeCode, &session(), &m);
        assert_eq!(row.and_then(|r| r.session_title).as_deref(), Some("Fix the flaky sync test"));
    }

    fn session() -> SessionId {
        SessionId::from("s1".to_owned())
    }
    // ---- Pi ----

    fn pi(raw: &serde_json::Value) -> AnyMessage {
        AnyMessage::Pi(serde_json::from_str::<PiMessage>(&raw.to_string()).unwrap())
    }

    const PARENT: &str = "0199aaaa-0000-7000-8000-000000000001";
    const FORK: &str = "0199bbbb-0000-7000-8000-000000000002";

    fn fork_header() -> AnyMessage {
        // pi-mono session-manager.ts:1851 (`forkFrom`) / :1674 (`createBranchedSession`) write the
        // parent's resolved file path here.
        pi(&serde_json::json!({
            "type": "session", "version": 3, "id": FORK, "timestamp": "2026-09-18T11:00:00Z",
            "cwd": "/w",
            "parentSession": format!(
                "/home/u/.pi/agent/sessions/--w--/2026-09-18T10-00-00-000Z_{PARENT}.jsonl"),
        }))
    }

    /// A fork's rows point at the parent session row, which is keyed by the parent's id.
    #[rstest]
    fn pi_fork_parent_is_the_parent_session_id() {
        let fork = SessionId::from(FORK.to_owned());
        let msg = capture(HarnessKind::Pi, &fork, &fork_header()).unwrap();
        assert_eq!(msg.parent.map(|p| p.session), Some(NativeSessionId::from(PARENT.to_owned())));
    }

    /// pi migrates v1/v2 files in place when it opens them (session-manager.ts:1092-1093
    /// `_rewriteFile`), assigning fresh ids to lines that had none (session-manager.ts:287
    /// `migrateV1ToV2`). A line captured before the rewrite got a content-addressed id; the same
    /// line after it gets a new native id; both must name one row, or a re-read would capture
    /// it (and its usage) twice.
    #[rstest]
    fn pi_migrated_line_keeps_its_source_id() {
        let before = serde_json::json!({
            "type": "message", "timestamp": "2026-01-01T00:00:00Z",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}],
                "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0}},
        });
        let mut after = before.clone();
        after["id"] = serde_json::json!("1a2b3c4d");
        after["parentId"] = serde_json::Value::Null;
        let s = session();
        assert_eq!(
            MessageEnricher::source_id(&s, &pi(&before)),
            MessageEnricher::source_id(&s, &pi(&after))
        );
    }
}
