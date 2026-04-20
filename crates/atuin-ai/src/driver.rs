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
use crate::fsm::events::{Event, PermissionResponse};
use crate::fsm::tools::ToolPreviewData;
use crate::fsm::{AgentFsm, AgentState};
use crate::permissions::resolver::PermissionResolver;
use crate::permissions::writer;
use crate::session::SessionManager;
use crate::stream::ChatRequest;
use crate::tools::ClientToolCall;
use crate::tui::state::ConversationEvent;

/// IO-side state that the driver owns. Not visible to the FSM.
pub(crate) struct IoContext {
    pub app_ctx: AppContext,
    pub client_ctx: ClientContext,
    pub session_mgr: SessionManager,
    pub file_tracker: FileReadTracker,
    pub edit_permissions: EditPermissionCache,
    pub snapshot_store: Option<crate::snapshots::SnapshotStore>,
}

/// State pushed to the Handle for the view/render thread.
///
/// Synced from the FSM after each transition. The view function reads this.
#[derive(Debug)]
pub(crate) struct ViewState {
    // ─── From FSM ───────────────────────────────────────────────
    pub agent_state: AgentState,
    /// Visible events (from view_start_index onward).
    pub visible_events: Vec<ConversationEvent>,
    /// Full conversation events (for API message building — retained for persistence).
    pub all_events: Vec<ConversationEvent>,
    /// Server session ID.
    pub session_id: Option<String>,
    /// Tool manager state for render data lookups.
    pub tools: crate::fsm::tools::ToolManager,
    /// Streaming text being accumulated (not yet in events).
    pub current_response: String,

    // ─── Session metadata (set once) ────────────────────────────
    pub is_resumed: bool,
    pub last_event_time: Option<chrono::DateTime<chrono::Utc>>,
    pub in_git_project: bool,
    pub invocation_id: String,

    // ─── View-only ──────────────────────────────────────────────
    pub archived_events: Vec<ConversationEvent>,
    pub view_start_index: usize,

    // ─── Ephemeral interaction state ────────────────────────────
    pub is_input_blank: bool,
    pub slash_command_input: Option<String>,
    pub slash_command_search_results: Vec<crate::tui::slash::SlashCommandSearchResult>,
    pub exit_action: Option<ExitAction>,

    // ─── Slash registry (for /help rendering) ───────────────────
    pub slash_registry: crate::tui::slash::SlashCommandRegistry,
}

impl ViewState {
    pub fn is_exiting(&self) -> bool {
        self.exit_action.is_some()
    }
}

/// Main driver loop. Processes events, transitions FSM, syncs view, executes effects.
///
/// Runs on a blocking thread. Returns when the event channel closes or exit is requested.
pub(crate) fn run_driver(
    mut fsm: AgentFsm,
    mut io: IoContext,
    handle: Handle<ViewState>,
    rx: mpsc::Receiver<Event>,
    tx: mpsc::Sender<Event>,
    exiting: Arc<AtomicBool>,
    initial_view: ViewState,
) {
    // Push the initial view state so the first render has something.
    let view_start_index = initial_view.view_start_index;
    let is_resumed = initial_view.is_resumed;
    let last_event_time = initial_view.last_event_time;
    let in_git_project = initial_view.in_git_project;
    handle.update(|vs| *vs = initial_view);

    while let Ok(event) = rx.recv() {
        // 1. Feed event to FSM
        let effects = fsm.handle(event);

        // 2. Sync ViewState to Handle
        sync_view_state(
            &handle,
            &fsm,
            view_start_index,
            is_resumed,
            last_event_time,
            in_git_project,
        );

        // 3. Execute effects
        for effect in effects {
            execute_effect(
                &effect,
                &fsm,
                &mut io,
                &handle,
                &tx,
                &exiting,
                in_git_project,
            );
        }

        // 4. Persist after each cycle
        persist(&fsm, &mut io);

        // 5. Check exit
        if exiting.load(Ordering::Acquire) {
            break;
        }
    }
}

/// Push current FSM state into the Handle for the view to read.
fn sync_view_state(
    handle: &Handle<ViewState>,
    fsm: &AgentFsm,
    view_start_index: usize,
    is_resumed: bool,
    last_event_time: Option<chrono::DateTime<chrono::Utc>>,
    in_git_project: bool,
) {
    let state = fsm.state.clone();
    let visible_events = fsm.ctx.events[view_start_index..].to_vec();
    let tools = fsm.ctx.tools.clone();
    let current_response = fsm.ctx.current_response.clone();
    let session_id = fsm.ctx.session_id.clone();

    handle.update(move |vs| {
        vs.agent_state = state;
        vs.visible_events = visible_events;
        vs.tools = tools;
        vs.current_response = current_response;
        vs.session_id = session_id;
        vs.is_resumed = is_resumed;
        vs.last_event_time = last_event_time;
        vs.in_git_project = in_git_project;
    });
}

