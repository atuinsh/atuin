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
        CommandCapture, RecordCommandsReply,
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
            command = %history.command,
            id = %history.id,
            "history ended before semantic capture arrived"
        );
        state.pending_histories.push_back(history);
        trim_front(&mut state.pending_histories, MAX_PENDING_HISTORIES);
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
        command = %record.capture.command,
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
}
