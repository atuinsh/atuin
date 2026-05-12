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
        let history = take_matching_history(&mut state.pending_histories, &capture.command);
        let record = SemanticCommandRecord { capture, history };

        log_record(&record, "recorded semantic command capture");
        state.records.push_back(record);
        trim_front(&mut state.records, MAX_RECORDS);
    }

    async fn record_history(&self, history: History) {
        let mut state = self.state.lock().await;

        if let Some(record) = state.records.iter_mut().rev().find(|record| {
            record.history.is_none() && commands_match(&record.capture.command, &history.command)
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

fn take_matching_history(histories: &mut VecDeque<History>, command: &str) -> Option<History> {
    let index = histories
        .iter()
        .position(|history| commands_match(command, &history.command))?;
    histories.remove(index)
}

fn commands_match(left: &str, right: &str) -> bool {
    normalize_command(left) == normalize_command(right)
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

    tracing::info!(
        history_id = %history_id,
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
