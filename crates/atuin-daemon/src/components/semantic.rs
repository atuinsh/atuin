//! Semantic command capture component.
//!
//! This is a prototype in-memory store for completed command captures emitted
//! by atuin-pty-proxy. It keeps recent captures per Atuin session and indexes
//! them by history ID for AI tool lookup.

use std::collections::{HashMap, VecDeque};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

use atuin_client::history::{History, HistoryId};
use eyre::Result;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, instrument};

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
    semantic::{
        CommandCapture, CommandOutputReply, CommandOutputRequest, OutputLine, RecordCommandsReply,
        semantic_server::{Semantic as SemanticSvc, SemanticServer},
    },
};

const MAX_SESSIONS: usize = 20;
const MAX_COMMANDS_PER_SESSION: usize = 128;
const MAX_BYTES_PER_SESSION: usize = 32 * 1024 * 1024;
const MAX_PENDING_HISTORIES: usize = 128;

/// Stores completed command captures and associates them with history events.
pub struct SemanticComponent {
    inner: Arc<SemanticComponentInner>,
}

struct SemanticComponentInner {
    state: Mutex<SemanticState>,
}

#[derive(Default)]
struct SemanticState {
    sessions: HashMap<SessionId, SessionCaptures>,
    session_lru: VecDeque<SessionId>,
    history_index: HashMap<HistoryId, CaptureRef>,
    pending_histories: VecDeque<History>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CaptureId(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureRef {
    session_id: SessionId,
    capture_id: CaptureId,
}

#[derive(Default)]
struct SessionCaptures {
    next_id: u64,
    records: VecDeque<StoredCapture>,
    output_bytes: usize,
}

struct StoredCapture {
    id: CaptureId,
    history_id: HistoryId,
    output_bytes: usize,
    record: SemanticCommandRecord,
}

struct EvictedCapture {
    history_id: HistoryId,
    capture_id: CaptureId,
}

#[derive(Debug, Clone)]
struct SemanticCommandRecord {
    capture: CommandCapture,
    history: Option<History>,
}

impl SemanticComponent {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SemanticComponentInner {
                state: Mutex::new(SemanticState::default()),
            }),
        }
    }

    pub fn grpc_service(&self) -> SemanticServer<SemanticGrpcService> {
        SemanticServer::new(SemanticGrpcService {
            inner: self.inner.clone(),
        })
    }
}

impl Default for SemanticComponent {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl Component for SemanticComponent {
    fn name(&self) -> &'static str {
        "semantic"
    }

    async fn start(&mut self, _handle: DaemonHandle) -> Result<()> {
        tracing::info!("semantic component started");
        Ok(())
    }

    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
        if let DaemonEvent::HistoryEnded(history) = event {
            self.inner.record_history(history.clone()).await;
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        let state = self.inner.state.lock().await;
        tracing::info!(
            sessions = state.sessions.len(),
            records = state.record_count(),
            indexed_histories = state.history_index.len(),
            pending_histories = state.pending_histories.len(),
            "semantic component stopped"
        );
        Ok(())
    }
}

impl SemanticComponentInner {
    async fn record_capture(&self, capture: CommandCapture) -> bool {
        let mut state = self.state.lock().await;
        state.record_capture(capture)
    }

    async fn record_history(&self, history: History) {
        let mut state = self.state.lock().await;
        state.record_history(history);
    }

    async fn command_output(&self, request: &CommandOutputRequest) -> CommandOutputReply {
        let mut state = self.state.lock().await;
        state.command_output(request)
    }
}

