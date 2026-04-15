use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::context::{AppContext, ClientContext};
use crate::context_window::ContextWindowBuilder;
use crate::permissions::check::PermissionResponse;
use crate::permissions::resolver::PermissionResolver;
use crate::permissions::rule::Rule;
use crate::permissions::writer::{self, RuleDisposition};
use crate::session::SessionManager;
use crate::stream::{ChatRequest, run_chat_stream};
use crate::tools::{ClientToolCall, ToolPhase};
use crate::tui::events::{AiTuiEvent, PermissionResult};
use crate::tui::state::{ConversationEvent, ExitAction, Session};
use eye_declare::Handle;
use tokio::task::JoinHandle;

/// Shared context for the dispatch loop. Bundles the references every
/// handler might need so `dispatch` doesn't forward a different subset
/// to each one.
pub(crate) struct DispatchContext<'a> {
    pub handle: &'a Handle<Session>,
    pub tx: &'a mpsc::Sender<AiTuiEvent>,
    pub app_ctx: &'a AppContext,
    pub client_ctx: &'a ClientContext,
    pub session_mgr: &'a mut SessionManager,
    /// Set by any handler that calls `h.exit()`. Read by `dispatch()`
    /// to break the loop — without round-tripping through the handle,
    /// which would hang if the TUI has already stopped.
    pub exiting: Arc<AtomicBool>,
}

/// Dispatch a single event. Returns `true` to keep the loop running,
/// `false` to shut down (after the final persist has completed).
pub(crate) fn dispatch(ctx: &mut DispatchContext, event: AiTuiEvent) -> bool {
    match event {
        AiTuiEvent::ContinueAfterTools => on_continue_after_tools(ctx),
        AiTuiEvent::InputUpdated(input) => on_input_updated(ctx, input),
        AiTuiEvent::SubmitInput(input) => on_submit_input(ctx, input),
        AiTuiEvent::SlashCommand(cmd) => on_slash_command(ctx, cmd),
        AiTuiEvent::CheckToolCallPermission(id) => on_check_tool_permission(ctx, id),
        AiTuiEvent::SelectPermission(result) => on_select_permission(ctx, result),
        AiTuiEvent::CancelGeneration => on_cancel_generation(ctx),
        AiTuiEvent::ExecuteCommand => on_execute_command(ctx),
        AiTuiEvent::CancelConfirmation => on_cancel_confirmation(ctx),
        AiTuiEvent::InterruptToolExecution => on_interrupt_tool_execution(ctx),
        AiTuiEvent::InsertCommand => on_insert_command(ctx),
        AiTuiEvent::Retry => on_retry(ctx),
        AiTuiEvent::Exit => on_exit(ctx),
    }

    // Persist any new conversation events after each dispatch cycle.
    persist_session(ctx);

    // The exiting flag is set by any handler that calls h.exit(). We
    // read it here rather than querying state through the handle,
    // because the TUI thread may have already stopped processing
    // handle requests by this point.
    !ctx.exiting.load(Ordering::Acquire)
}

/// Persist new events and the server session ID if it has changed.
/// Called from the dispatch thread (sync), bridges to async via the tokio handle.
fn persist_session(ctx: &mut DispatchContext) {
    let Ok((events, server_sid)) = ctx
        .handle
        .fetch(|state| {
            (
                state.conversation.events.clone(),
                state.conversation.session_id.clone(),
            )
        })
        .blocking_recv()
    else {
        return;
    };

    let rt = tokio::runtime::Handle::current();
    if let Err(e) = rt.block_on(ctx.session_mgr.persist_events(&events)) {
        tracing::warn!("failed to persist session events: {e}");
    }
    if let Some(ref sid) = server_sid
        && let Err(e) = rt.block_on(ctx.session_mgr.persist_server_session_id(sid))
    {
        tracing::warn!("failed to persist server session ID: {e}");
    }
}

