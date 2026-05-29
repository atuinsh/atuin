//! Semantic command capture component.
//!
//! This is a prototype in-memory store for completed command captures emitted
//! by atuin-pty-proxy. It associates captures with regular history lifecycle events
//! when possible, then logs the joined record.

use std::collections::VecDeque;
use std::sync::Arc;

use atuin_client::history::History;
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

const MAX_RECORDS: usize = 512;
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
    records: VecDeque<SemanticCommandRecord>,
    pending_histories: VecDeque<History>,
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
            records = state.records.len(),
            pending_histories = state.pending_histories.len(),
            "semantic component stopped"
        );
        Ok(())
    }
}

impl SemanticComponentInner {
    async fn record_capture(&self, capture: CommandCapture) {
        let mut state = self.state.lock().await;
        let history = take_matching_history(&mut state.pending_histories, &capture);
        let record = SemanticCommandRecord { capture, history };

        log_record(&record, "recorded semantic command capture");
        state.records.push_back(record);
        trim_front(&mut state.records, MAX_RECORDS);
    }

    async fn record_history(&self, history: History) {
        let mut state = self.state.lock().await;

        if let Some(record) = state.records.iter_mut().rev().find(|record| {
            record.history.is_none() && history_matches_capture(&history, &record.capture)
        }) {
            record.history = Some(history);
            log_record(record, "associated semantic command capture with history");
            return;
        }

        tracing::debug!(
            id = %history.id,
            command_bytes = history.command.len(),
            "history ended before semantic capture arrived"
        );
        state.pending_histories.push_back(history);
        trim_front(&mut state.pending_histories, MAX_PENDING_HISTORIES);
    }

    async fn command_output(&self, request: &CommandOutputRequest) -> CommandOutputReply {
        let state = self.state.lock().await;
        let Some(record) = state
            .records
            .iter()
            .rev()
            .find(|record| record_has_history_id(record, &request.history_id))
        else {
            return CommandOutputReply {
                found: false,
                output: String::new(),
                total_bytes: 0,
                total_lines: 0,
                lines: Vec::new(),
            };
        };

        let output = &record.capture.output;
        CommandOutputReply {
            found: true,
            output: String::new(),
            total_bytes: output.len() as u64,
            total_lines: output.lines().count() as u64,
            lines: select_output_ranges(output, &request.ranges),
        }
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
            accepted += 1;
            self.inner.record_capture(capture).await;
        }

        Ok(Response::new(RecordCommandsReply { accepted }))
    }

    #[instrument(skip_all, level = Level::INFO)]
    async fn command_output(
        &self,
        request: Request<CommandOutputRequest>,
    ) -> Result<Response<CommandOutputReply>, Status> {
        let request = request.into_inner();
        if request.history_id.is_empty() {
            return Err(Status::invalid_argument("history_id is required"));
        }

        Ok(Response::new(self.inner.command_output(&request).await))
    }
}

fn take_matching_history(
    histories: &mut VecDeque<History>,
    capture: &CommandCapture,
) -> Option<History> {
    let index = histories
        .iter()
        .position(|history| history_matches_capture(history, capture))?;
    histories.remove(index)
}

fn history_matches_capture(history: &History, capture: &CommandCapture) -> bool {
    if let Some(history_id) = capture.history_id.as_deref() {
        return history.id.0 == history_id;
    }

    commands_match(&capture.command, &history.command)
}

fn record_has_history_id(record: &SemanticCommandRecord, history_id: &str) -> bool {
    record
        .capture
        .history_id
        .as_deref()
        .is_some_and(|capture_history_id| capture_history_id == history_id)
        || record
            .history
            .as_ref()
            .is_some_and(|history| history.id.0 == history_id)
}

fn commands_match(left: &str, right: &str) -> bool {
    !left.is_empty() && normalize_command(left) == normalize_command(right)
}

fn normalize_command(command: &str) -> &str {
    command.trim_matches(|c| c == '\r' || c == '\n' || c == ' ')
}

fn trim_front<T>(records: &mut VecDeque<T>, max_len: usize) {
    while records.len() > max_len {
        records.pop_front();
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
    let history_id = record
        .history
        .as_ref()
        .map(|history| history.id.to_string())
        .unwrap_or_else(|| "<pending>".to_string());
    let exit = record.history.as_ref().map(|history| history.exit);
    let duration = record.history.as_ref().map(|history| history.duration);
    let author = record
        .history
        .as_ref()
        .map(|history| history.author.as_str());
    let capture_history_id = record.capture.history_id.as_deref();
    let session_id = record.capture.session_id.as_deref();

    tracing::info!(
        history_id = %history_id,
        capture_history_id = ?capture_history_id,
        session_id = ?session_id,
        command_bytes = record.capture.command.len(),
        prompt_bytes = record.capture.prompt.len(),
        output_bytes = record.capture.output.len(),
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
    use atuin_client::history::HistoryId;
    use time::OffsetDateTime;

    fn history(id: &str, command: &str) -> History {
        History {
            id: HistoryId(id.to_string()),
            timestamp: OffsetDateTime::UNIX_EPOCH,
            duration: 0,
            exit: 0,
            command: command.to_string(),
            cwd: String::new(),
            session: String::new(),
            hostname: String::new(),
            author: String::new(),
            intent: None,
            deleted_at: None,
        }
    }

    fn capture(history_id: Option<&str>, command: &str) -> CommandCapture {
        CommandCapture {
            prompt: String::new(),
            command: command.to_string(),
            output: String::new(),
            exit_code: None,
            history_id: history_id.map(str::to_string),
            session_id: None,
        }
    }

    #[test]
    fn history_id_match_takes_precedence_over_command_text() {
        let history = history("id-1", "cargo test");
        let capture = capture(Some("id-1"), "different command");

        assert!(history_matches_capture(&history, &capture));
    }

    #[test]
    fn command_match_is_fallback_only_when_capture_has_no_history_id() {
        let history = history("id-1", "cargo test");

        assert!(history_matches_capture(
            &history,
            &capture(None, "cargo test\n")
        ));
        assert!(!history_matches_capture(
            &history,
            &capture(Some("id-2"), "cargo test")
        ));
        assert!(!history_matches_capture(&history, &capture(None, "")));
    }

    fn output_line(line_number: u64, content: &str) -> OutputLine {
        OutputLine {
            line_number,
            content: content.to_string(),
        }
    }

    #[test]
    fn record_history_id_match_includes_associated_history() {
        let record = SemanticCommandRecord {
            capture: capture(None, "cargo test"),
            history: Some(history("id-1", "cargo test")),
        };

        assert!(record_has_history_id(&record, "id-1"));
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