impl SemanticState {
    fn record_capture(&mut self, mut capture: CommandCapture) -> bool {
        let Some(history_id) = history_id_from_str(capture.history_id.as_deref()) else {
            tracing::debug!(
                command_bytes = capture.command.len(),
                prompt_bytes = capture.prompt.len(),
                output_bytes = capture.output.len(),
                output_truncated = capture.output_truncated,
                "dropping semantic command capture without history id"
            );
            return false;
        };

        let history = take_pending_history(&mut self.pending_histories, &history_id);
        let Some(session_id) = capture
            .session_id
            .as_deref()
            .and_then(|session_id| SessionId::try_from(session_id).ok())
            .or_else(|| {
                history
                    .as_ref()
                    .and_then(|history| SessionId::try_from(history.session.as_str()).ok())
            })
        else {
            tracing::debug!(
                history_id = %history_id,
                command_bytes = capture.command.len(),
                prompt_bytes = capture.prompt.len(),
                output_bytes = capture.output.len(),
                output_truncated = capture.output_truncated,
                "dropping semantic command capture without session id"
            );
            return false;
        };

        capture.history_id = Some(history_id.to_string());
        capture.session_id = Some(session_id.to_string());
        if capture.output_observed_bytes == 0 {
            capture.output_observed_bytes = capture.output.len() as u64;
        }

        let record = SemanticCommandRecord { capture, history };
        log_record(&record, "recorded semantic command capture");
        self.push_record(session_id, history_id, record);
        true
    }

    fn record_history(&mut self, history: History) {
        let history_id = history.id.clone();

        if let Some(capture_ref) = self.history_index.get(&history_id).cloned() {
            if let Some(stored) = self.stored_capture_mut(&capture_ref) {
                stored.record.history = Some(history);
                log_record(
                    &stored.record,
                    "associated semantic command capture with history",
                );
                return;
            }

            self.history_index.remove(&history_id);
        }

        tracing::debug!(
            id = %history.id,
            command_bytes = history.command.len(),
            "history ended before semantic capture arrived"
        );
        push_pending_history(&mut self.pending_histories, history);
    }

    fn command_output(&mut self, request: &CommandOutputRequest) -> CommandOutputReply {
        let Some(history_id) = history_id_from_str(Some(&request.history_id)) else {
            return command_output_not_found();
        };
        let Some(capture_ref) = self.history_index.get(&history_id).cloned() else {
            return command_output_not_found();
        };

        let Some(reply) = self.command_output_for_ref(&capture_ref, &request.ranges) else {
            self.history_index.remove(&history_id);
            return command_output_not_found();
        };

        self.touch_session(&capture_ref.session_id);
        reply
    }

    fn command_output_for_ref(
        &self,
        capture_ref: &CaptureRef,
        ranges: &[crate::semantic::OutputRange],
    ) -> Option<CommandOutputReply> {
        let stored = self
            .sessions
            .get(&capture_ref.session_id)?
            .stored_capture(capture_ref.capture_id)?;
        let output = &stored.record.capture.output;
        let output_observed_bytes = stored
            .record
            .capture
            .output_observed_bytes
            .max(output.len() as u64);

        Some(CommandOutputReply {
            found: true,
            output: String::new(),
            total_bytes: output.len() as u64,
            total_lines: output.lines().count() as u64,
            lines: select_output_ranges(output, ranges),
            output_truncated: stored.record.capture.output_truncated,
            output_observed_bytes,
        })
    }

    fn push_record(
        &mut self,
        session_id: SessionId,
        history_id: HistoryId,
        record: SemanticCommandRecord,
    ) {
        self.touch_session(&session_id);

        let (capture_id, evicted) = {
            let session = self.sessions.entry(session_id.clone()).or_default();
            session.push(history_id.clone(), record)
        };

        let capture_ref = CaptureRef {
            session_id: session_id.clone(),
            capture_id,
        };
        self.history_index.insert(history_id, capture_ref);

        for evicted in evicted {
            self.remove_history_index_if_matches(
                &session_id,
                &evicted.history_id,
                evicted.capture_id,
            );
        }

        self.expire_lru_sessions();
    }

    fn touch_session(&mut self, session_id: &SessionId) {
        if let Some(index) = self.session_lru.iter().position(|id| id == session_id) {
            self.session_lru.remove(index);
        }
        self.session_lru.push_back(session_id.clone());
    }