fn launch_stream(ctx: &DispatchContext, setup: impl FnOnce(&mut Session) + Send + 'static) {
    let h2 = ctx.handle.clone();
    let tx2 = ctx.tx.clone();
    let app = ctx.app_ctx.clone();
    let cc = ctx.client_ctx.clone();
    let caps = ctx.app_ctx.capabilities.clone();
    ctx.handle.update(move |state| {
        (setup)(state);
        state.start_streaming();
        let messages =
            ContextWindowBuilder::with_default_budget().build(&state.conversation.events);
        let sid = state.conversation.session_id.clone();
        let request = ChatRequest::new(messages, sid, &caps, state.invocation_id.clone());
        let task: JoinHandle<()> = tokio::spawn(async move {
            run_chat_stream(h2, tx2, app, cc, request).await;
        });
        state.stream_abort = Some(task.abort_handle());
    });
}

fn on_continue_after_tools(ctx: &mut DispatchContext) {
    launch_stream(ctx, |_state| {});
}

fn on_input_updated(ctx: &mut DispatchContext, input: String) {
    let input_blank = input.is_empty();
    let slash_command = if input.starts_with('/') {
        Some(input.trim_start_matches('/').to_string())
    } else {
        None
    };

    ctx.handle.update(move |state| {
        state.interaction.is_input_blank = input_blank;
        state.interaction.slash_command_input = slash_command;

        if let Some(query) = state.interaction.slash_command_input.as_ref() {
            let mut results = state.slash_registry.search_fuzzy(query);

            results.sort_by(|a, b| {
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            state.interaction.slash_command_search_results = results;
        } else {
            state.interaction.slash_command_search_results.clear();
        }
    });
}

fn on_submit_input(ctx: &mut DispatchContext, input: String) {
    ctx.handle.update(move |state| {
        state.interaction.slash_command_input = None;
        state.interaction.slash_command_search_results.clear();
    });

    let input = input.trim().to_string();
    if input.is_empty() {
        let h2 = ctx.handle.clone();
        let exiting = ctx.exiting.clone();
        ctx.handle.update(move |state| {
            if state.conversation.has_any_command() {
                state.exit_action = Some(ExitAction::Execute(
                    state.conversation.current_command().unwrap().to_string(),
                ));
            } else {
                state.exit_action = Some(ExitAction::Cancel);
            }
            exiting.store(true, Ordering::Release);
            h2.exit();
        });
        return;
    }

    if input.starts_with('/') {
        if input.trim() == "/new" {
            on_new_session(ctx);
        } else {
            ctx.handle.update(move |state| {
                state
                    .conversation
                    .handle_slash_command(&input, &state.slash_registry);
            });
        }
        return;
    }

    // Start generation and spawn streaming task
    launch_stream(ctx, |state| {
        state.start_generating(input);
        state.interaction.is_input_blank = true;
    });
}

fn on_slash_command(ctx: &mut DispatchContext, command: String) {
    ctx.handle.update(move |state| {
        state
            .conversation
            .handle_slash_command(&command, &state.slash_registry);
    });
}

// ───────────────────────────────────────────────────────────────────
// Tool execution dispatch
// ───────────────────────────────────────────────────────────────────

/// Execute a tool call. Handles Shell tools (streaming with preview) and
/// non-shell tools (synchronous) uniformly.
fn execute_tool(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    tool_id: String,
    tool: ClientToolCall,
    db: &std::sync::Arc<atuin_client::database::Sqlite>,
) {
    match &tool {
        ClientToolCall::Shell(shell_call) => {
            let shell_call = shell_call.clone();
            execute_shell_tool(handle, tx, &tool_id, &shell_call);
        }
        _ => {
            execute_simple_tool(handle, tx, tool_id, tool, db);
        }
    }
}

/// Execute a non-shell tool and finish the tool call.
/// The ToolCall event is already in the conversation (added by handle_client_tool_call).
fn execute_simple_tool(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    tool_id: String,
    tool: ClientToolCall,
    db: &std::sync::Arc<atuin_client::database::Sqlite>,
) {
    let h = handle.clone();
    let tx = tx.clone();
    let db = db.clone();

    tokio::spawn(async move {
        let outcome = tool.execute(&db).await;
        h.update(move |state| {
            state.finish_tool_call(&tool_id, outcome);
            if !state.tool_tracker.has_pending() {
                let _ = tx.send(AiTuiEvent::ContinueAfterTools);
            }
        });
    });
}

/// Execute a shell tool with streaming VT100 preview.
fn execute_shell_tool(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    tool_id: &str,
    shell_call: &crate::tools::ShellToolCall,
) {
    let h = handle.clone();
    let tx = tx.clone();
    let shell_call = shell_call.clone();
    let command = shell_call.command.clone();
    let tc_id = tool_id.to_string();

    // 1. Set up channels for streaming output and interruption
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<Vec<String>>(32);
    let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();

    // 2. Mark as executing with preview and store the abort sender on the tracker entry
    let tc_id_setup = tc_id.clone();
    h.update(move |state| {
        if let Some(tracked) = state.tool_tracker.get_mut(&tc_id_setup) {
            tracked.mark_executing_preview(command);
            tracked.abort_tx = Some(abort_tx);
        }
    });

    // 3. Spawn a task to consume output updates and feed them to state
    let h_output = h.clone();
    let preview_id = tc_id.clone();
    let output_task = tokio::spawn(async move {
        while let Some(lines) = output_rx.recv().await {
            let id = preview_id.clone();
            h_output.update(move |state| {
                if let Some(tracked) = state.tool_tracker.get_mut(&id)
                    && let ToolPhase::ExecutingWithPreview {
                        ref mut output_lines,
                        ..
                    } = tracked.phase
                {
                    *output_lines = lines;
                }
            });
        }
    });

    // 4. Spawn the streaming execution task
    let tc_id_finish = tc_id;
    tokio::spawn(async move {
        let outcome =
            crate::tools::execute_shell_command_streaming(&shell_call, output_tx, abort_rx).await;

        // Wait for the output task to finish so the final preview lines are captured
        let _ = output_task.await;

        h.update(move |state| {
            state.finish_tool_call(&tc_id_finish, outcome);
            if !state.tool_tracker.has_pending() {
                let _ = tx.send(AiTuiEvent::ContinueAfterTools);
            }
        });
    });
}

// ───────────────────────────────────────────────────────────────────
// Permission handlers
// ───────────────────────────────────────────────────────────────────

fn on_check_tool_permission(ctx: &mut DispatchContext, id: String) {
    let h2 = ctx.handle.clone();
    let tx_for_task = ctx.tx.clone();
    let db = ctx.app_ctx.history_db.clone();

    tokio::spawn(async move {
        let id_for_error = id.clone();
        let result = check_tool_permission_inner(&h2, &tx_for_task, &db, id).await;

        // If the inner function didn't handle the tool (returned an error message),
        // finish the tool call with that error so the conversation doesn't stall.
        if let Err(error_msg) = result {
            let tx = tx_for_task.clone();
            h2.update(move |state| {
                state.finish_tool_call(&id_for_error, crate::tools::ToolOutcome::Error(error_msg));
                if !state.tool_tracker.has_pending() {
                    let _ = tx.send(AiTuiEvent::ContinueAfterTools);
                }
            });
        }
    });
}

/// Inner permission check that returns Err(message) if the tool call should be
/// finished with an error. Returns Ok(()) if the tool was handled (executed,
/// denied, or sent to the permission UI).
async fn check_tool_permission_inner(
    h2: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    db: &std::sync::Arc<atuin_client::database::Sqlite>,
    id: String,
) -> Result<(), String> {
    // 1. Fetch the tracked tool's data
    let id_for_fetch = id.clone();
    let (tool, target_dir) = h2
        .fetch(move |state| {
            state
                .tool_tracker
                .get(&id_for_fetch)
                .map(|t| (t.tool.clone(), t.target_dir().map(PathBuf::from)))
        })
        .await
        .map_err(|e| format!("Internal error fetching tool state: {e}"))?
        .ok_or_else(|| "Internal error: tool not found in tracker".to_string())?;

    // 2. Resolve working directory
    let working_dir = target_dir
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "Could not determine working directory".to_string())?;

    // 3. Create permission resolver and check
    let resolver = PermissionResolver::new(working_dir)
        .await
        .map_err(|e| format!("Permission check failed: {e}"))?;

    let response = resolver
        .check(&tool)
        .await
        .map_err(|e| format!("Permission check failed: {e}"))?;

    // 4. Handle response — all paths here handle the tool, so return Ok
    let id_clone = id.clone();
    match response {
        PermissionResponse::Allowed => {
            execute_tool(h2, tx, id, tool, db);
        }
        PermissionResponse::Denied => {
            let tx = tx.clone();
            h2.update(move |state| {
                state.finish_tool_call(
                    &id_clone,
                    crate::tools::ToolOutcome::Error(
                        "Permission denied on the user's system".to_string(),
                    ),
                );
                if !state.tool_tracker.has_pending() {
                    let _ = tx.send(AiTuiEvent::ContinueAfterTools);
                }
            });
        }
        PermissionResponse::Ask => {
            h2.update(move |state| {
                if let Some(tracked) = state.tool_tracker.get_mut(&id_clone) {
                    tracked.mark_asking();
                }
            });
        }
    }

    Ok(())
}

fn on_select_permission(ctx: &mut DispatchContext, permission: PermissionResult) {
    let tx = ctx.tx.clone();
    let h2 = ctx.handle.clone();

    match permission {
        PermissionResult::Allow => {
            // Fetch the tool that's asking for permission, then execute it
            let db = ctx.app_ctx.history_db.clone();
            tokio::spawn(async move {
                let Ok(Some((tool_id, tool))) = h2
                    .fetch(move |state| {
                        state
                            .tool_tracker
                            .asking_for_permission()
                            .map(|t| (t.id.clone(), t.tool.clone()))
                    })
                    .await
                else {
                    return;
                };

                execute_tool(&h2, &tx, tool_id, tool, &db);
            });
        }
        PermissionResult::AlwaysAllowInDir => {
            let db = ctx.app_ctx.history_db.clone();
            let git_root = ctx.app_ctx.git_root.clone();
            tokio::spawn(async move {
                let Ok(Some((tool_id, tool))) = h2
                    .fetch(move |state| {
                        state
                            .tool_tracker
                            .asking_for_permission()
                            .map(|t| (t.id.clone(), t.tool.clone()))
                    })
                    .await
                else {
                    return;
                };

                // Write the rule to the project (git root) or cwd permissions file
                let project_root = git_root
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| PathBuf::from("."));
                let file_path = writer::project_permissions_path(&project_root);
                let rule = Rule {
                    tool: tool.rule_name().to_string(),
                    scope: None,
                };
                if let Err(e) = writer::write_rule(&file_path, &rule, RuleDisposition::Allow).await
                {
                    tracing::error!("Failed to write project permission rule: {e}");
                }

                execute_tool(&h2, &tx, tool_id, tool, &db);
            });
        }
        PermissionResult::AlwaysAllow => {
            let db = ctx.app_ctx.history_db.clone();
            tokio::spawn(async move {
                let Ok(Some((tool_id, tool))) = h2
                    .fetch(move |state| {
                        state
                            .tool_tracker
                            .asking_for_permission()
                            .map(|t| (t.id.clone(), t.tool.clone()))
                    })
                    .await
                else {
                    return;
                };

                // Write the rule to the global permissions file
                let file_path = writer::global_permissions_path();
                let rule = Rule {
                    tool: tool.rule_name().to_string(),
                    scope: None,
                };
                if let Err(e) = writer::write_rule(&file_path, &rule, RuleDisposition::Allow).await
                {
                    tracing::error!("Failed to write global permission rule: {e}");
                }

                execute_tool(&h2, &tx, tool_id, tool, &db);
            });
        }
        PermissionResult::Deny => {
            h2.update(move |state| {
                let Some(tracked) = state.tool_tracker.asking_for_permission() else {
                    return;
                };
                let tool_id = tracked.id.clone();

                state.finish_tool_call(
                    &tool_id,
                    crate::tools::ToolOutcome::Error("Permission denied by the user".to_string()),
                );
                if !state.tool_tracker.has_pending() {
                    let _ = tx.send(AiTuiEvent::ContinueAfterTools);
                }
            });
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Other handlers
// ───────────────────────────────────────────────────────────────────

fn on_cancel_generation(ctx: &mut DispatchContext) {
    ctx.handle.update(|state| match state.interaction.mode {
        crate::tui::state::AppMode::Generating => {
            state.cancel_generation();
        }
        crate::tui::state::AppMode::Streaming => {
            state.cancel_streaming();
        }
        _ => {}
    });
}

fn on_execute_command(ctx: &mut DispatchContext) {
    let h2 = ctx.handle.clone();
    let exiting = ctx.exiting.clone();
    ctx.handle.update(move |state| {
        let cmd = state.conversation.current_command().map(|c| c.to_string());
        if let Some(cmd) = cmd {
            if state.conversation.is_current_command_dangerous()
                && !state.interaction.confirmation_pending
            {
                state.interaction.confirmation_pending = true;
            } else {
                state.interaction.confirmation_pending = false;
                state.exit_action = Some(ExitAction::Execute(cmd));
                exiting.store(true, Ordering::Release);
                h2.exit();
            }
        }
    });
}

fn on_cancel_confirmation(ctx: &mut DispatchContext) {
    ctx.handle.update(move |state| {
        state.interaction.confirmation_pending = false;
    });
}

fn on_insert_command(ctx: &mut DispatchContext) {
    let h2 = ctx.handle.clone();
    let exiting = ctx.exiting.clone();
    ctx.handle.update(move |state| {
        let cmd = state.conversation.current_command().map(|c| c.to_string());
        if let Some(cmd) = cmd {
            state.interaction.confirmation_pending = false;
            state.exit_action = Some(ExitAction::Insert(cmd));
            exiting.store(true, Ordering::Release);
            h2.exit();
        }
    });
}

fn on_retry(ctx: &mut DispatchContext) {
    launch_stream(ctx, |state| {
        state.retry();
    });
}

fn on_new_session(ctx: &mut DispatchContext) {
    let rt = tokio::runtime::Handle::current();

    if let Err(e) = rt.block_on(ctx.session_mgr.archive_and_reset()) {
        tracing::warn!("failed to start new session: {e}");
        return;
    }

    ctx.handle.update(|state| {
        // Move the current invocation's visible events to the archived view
        // so they remain on screen but are no longer sent to the API.
        let visible_events: Vec<ConversationEvent> =
            state.conversation.events[state.view_start_index..].to_vec();
        state.archived_view_events.extend(visible_events);

        state.conversation.events.clear();
        state.conversation.session_id = None;
        state.tool_tracker = crate::tools::ToolTracker::new();
        state.view_start_index = 0;
        state.is_resumed = false;
        state.last_event_time = None;
        state
            .conversation
            .events
            .push(ConversationEvent::OutOfBandOutput {
                name: "System".to_string(),
                command: Some("/new".to_string()),
                content: "Started a new session.".to_string(),
            });
    });
}

fn on_exit(ctx: &mut DispatchContext) {
    let h2 = ctx.handle.clone();
    let exiting = ctx.exiting.clone();
    ctx.handle.update(move |state| {
        if let Some(abort) = state.stream_abort.take() {
            abort.abort();
        }
        state.exit_action = Some(ExitAction::Cancel);
        exiting.store(true, Ordering::Release);
        h2.exit();
    });
}

fn on_interrupt_tool_execution(ctx: &mut DispatchContext) {
    ctx.handle.update(move |state| {
        // Find executing previews, send interrupt, and mark as interrupted
        for tracked in state.tool_tracker.iter_mut() {
            if let ToolPhase::ExecutingWithPreview {
                ref mut interrupted,
                ref mut exit_code,
                ..
            } = tracked.phase
            {
                *interrupted = true;
                if exit_code.is_none() {
                    *exit_code = Some(-1);
                }
                // Send interrupt signal via the tracker entry's abort channel
                if let Some(abort_tx) = tracked.abort_tx.take() {
                    let _ = abort_tx.send(());
                }
            }
        }

        // The spawned execution task will handle finalizing and sending
        // ContinueAfterTools when the process exits. Input mode is already active.
    });
}
