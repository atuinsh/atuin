//! Driver loop for the agent FSM.
//!
//! Receives events from the channel, calls `fsm.handle()`, syncs ViewState
//! to the Handle, and executes effects (spawning async tasks for IO).
//!
//! The driver runs on a blocking thread (`spawn_blocking`) so it can call
//! `blocking_recv()` on the Handle and `block_on()` for async persistence.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use eye_declare::Handle;

use crate::context::{AppContext, ClientContext};
use crate::edit_permissions::EditPermissionCache;
use crate::file_tracker::FileReadTracker;
use crate::fsm::effects::{Effect, ExitAction, PermissionTarget};
use crate::fsm::events::{Event, PermissionChoice, PermissionResponse};
use crate::fsm::tools::ToolPreviewData;
use crate::fsm::{AgentFsm, AgentState};
use crate::permissions::resolver::PermissionResolver;
use crate::permissions::writer;
use crate::session::SessionManager;
use crate::stream::ChatRequest;
use crate::tools::ClientToolCall;
use crate::tui::events::{AiTuiEvent, PermissionResult};
use crate::tui::state::ConversationEvent;

// ============================================================================
// Driver event — the unified channel type
// ============================================================================

/// Events processed by the driver loop.
///
/// Components emit `Tui` variants via the channel. Spawned async tasks
/// (stream, tool execution) emit `Fsm` variants directly.
#[derive(Debug)]
pub(crate) enum DriverEvent {
    /// Event from a TUI component (key press, input change, etc.)
    Tui(AiTuiEvent),
    /// Internal FSM event (from spawned stream/tool tasks)
    Fsm(Event),
}

// ============================================================================
// IO context (driver-owned, not visible to FSM)
// ============================================================================

pub(crate) struct IoContext {
    pub app_ctx: AppContext,
    pub client_ctx: ClientContext,
    pub session_mgr: SessionManager,
    pub file_tracker: FileReadTracker,
    pub edit_permissions: EditPermissionCache,
    pub snapshot_store: Option<crate::snapshots::SnapshotStore>,
}

// ============================================================================
// ViewState (Handle payload for the render thread)
// ============================================================================

/// State pushed to the Handle for the view/render thread.
/// Synced from the FSM after each transition.
#[derive(Debug)]
pub(crate) struct ViewState {
    // ─── From FSM ───────────────────────────────────────────────
    pub agent_state: AgentState,
    pub visible_events: Vec<ConversationEvent>,
    pub all_events: Vec<ConversationEvent>,
    pub session_id: Option<String>,
    pub tools: crate::fsm::tools::ToolManager,
    pub current_response: String,

    // ─── Session metadata (set once) ────────────────────────────
    pub is_resumed: bool,
    pub last_event_time: Option<chrono::DateTime<chrono::Utc>>,
    pub in_git_project: bool,

    // ─── View-only ──────────────────────────────────────────────
    pub archived_events: Vec<ConversationEvent>,

    // ─── Ephemeral interaction state ────────────────────────────
    pub is_input_blank: bool,
    pub slash_command_input: Option<String>,
    pub slash_command_search_results: Vec<crate::tui::slash::SlashCommandSearchResult>,
    pub exit_action: Option<ExitAction>,
    pub slash_registry: crate::tui::slash::SlashCommandRegistry,
}