    fn expire_lru_sessions(&mut self) {
        while self.session_lru.len() > MAX_SESSIONS {
            let Some(session_id) = self.session_lru.pop_front() else {
                break;
            };
            let Some(session) = self.sessions.remove(&session_id) else {
                continue;
            };

            for stored in session.records {
                self.remove_history_index_if_matches(&session_id, &stored.history_id, stored.id);
            }
        }
    }

    fn remove_history_index_if_matches(
        &mut self,
        session_id: &SessionId,
        history_id: &HistoryId,
        capture_id: CaptureId,
    ) {
        if self
            .history_index
            .get(history_id)
            .is_some_and(|capture_ref| {
                &capture_ref.session_id == session_id && capture_ref.capture_id == capture_id
            })
        {
            self.history_index.remove(history_id);
        }
    }

    fn stored_capture_mut(&mut self, capture_ref: &CaptureRef) -> Option<&mut StoredCapture> {
        self.sessions
            .get_mut(&capture_ref.session_id)?
            .stored_capture_mut(capture_ref.capture_id)
    }

    fn record_count(&self) -> usize {
        self.sessions
            .values()
            .map(|session| session.records.len())
            .sum()
    }
}

impl SessionCaptures {
    fn push(
        &mut self,
        history_id: HistoryId,
        record: SemanticCommandRecord,
    ) -> (CaptureId, Vec<EvictedCapture>) {
        self.push_with_limits(
            history_id,
            record,
            MAX_COMMANDS_PER_SESSION,
            MAX_BYTES_PER_SESSION,
        )
    }

    fn push_with_limits(
        &mut self,
        history_id: HistoryId,
        record: SemanticCommandRecord,
        max_commands: usize,
        max_output_bytes: usize,
    ) -> (CaptureId, Vec<EvictedCapture>) {
        let capture_id = CaptureId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let output_bytes = record.capture.output.len();
        self.output_bytes = self.output_bytes.saturating_add(output_bytes);
        self.records.push_back(StoredCapture {
            id: capture_id,
            history_id,
            output_bytes,
            record,
        });

        (
            capture_id,
            self.evict_to_limits(max_commands, max_output_bytes),
        )
    }

    fn evict_to_limits(
        &mut self,
        max_commands: usize,
        max_output_bytes: usize,
    ) -> Vec<EvictedCapture> {
        let mut evicted = Vec::new();
        while self.records.len() > max_commands || self.output_bytes > max_output_bytes {
            let Some(record) = self.records.pop_front() else {
                break;
            };
            self.output_bytes = self.output_bytes.saturating_sub(record.output_bytes);
            evicted.push(EvictedCapture {
                history_id: record.history_id,
                capture_id: record.id,
            });
        }
        evicted
    }

    fn stored_capture(&self, capture_id: CaptureId) -> Option<&StoredCapture> {
        self.records.iter().find(|record| record.id == capture_id)
    }

    fn stored_capture_mut(&mut self, capture_id: CaptureId) -> Option<&mut StoredCapture> {
        self.records
            .iter_mut()
            .find(|record| record.id == capture_id)
    }
}

impl TryFrom<&str> for SessionId {
    type Error = ();

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        let value = value.trim();
        if value.is_empty() {
            return Err(());
        }

        Ok(Self(value.to_string()))
    }
}