/// Execute a single effect.
fn execute_effect(
    effect: &Effect,
    fsm: &AgentFsm,
    io: &mut IoContext,
    handle: &Handle<ViewState>,
    tx: &mpsc::Sender<Event>,
    exiting: &Arc<AtomicBool>,
    _in_git_project: bool,
) {
    match effect {
        Effect::StartStream {
            messages,
            session_id,
        } => {
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
                run_stream_bridge(request, app, cc, tx).await;
            });
        }

        Effect::AbortStream => {
            // The stream task holds no abort handle in the new architecture.
            // It will naturally stop when it can't send events (channel closed or
            // FSM ignores stale events). For immediate abort, we'd need to track
            // a JoinHandle — deferred for now; the FSM's stale-event handling
            // makes this safe.
            //
            // TODO: Add JoinHandle tracking for immediate abort if latency matters.
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

            // Check session grants first
            if let Some(resolved) = tool.resolved_file_path() {
                if io.edit_permissions.has_valid_grant(&resolved) {
                    let _ = tx.send(Event::PermissionResolved {
                        tool_id,
                        response: PermissionResponse::SessionGranted,
                    });
                    return;
                }
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
                let _ = tx.send(Event::PermissionResolved { tool_id, response });
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

                    tokio::spawn(async move {
                        let (output_tx, mut output_rx) =
                            tokio::sync::mpsc::channel::<Vec<String>>(16);
                        let (_interrupt_tx, interrupt_rx) = tokio::sync::oneshot::channel();

                        // Forward preview updates in a separate task
                        let preview_id = tool_id_for_preview;
                        let tx_fwd = tx_preview;
                        tokio::spawn(async move {
                            while let Some(lines) = output_rx.recv().await {
                                let _ = tx_fwd.send(Event::ToolPreviewUpdate {
                                    tool_id: preview_id.clone(),
                                    lines,
                                    exit_code: None,
                                });
                            }
                        });

                        // Run the command (blocking on completion)
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

                        let _ = tx.send(Event::ToolExecutionDone {
                            tool_id,
                            outcome,
                            preview,
                        });
                    });
                }
                _ => {
                    // Simple tools (read, edit, write, history)
                    tokio::spawn(async move {
                        let outcome = tool.execute(&db).await;
                        let _ = tx.send(Event::ToolExecutionDone {
                            tool_id,
                            outcome,
                            preview: None,
                        });
                    });
                }
            }
        }

        Effect::AbortTool { tool_id: _ } => {
            // TODO: Track interrupt senders per tool and send abort signal.
            // For now, the process will be killed when the task is dropped.
        }

        Effect::Persist => {
            // Persistence is done after every cycle in the main loop.
            // This effect is a no-op since we always persist.
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
                let _ = tx.send(Event::ConfirmationTimeout { timeout_id });
            });
        }

        Effect::CancelTimeout { timeout_id: _ } => {
            // Timeout cancellation is implicit: the FSM checks the timeout_id
            // when the event arrives and ignores it if it doesn't match.
            // No explicit cancellation mechanism needed.
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

/// Persist conversation state to disk.
fn persist(fsm: &AgentFsm, io: &mut IoContext) {
    let rt = tokio::runtime::Handle::current();

    if let Err(e) = rt.block_on(io.session_mgr.persist_events(&fsm.ctx.events)) {
        tracing::warn!("Failed to persist session events: {e}");
    }
    if let Some(ref sid) = fsm.ctx.session_id {
        if let Err(e) = rt.block_on(io.session_mgr.persist_server_session_id(sid)) {
            tracing::warn!("Failed to persist server session ID: {e}");
        }
    }
    if let Ok(json) = io.file_tracker.to_json() {
        if let Err(e) = rt.block_on(
            io.session_mgr
                .set_metadata(crate::file_tracker::METADATA_KEY, &json),
        ) {
            tracing::warn!("Failed to persist file tracker: {e}");
        }
    }
    if let Ok(json) = io.edit_permissions.to_json() {
        if let Err(e) = rt.block_on(
            io.session_mgr
                .set_metadata(crate::edit_permissions::METADATA_KEY, &json),
        ) {
            tracing::warn!("Failed to persist edit permissions: {e}");
        }
    }
}

/// Bridge between the streaming SSE connection and FSM events.
///
/// Translates `StreamFrame`s into FSM `Event`s and sends them on the channel.
async fn run_stream_bridge(
    request: ChatRequest,
    app_ctx: AppContext,
    client_ctx: ClientContext,
    tx: mpsc::Sender<Event>,
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

    // Signal stream started
    let _ = tx.send(Event::StreamStarted);

    while let Some(frame) = stream.next().await {
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
            let is_terminal = matches!(
                event,
                Event::StreamDone { .. } | Event::StreamError(_) | Event::SuggestCommand { .. }
            );
            if tx.send(event).is_err() {
                break; // Channel closed
            }
            if is_terminal {
                break;
            }
        }
    }
}