impl ViewState {
    pub fn is_exiting(&self) -> bool {
        self.exit_action.is_some()
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.agent_state, AgentState::Turn { .. })
    }

    pub fn has_confirmation(&self) -> bool {
        matches!(
            self.agent_state,
            AgentState::Idle {
                confirmation: Some(_)
            }
        )
    }

    pub fn is_input_active(&self) -> bool {
        matches!(self.agent_state, AgentState::Idle { .. }) && !self.has_confirmation()
    }

    /// Whether any command has been suggested in the current invocation.
    pub fn has_command(&self) -> bool {
        self.visible_events.iter().any(|e| {
            if let ConversationEvent::ToolCall { name, input, .. } = e {
                name == "suggest_command" && input.get("command").and_then(|v| v.as_str()).is_some()
            } else {
                false
            }
        })
    }

    pub fn footer_text(&self) -> &'static str {
        match &self.agent_state {
            AgentState::Idle { confirmation: None } => {
                if self.has_command() && self.is_input_blank {
                    "[Enter] Execute suggested command  [Tab] Insert Command"
                } else {
                    "[Enter] Send  [Shift+Enter] New line  [Esc] Exit"
                }
            }
            AgentState::Idle {
                confirmation: Some(_),
            } => "[Enter] Confirm dangerous command  [Esc] Cancel",
            AgentState::Turn { .. } => "[Esc] Cancel",
            AgentState::Error(_) => "[Enter]/[r] Retry  [Esc] Exit",
        }
    }
}

// ============================================================================
// Main driver loop
// ============================================================================

struct DriverContext<'a> {
    fsm: &'a mut AgentFsm,
    io: &'a mut IoContext,
    handle: &'a Handle<ViewState>,
    tx: &'a mpsc::Sender<DriverEvent>,
    exiting: &'a Arc<AtomicBool>,
    stream_cancel_tx: &'a mut Option<tokio::sync::watch::Sender<()>>,
    tool_abort_txs: &'a mut std::collections::HashMap<String, tokio::sync::oneshot::Sender<()>>,
}

/// Main driver loop. Processes events, transitions FSM, syncs view, executes effects.
///
/// Runs on a blocking thread. Returns when the event channel closes or exit is requested.
/// The Handle already contains the initial ViewState (set by Application::builder).
pub(crate) fn run_driver(
    mut fsm: AgentFsm,
    mut io: IoContext,
    handle: Handle<ViewState>,
    rx: mpsc::Receiver<DriverEvent>,
    tx: mpsc::Sender<DriverEvent>,
    exiting: Arc<AtomicBool>,
    in_git_project: bool,
) {
    // Dropping the sender cancels the stream (receiver sees Err on changed()).
    let mut stream_cancel_tx: Option<tokio::sync::watch::Sender<()>> = None;
    // Per-tool interrupt senders for shell commands.
    let mut tool_abort_txs: std::collections::HashMap<String, tokio::sync::oneshot::Sender<()>> =
        std::collections::HashMap::new();

    while let Ok(driver_event) = rx.recv() {
        // Log and translate DriverEvent to FSM Event (or handle directly)
        let fsm_event = match driver_event {
            DriverEvent::Fsm(event) => {
                tracing::trace!(?event, state = ?fsm.state, "FSM event");
                Some(event)
            }
            DriverEvent::Tui(tui_event) => {
                tracing::trace!(?tui_event, state = ?fsm.state, "TUI event");
                translate_tui_event(tui_event, &handle)
            }
        };

        if let Some(event) = fsm_event {
            // Feed event to FSM
            let effects = fsm.handle(event);
            tracing::trace!(?effects, state = ?fsm.state, "FSM transition");

            // Sync ViewState to Handle (FSM owns all state now)
            sync_view_state(&handle, &fsm, in_git_project);

            // Execute effects (only persist when FSM says to)
            for effect in &effects {
                if matches!(effect, Effect::Persist) {
                    persist(&fsm, &mut io);
                }

                let ctx = DriverContext {
                    fsm: &mut fsm,
                    io: &mut io,
                    handle: &handle,
                    tx: &tx,
                    exiting: &exiting,
                    stream_cancel_tx: &mut stream_cancel_tx,
                    tool_abort_txs: &mut tool_abort_txs,
                };

                execute_effect(effect, ctx);
            }

            // Final sync after effects — ensures the render thread sees
            // the absolute final state even if effects modified anything.
            if !effects.is_empty() {
                sync_view_state(&handle, &fsm, in_git_project);
            }
        } else {
            // Event was handled directly (e.g. InputUpdated) — just sync
            sync_view_state(&handle, &fsm, in_git_project);
        }

        if exiting.load(Ordering::Acquire) {
            break;
        }
        tracing::trace!(state = ?fsm.state, "driver loop iteration complete, waiting for next event");
    }
}