impl TryFrom<String> for SessionId {
    type Error = ();

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for SessionId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub struct SemanticGrpcService {
    inner: Arc<SemanticComponentInner>,
}

#[tonic::async_trait]
impl SemanticSvc for SemanticGrpcService {
    #[instrument(skip_all, level = Level::INFO)]
    async fn record_commands(
        &self,
        request: Request<Streaming<CommandCapture>>,
    ) -> Result<Response<RecordCommandsReply>, Status> {
        let mut stream = request.into_inner();
        let mut accepted = 0_u64;

        while let Some(capture) = stream.message().await? {
            if self.inner.record_capture(capture).await {
                accepted += 1;
            }
        }

        Ok(Response::new(RecordCommandsReply { accepted }))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn command_output(
        &self,
        request: Request<CommandOutputRequest>,
    ) -> Result<Response<CommandOutputReply>, Status> {
        let request = request.into_inner();
        if request.history_id.trim().is_empty() {
            return Err(Status::invalid_argument("history_id is required"));
        }

        Ok(Response::new(self.inner.command_output(&request).await))
    }
}

fn history_id_from_str(value: Option<&str>) -> Option<HistoryId> {
    let value = value?.trim();
    (!value.is_empty()).then(|| HistoryId(value.to_string()))
}

fn take_pending_history(
    histories: &mut VecDeque<History>,
    history_id: &HistoryId,
) -> Option<History> {
    let index = histories
        .iter()
        .position(|history| &history.id == history_id)?;
    histories.remove(index)
}

fn push_pending_history(histories: &mut VecDeque<History>, history: History) {
    if let Some(index) = histories
        .iter()
        .position(|pending| pending.id == history.id)
    {
        histories.remove(index);
    }

    histories.push_back(history);
    trim_front(histories, MAX_PENDING_HISTORIES);
}

fn trim_front<T>(records: &mut VecDeque<T>, max_len: usize) {
    while records.len() > max_len {
        records.pop_front();
    }
}

fn command_output_not_found() -> CommandOutputReply {
    CommandOutputReply {
        found: false,
        output: String::new(),
        total_bytes: 0,
        total_lines: 0,
        lines: Vec::new(),
        output_truncated: false,
        output_observed_bytes: 0,
    }
}

fn select_output_ranges(output: &str, ranges: &[crate::semantic::OutputRange]) -> Vec<OutputLine> {
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let ranges = if ranges.is_empty() {
        vec![crate::semantic::OutputRange { start: 0, end: 999 }]
    } else {
        ranges.to_vec()
    };

    let mut ranges = ranges
        .into_iter()
        .filter_map(|range| normalize_line_range(range.start, range.end, lines.len()))
        .collect::<Vec<_>>();
    ranges.sort_unstable_by_key(|(start, _)| *start);

    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some((_, merged_end)) if start <= merged_end.saturating_add(1) => {
                *merged_end = (*merged_end).max(end);
            }
            _ => merged.push((start, end)),
        }
    }

    merged
        .into_iter()
        .flat_map(|(start, end)| {
            lines[start..=end]
                .iter()
                .enumerate()
                .map(move |(offset, line)| OutputLine {
                    line_number: (start + offset + 1) as u64,
                    content: (*line).to_string(),
                })
        })
        .collect()
}

fn normalize_line_range(start: i64, end: i64, line_count: usize) -> Option<(usize, usize)> {
    let line_count = i64::try_from(line_count).ok()?;
    let start = if start < 0 { line_count + start } else { start };
    let end = if end < 0 { line_count + end } else { end };

    if end < 0 || start >= line_count {
        return None;
    }

    let start = start.max(0);
    let end = end.min(line_count - 1);

    (start <= end).then_some((start as usize, end as usize))
}