// ============================================================================
// TUI event translation
// ============================================================================

/// Translate a TUI event into an FSM event.
/// Returns None for events handled directly (e.g. InputUpdated).
fn translate_tui_event(event: AiTuiEvent, handle: &Handle<ViewState>) -> Option<Event> {
    match event {
        AiTuiEvent::SubmitInput(input) => {
            // Clear slash state and reset is_input_blank (the InputBox clears
            // its text on submit but doesn't fire InputUpdated for the clear).
            handle.update(|vs| {
                vs.slash_command_input = None;
                vs.slash_command_search_results.clear();
                vs.is_input_blank = true;
            });

            let input = input.trim().to_string();
            if input.is_empty() {
                Some(Event::ExecuteCommand)
            } else if input == "/new" {
                Some(Event::NewSession)
            } else if input.starts_with('/') {
                let content = resolve_slash_command(&input, handle);
                Some(Event::SlashCommand {
                    command: input,
                    content,
                })
            } else {
                Some(Event::UserSubmit(input))
            }
        }
        AiTuiEvent::InputUpdated(text) => {
            let is_blank = text.is_empty();
            handle.update(move |vs| {
                vs.is_input_blank = is_blank;
                if text.starts_with('/') {
                    let query = text.trim_start_matches('/').to_string();
                    let mut results = vs.slash_registry.search_fuzzy(&query);
                    results.sort_by(|a, b| {
                        b.relevance
                            .partial_cmp(&a.relevance)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    vs.slash_command_input = Some(query);
                    vs.slash_command_search_results = results;
                } else {
                    vs.slash_command_input = None;
                    vs.slash_command_search_results.clear();
                }
            });
            None
        }
        AiTuiEvent::CancelGeneration => Some(Event::Cancel),
        AiTuiEvent::ExecuteCommand => Some(Event::ExecuteCommand),
        AiTuiEvent::InsertCommand => Some(Event::InsertCommand),
        AiTuiEvent::CancelConfirmation => Some(Event::Cancel),
        AiTuiEvent::InterruptToolExecution => Some(Event::InterruptTools),
        AiTuiEvent::Retry => Some(Event::Retry),
        AiTuiEvent::Exit => Some(Event::Cancel),
        AiTuiEvent::SelectPermission(result) => {
            let tool_id = handle
                .fetch(|vs| vs.tools.awaiting_permission().map(|t| t.id.clone()))
                .blocking_recv()
                .ok()
                .flatten()?;

            let choice = match result {
                PermissionResult::Allow => PermissionChoice::Allow,
                PermissionResult::AllowFileForSession => PermissionChoice::AllowForSession,
                PermissionResult::AlwaysAllowInDir => PermissionChoice::AlwaysAllowInProject,
                PermissionResult::AlwaysAllow => PermissionChoice::AlwaysAllow,
                PermissionResult::Deny => PermissionChoice::Deny,
            };
            Some(Event::PermissionUserChoice { tool_id, choice })
        }
        AiTuiEvent::SlashCommand(cmd) => {
            let content = resolve_slash_command(&cmd, handle);
            Some(Event::SlashCommand {
                command: cmd,
                content,
            })
        }
    }
}

/// Resolve a slash command to its output content.
fn resolve_slash_command(command: &str, handle: &Handle<ViewState>) -> String {
    match command.trim() {
        "/help" => {
            let commands = handle
                .fetch(|vs| {
                    vs.slash_registry
                        .get_commands()
                        .iter()
                        .map(|cmd| format!("- `/{}` — {}", cmd.name, cmd.description))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .blocking_recv()
                .unwrap_or_default();
            include_str!("tui/content/help.md").replace("{commands}", &commands)
        }
        _ => format!("Unknown command: {command}"),
    }
}

// ============================================================================
// ViewState sync
// ============================================================================

fn sync_view_state(handle: &Handle<ViewState>, fsm: &AgentFsm, in_git_project: bool) {
    let state = fsm.state.clone();
    let safe_start = fsm.ctx.view_start_index.min(fsm.ctx.events.len());
    let mut visible_events = fsm.ctx.events[safe_start..].to_vec();
    let all_events = fsm.ctx.events.clone();
    let tools = fsm.ctx.tools.clone();
    let current_response = fsm.ctx.current_response.clone();
    let session_id = fsm.ctx.session_id.clone();
    let is_resumed = fsm.ctx.is_resumed;
    let last_event_time = fsm.ctx.last_event_time;
    let archived_events = fsm.ctx.archived_events.clone();

    // Inject streaming text as a synthetic event for live rendering.
    // The FSM commits text to events on stream end; this makes it visible during streaming.
    let trimmed = current_response.trim_start();
    if !trimmed.is_empty() {
        visible_events.push(ConversationEvent::Text {
            content: trimmed.to_string(),
        });
    }

    tracing::trace!(?state, "sync_view_state pushing to handle");
    handle.update(move |vs| {
        vs.agent_state = state;
        vs.visible_events = visible_events;
        vs.all_events = all_events;
        vs.tools = tools;
        vs.current_response = current_response;
        vs.session_id = session_id;
        vs.is_resumed = is_resumed;
        vs.last_event_time = last_event_time;
        vs.in_git_project = in_git_project;
        vs.archived_events = archived_events;
    });
}

// ============================================================================
// Effect execution
// ============================================================================

fn execute_effect(effect: &Effect, ctx: DriverContext) {
    let DriverContext {
        fsm,
        io,
        handle,
        tx,
        exiting,
        stream_cancel_tx,
        tool_abort_txs,
    } = ctx;

    match effect {
        Effect::StartStream {
            messages,
            session_id,
        } => {
            // Cancel any existing stream before starting a new one
            stream_cancel_tx.take();

            let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(());
            *stream_cancel_tx = Some(cancel_tx);

            let tx = tx.clone();
            let app = io.app_ctx.clone();
            let cc = io.client_ctx.clone();
            let request = ChatRequest::new(
                messages.clone(),
                session_id.clone(),
                &app.capabilities,
                fsm.ctx.invocation_id.clone(),
            );
            tokio::spawn(async move {
                run_stream_bridge(request, app, cc, tx, cancel_rx).await;
            });
        }

        Effect::AbortStream => {
            // Drop the sender — the bridge's cancel_rx.changed() will error,
            // breaking the stream loop and dropping the HTTP connection.
            stream_cancel_tx.take();
        }

        Effect::CheckPermission { tool_id, tool } => {
            let tool_id = tool_id.clone();
            let tool = tool.clone();
            let tx = tx.clone();
            let working_dir = tool
                .target_dir()
                .map(|p| p.to_path_buf())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));

            // Check session grants first (synchronous)
            if let Some(resolved) = tool.resolved_file_path()
                && io.edit_permissions.has_valid_grant(&resolved)
            {
                let _ = tx.send(DriverEvent::Fsm(Event::PermissionResolved {
                    tool_id,
                    response: PermissionResponse::SessionGranted,
                }));
                return;
            }

            tokio::spawn(async move {
                let response = match PermissionResolver::new(working_dir).await {
                    Ok(resolver) => match resolver.check(&tool).await {
                        Ok(crate::permissions::check::PermissionResponse::Allowed) => {
                            PermissionResponse::Allowed
                        }
                        Ok(crate::permissions::check::PermissionResponse::Denied) => {
                            PermissionResponse::Denied
                        }
                        Ok(crate::permissions::check::PermissionResponse::Ask) => {
                            PermissionResponse::Ask
                        }
                        Err(_) => PermissionResponse::Ask,
                    },
                    Err(_) => PermissionResponse::Ask,
                };
                let _ = tx.send(DriverEvent::Fsm(Event::PermissionResolved {
                    tool_id,
                    response,
                }));
            });
        }

        Effect::ExecuteTool { tool_id, tool } => {
            let tool_id = tool_id.clone();
            let tool = tool.clone();
            let tx = tx.clone();
            let db = io.app_ctx.history_db.clone();

            match &tool {
                ClientToolCall::Shell(shell_call) => {
                    let shell_call = shell_call.clone();
                    let tx_preview = tx.clone();
                    let tool_id_for_preview = tool_id.clone();

                    // Create interrupt channel and store the sender for AbortTool
                    let (interrupt_tx, interrupt_rx) = tokio::sync::oneshot::channel();
                    tool_abort_txs.insert(tool_id.clone(), interrupt_tx);

                    tokio::spawn(async move {
                        let (output_tx, mut output_rx) =
                            tokio::sync::mpsc::channel::<Vec<String>>(16);

                        let preview_id = tool_id_for_preview;
                        let tx_fwd = tx_preview;
                        tokio::spawn(async move {
                            while let Some(lines) = output_rx.recv().await {
                                let _ = tx_fwd.send(DriverEvent::Fsm(Event::ToolPreviewUpdate {
                                    tool_id: preview_id.clone(),
                                    lines,
                                    exit_code: None,
                                }));
                            }
                        });

                        let outcome = crate::tools::execute_shell_command_streaming(
                            &shell_call,
                            output_tx,
                            interrupt_rx,
                        )
                        .await;

                        let preview = if let crate::tools::ToolOutcome::Structured {
                            exit_code,
                            interrupted,
                            ..
                        } = &outcome
                        {
                            Some(ToolPreviewData::Shell {
                                lines: vec![],
                                exit_code: *exit_code,
                                interrupted: *interrupted,
                            })
                        } else {
                            None
                        };

                        let _ = tx.send(DriverEvent::Fsm(Event::ToolExecutionDone {
                            tool_id,
                            outcome,
                            preview,
                        }));
                    });
                }
                ClientToolCall::Edit(edit_call) => {
                    let resolved = edit_call.resolved_path();

                    // Capture old content for snapshot + diff preview
                    let old_content = std::fs::read(&resolved).ok();
                    if let Some(ref content) = old_content
                        && let Some(ref mut store) = io.snapshot_store
                        && let Err(e) = store.ensure_snapshot(&resolved, content)
                    {
                        tracing::warn!("Failed to snapshot before edit: {e}");
                    }

                    // Edit is fast (file read + string replace + write) — run inline
                    let (outcome, new_content) = edit_call.execute(&resolved, &io.file_tracker);

                    // Update file tracker with new content
                    if let Some(new_bytes) = &new_content
                        && let Ok(mtime) = std::fs::metadata(&resolved).and_then(|m| m.modified())
                    {
                        io.file_tracker
                            .update_after_edit(&resolved, new_bytes, mtime);
                    }

                    // Compute diff preview
                    let preview = match (&old_content, &new_content) {
                        (Some(old_bytes), Some(new_bytes)) => {
                            let old_str = String::from_utf8_lossy(old_bytes);
                            let new_str = String::from_utf8_lossy(new_bytes);
                            let diff = crate::diff::EditPreview::compute(&old_str, &new_str);
                            if diff.hunks.is_empty() {
                                None
                            } else {
                                Some(ToolPreviewData::Edit(diff))
                            }
                        }
                        _ => None,
                    };

                    let _ = tx.send(DriverEvent::Fsm(Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview,
                    }));
                }
                ClientToolCall::Write(write_call) => {
                    let resolved = write_call.resolved_path();

                    // Snapshot existing file before overwriting
                    if let Ok(content) = std::fs::read(&resolved)
                        && let Some(ref mut store) = io.snapshot_store
                        && let Err(e) = store.ensure_snapshot(&resolved, &content)
                    {
                        tracing::warn!("Failed to snapshot before write: {e}");
                    }

                    // Write is fast (atomic file write) — run inline
                    let (outcome, written_bytes) = write_call.execute(&resolved);

                    // Update file tracker with new content
                    if let Some(new_bytes) = &written_bytes
                        && let Ok(mtime) = std::fs::metadata(&resolved).and_then(|m| m.modified())
                    {
                        io.file_tracker
                            .update_after_edit(&resolved, new_bytes, mtime);
                    }

                    let preview = if !outcome.is_error() {
                        Some(ToolPreviewData::Write(
                            crate::diff::WritePreview::from_content(&write_call.content),
                        ))
                    } else {
                        None
                    };

                    let _ = tx.send(DriverEvent::Fsm(Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview,
                    }));
                }
                ClientToolCall::Read(read_call) => {
                    // Read is fast (file read) — run inline so we can update file_tracker
                    let outcome = read_call.execute();

                    // Track the read for freshness checking on subsequent edits
                    if !outcome.is_error() {
                        let resolved = read_call.resolved_path();
                        if resolved.is_file()
                            && let Ok(content) = std::fs::read(&resolved)
                            && let Ok(mtime) =
                                std::fs::metadata(&resolved).and_then(|m| m.modified())
                        {
                            io.file_tracker.record_read(resolved, &content, mtime);
                        }
                    }

                    let _ = tx.send(DriverEvent::Fsm(Event::ToolExecutionDone {
                        tool_id,
                        outcome,
                        preview: None,
                    }));
                }
                ClientToolCall::AtuinHistory(_) => {
                    // History search needs async DB access
                    tokio::spawn(async move {
                        let outcome = tool.execute(&db).await;
                        let _ = tx.send(DriverEvent::Fsm(Event::ToolExecutionDone {
                            tool_id,
                            outcome,
                            preview: None,
                        }));
                    });
                }
            }
        }

        Effect::AbortTool { tool_id } => {
            if let Some(abort_tx) = tool_abort_txs.remove(tool_id) {
                let _ = abort_tx.send(());
            }
        }

        Effect::Persist => {
            // Handled inline in the driver loop (before this function is called).
        }

        Effect::WritePermissionRule {
            target,
            rule,
            disposition,
        } => {
            let file_path = match target {
                PermissionTarget::Project => {
                    let project_root = io
                        .app_ctx
                        .git_root
                        .clone()
                        .or_else(|| std::env::current_dir().ok())
                        .unwrap_or_else(|| PathBuf::from("."));
                    writer::project_permissions_path(&project_root)
                }
                PermissionTarget::Global => writer::global_permissions_path(),
            };
            let rule = rule.clone();
            let disposition = disposition.clone();
            tokio::spawn(async move {
                if let Err(e) = writer::write_rule(&file_path, &rule, disposition).await {
                    tracing::error!("Failed to write permission rule: {e}");
                }
            });
        }

        Effect::CacheSessionGrant { path } => {
            io.edit_permissions.grant(path.clone());
        }

        Effect::ArchiveSession => {
            let rt = tokio::runtime::Handle::current();
            if let Err(e) = rt.block_on(io.session_mgr.archive_and_reset()) {
                tracing::warn!("Failed to archive session: {e}");
            }
        }

        Effect::ScheduleTimeout {
            timeout_id,
            duration,
        } => {
            let timeout_id = *timeout_id;
            let duration = *duration;
            let tx = tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;
                let _ = tx.send(DriverEvent::Fsm(Event::ConfirmationTimeout { timeout_id }));
            });
        }

        Effect::ExitApp(action) => {
            let action = action.clone();
            handle.update(move |vs| {
                vs.exit_action = Some(action);
            });
            exiting.store(true, Ordering::Release);
            let h2 = handle.clone();
            h2.exit();
        }
    }
}

// ============================================================================
// Persistence
// ============================================================================

fn persist(fsm: &AgentFsm, io: &mut IoContext) {
    let start = std::time::Instant::now();
    let rt = tokio::runtime::Handle::current();

    if let Err(e) = rt.block_on(io.session_mgr.persist_events(&fsm.ctx.events)) {
        tracing::warn!("Failed to persist session events: {e}");
    }
    if let Some(ref sid) = fsm.ctx.session_id
        && let Err(e) = rt.block_on(io.session_mgr.persist_server_session_id(sid))
    {
        tracing::warn!("Failed to persist server session ID: {e}");
    }
    if let Ok(json) = io.file_tracker.to_json()
        && let Err(e) = rt.block_on(
            io.session_mgr
                .set_metadata(crate::file_tracker::METADATA_KEY, &json),
        )
    {
        tracing::warn!("Failed to persist file tracker: {e}");
    }
    if let Ok(json) = io.edit_permissions.to_json()
        && let Err(e) = rt.block_on(
            io.session_mgr
                .set_metadata(crate::edit_permissions::METADATA_KEY, &json),
        )
    {
        tracing::warn!("Failed to persist edit permissions: {e}");
    }
    tracing::trace!(elapsed_ms = start.elapsed().as_millis(), "persist complete");
}