fn log_record(record: &SemanticCommandRecord, message: &'static str) {
    let history_id = record.capture.history_id.as_deref().unwrap_or("<missing>");
    let associated_history_id = record
        .history
        .as_ref()
        .map(|history| history.id.to_string());
    let exit = record.history.as_ref().map(|history| history.exit);
    let duration = record.history.as_ref().map(|history| history.duration);
    let author = record
        .history
        .as_ref()
        .map(|history| history.author.as_str());
    let session_id = record.capture.session_id.as_deref();

    tracing::debug!(
        history_id = %history_id,
        associated_history_id = ?associated_history_id,
        session_id = ?session_id,
        command_bytes = record.capture.command.len(),
        prompt_bytes = record.capture.prompt.len(),
        output_bytes = record.capture.output.len(),
        output_truncated = record.capture.output_truncated,
        output_observed_bytes = record.capture.output_observed_bytes,
        capture_exit_code = ?record.capture.exit_code,
        history_exit = ?exit,
        duration = ?duration,
        author = ?author,
        "{message}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn history(id: &str, session: &str, command: &str) -> History {
        History {
            id: HistoryId(id.to_string()),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            duration: 0,
            exit: 0,
            command: command.to_string(),
            cwd: String::new(),
            session: session.to_string(),
            hostname: String::new(),
            author: String::new(),
            intent: None,
            deleted_at: None,
        }
    }

    fn capture(history_id: Option<&str>, session_id: Option<&str>, output: &str) -> CommandCapture {
        CommandCapture {
            prompt: String::new(),
            command: String::new(),
            output: output.to_string(),
            exit_code: None,
            history_id: history_id.map(str::to_string),
            session_id: session_id.map(str::to_string),
            output_truncated: false,
            output_observed_bytes: output.len() as u64,
        }
    }

    fn command_output(state: &mut SemanticState, history_id: &str) -> CommandOutputReply {
        state.command_output(&CommandOutputRequest {
            history_id: history_id.to_string(),
            ranges: Vec::new(),
        })
    }

    fn output_line(line_number: u64, content: &str) -> OutputLine {
        OutputLine {
            line_number,
            content: content.to_string(),
        }
    }

    #[test]
    fn drops_capture_without_history_id() {
        let mut state = SemanticState::default();

        assert!(!state.record_capture(capture(None, Some("session-1"), "output")));
        assert!(!command_output(&mut state, "id-1").found);
        assert_eq!(state.record_count(), 0);
    }

    #[test]
    fn stores_capture_by_session_and_history_id() {
        let mut state = SemanticState::default();

        assert!(state.record_capture(capture(Some("id-1"), Some("session-1"), "output")));

        let reply = command_output(&mut state, "id-1");
        assert!(reply.found);
        assert_eq!(reply.total_bytes, 6);
        assert_eq!(reply.output_observed_bytes, 6);
        assert_eq!(reply.lines, vec![output_line(1, "output")]);
    }

    #[test]
    fn uses_pending_history_session_when_capture_session_is_missing() {
        let mut state = SemanticState::default();

        state.record_history(history("id-1", "session-from-history", "cargo test"));
        assert!(state.record_capture(capture(Some("id-1"), None, "output")));

        assert!(
            state
                .sessions
                .contains_key(&SessionId("session-from-history".to_string()))
        );
        assert!(command_output(&mut state, "id-1").found);
    }

    #[test]
    fn associates_history_by_id_after_capture_arrives() {
        let mut state = SemanticState::default();

        assert!(state.record_capture(capture(Some("id-1"), Some("session-1"), "output")));
        state.record_history(history("id-1", "session-1", "different command"));

        let capture_ref = state
            .history_index
            .get(&HistoryId("id-1".to_string()))
            .unwrap();
        let stored = state
            .sessions
            .get(&capture_ref.session_id)
            .unwrap()
            .stored_capture(capture_ref.capture_id)
            .unwrap();
        assert!(stored.record.history.is_some());
    }

    #[test]
    fn evicts_oldest_command_when_session_ring_is_full() {
        let mut state = SemanticState::default();

        for index in 0..=MAX_COMMANDS_PER_SESSION {
            assert!(state.record_capture(capture(
                Some(&format!("id-{index}")),
                Some("session-1"),
                "output",
            )));
        }

        assert!(!command_output(&mut state, "id-0").found);
        assert!(command_output(&mut state, &format!("id-{MAX_COMMANDS_PER_SESSION}")).found);
        assert_eq!(state.record_count(), MAX_COMMANDS_PER_SESSION);
    }

    #[test]
    fn evicts_oldest_session_after_lru_limit() {
        let mut state = SemanticState::default();

        for index in 0..MAX_SESSIONS {
            assert!(state.record_capture(capture(
                Some(&format!("id-{index}")),
                Some(&format!("session-{index}")),
                "output",
            )));
        }
        assert!(command_output(&mut state, "id-0").found);

        assert!(state.record_capture(capture(Some("new-id"), Some("new-session"), "output",)));

        assert!(command_output(&mut state, "id-0").found);
        assert!(!command_output(&mut state, "id-1").found);
        assert!(command_output(&mut state, "new-id").found);
        assert_eq!(state.sessions.len(), MAX_SESSIONS);
    }

    #[test]
    fn evicts_by_session_byte_limit() {
        let mut session = SessionCaptures::default();
        let first_output = "x".repeat(10);
        let second_output = "y";
        let (_, evicted_first) = session.push_with_limits(
            HistoryId("first".to_string()),
            SemanticCommandRecord {
                capture: capture(Some("first"), Some("session-1"), &first_output),
                history: None,
            },
            MAX_COMMANDS_PER_SESSION,
            10,
        );
        assert!(evicted_first.is_empty());

        let (_, evicted_second) = session.push_with_limits(
            HistoryId("second".to_string()),
            SemanticCommandRecord {
                capture: capture(Some("second"), Some("session-1"), second_output),
                history: None,
            },
            MAX_COMMANDS_PER_SESSION,
            10,
        );

        assert_eq!(evicted_second.len(), 1);
        assert_eq!(evicted_second[0].history_id, HistoryId("first".to_string()));
        assert_eq!(session.records.len(), 1);
        assert_eq!(session.output_bytes, 1);
    }

    #[test]
    fn command_output_reports_truncation_metadata() {
        let mut state = SemanticState::default();
        let mut capture = capture(Some("id-1"), Some("session-1"), "partial");
        capture.output_truncated = true;
        capture.output_observed_bytes = 1024;

        assert!(state.record_capture(capture));

        let reply = command_output(&mut state, "id-1");
        assert!(reply.output_truncated);
        assert_eq!(reply.total_bytes, 7);
        assert_eq!(reply.output_observed_bytes, 1024);
    }

    #[test]
    fn output_ranges_are_line_based_inclusive_and_support_negative_offsets() {
        let output = "zero\none\ntwo\nthree\nfour";
        let ranges = vec![
            crate::semantic::OutputRange { start: 1, end: 2 },
            crate::semantic::OutputRange { start: -2, end: -1 },
        ];

        assert_eq!(
            select_output_ranges(output, &ranges),
            vec![
                output_line(2, "one"),
                output_line(3, "two"),
                output_line(4, "three"),
                output_line(5, "four"),
            ]
        );
    }

    #[test]
    fn output_ranges_merge_overlaps_and_adjacent_ranges() {
        let output = (0..100)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let ranges = vec![
            crate::semantic::OutputRange { start: 0, end: 100 },
            crate::semantic::OutputRange {
                start: -100,
                end: -1,
            },
        ];

        let selected = select_output_ranges(&output, &ranges);

        assert_eq!(selected.len(), 100);
        assert_eq!(selected.first(), Some(&output_line(1, "line 0")));
        assert_eq!(selected.last(), Some(&output_line(100, "line 99")));
    }

    #[test]
    fn output_ranges_can_leave_gaps_for_client_formatting() {
        let output = "zero\none\ntwo\nthree\nfour";
        let ranges = vec![
            crate::semantic::OutputRange { start: 0, end: 1 },
            crate::semantic::OutputRange { start: 4, end: 4 },
        ];

        assert_eq!(
            select_output_ranges(output, &ranges),
            vec![
                output_line(1, "zero"),
                output_line(2, "one"),
                output_line(5, "four"),
            ]
        );
    }

    #[test]
    fn empty_output_ranges_default_to_first_thousand_lines() {
        let output = (0..1001)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");

        let selected = select_output_ranges(&output, &[]);

        assert_eq!(selected.len(), 1000);
        assert_eq!(selected.first(), Some(&output_line(1, "line 0")));
        assert_eq!(selected.last(), Some(&output_line(1000, "line 999")));
    }

    #[test]
    fn output_ranges_skip_ranges_fully_outside_output() {
        let output = "zero\none\ntwo";
        let ranges = vec![
            crate::semantic::OutputRange { start: 10, end: 20 },
            crate::semantic::OutputRange {
                start: -20,
                end: -10,
            },
        ];

        assert_eq!(select_output_ranges(output, &ranges), Vec::new());
    }
}