// ============================================================================
// Stream bridge
// ============================================================================

async fn run_stream_bridge(
    request: ChatRequest,
    app_ctx: AppContext,
    client_ctx: ClientContext,
    tx: mpsc::Sender<DriverEvent>,
    mut cancel_rx: tokio::sync::watch::Receiver<()>,
) {
    use crate::stream::{StreamContent, StreamControl, StreamFrame, create_chat_stream};
    use futures::StreamExt;

    let stream = create_chat_stream(
        app_ctx.endpoint.clone(),
        app_ctx.token.clone(),
        request,
        client_ctx,
        app_ctx.send_cwd,
        app_ctx.last_command.clone(),
    );
    futures::pin_mut!(stream);

    let _ = tx.send(DriverEvent::Fsm(Event::StreamStarted));

    loop {
        // Select between the next stream frame and cancellation.
        // When the driver drops the cancel sender, changed() returns Err
        // and we break — dropping the HTTP stream and cancelling the request.
        let frame = tokio::select! {
            biased;
            _ = cancel_rx.changed() => break,
            frame = stream.next() => match frame {
                Some(frame) => frame,
                None => break,
            },
        };

        let event = match frame {
            Ok(StreamFrame::Content(content)) => match content {
                StreamContent::TextChunk(text) => Some(Event::StreamChunk(text)),
                StreamContent::ToolCall { id, name, input } => {
                    if name == "suggest_command" {
                        Some(Event::SuggestCommand { id, input })
                    } else {
                        Some(Event::StreamToolCall { id, name, input })
                    }
                }
                StreamContent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    remote,
                    content_length,
                } => Some(Event::StreamServerToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    remote,
                    content_length,
                }),
            },
            Ok(StreamFrame::Control(control)) => match control {
                StreamControl::StatusChanged(status) => Some(Event::StreamStatusChanged(status)),
                StreamControl::Done { session_id } => Some(Event::StreamDone { session_id }),
                StreamControl::Error(msg) => Some(Event::StreamError(msg)),
            },
            Err(e) => Some(Event::StreamError(e.to_string())),
        };

        if let Some(event) = event {
            // StreamDone and StreamError are terminal — the server won't send more.
            // SuggestCommand is NOT terminal: the server sends StreamDone after it
            // with the session_id we need to capture.
            let is_terminal = matches!(event, Event::StreamDone { .. } | Event::StreamError(_));
            if tx.send(DriverEvent::Fsm(event)).is_err() {
                break;
            }
            if is_terminal {
                break;
            }
        }
    }
}
