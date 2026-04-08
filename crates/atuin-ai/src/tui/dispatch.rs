use std::path::PathBuf;
use std::sync::mpsc;

use crate::context::{AppContext, ClientContext};
use crate::permissions::check::PermissionResponse;
use crate::permissions::resolver::PermissionResolver;
use crate::stream::{ChatRequest, run_chat_stream};
use crate::tools::ToolCallState;
use crate::tui::ConversationEvent;
use crate::tui::events::{AiTuiEvent, PermissionResult};
use crate::tui::state::{ExitAction, Session};
use eye_declare::Handle;
use tokio::task::JoinHandle;

pub(crate) fn dispatch(
    handle: &Handle<Session>,
    event: AiTuiEvent,
    tx: &mpsc::Sender<AiTuiEvent>,
    app_ctx: &AppContext,
    client_ctx: &ClientContext,
) {
    match event {
        AiTuiEvent::ContinueAfterTools => {
            on_continue_after_tools(handle, tx, app_ctx, client_ctx);
        }
        AiTuiEvent::InputUpdated(input) => {
            on_input_updated(handle, input);
        }
        AiTuiEvent::SubmitInput(input) => {
            on_submit_input(handle, tx, app_ctx, client_ctx, input);
        }
        AiTuiEvent::SlashCommand(cmd) => {
            on_slash_command(handle, cmd);
        }
        AiTuiEvent::CheckToolCallPermission(id) => {
            on_check_tool_permission(handle, tx, app_ctx, id);
        }
        AiTuiEvent::SelectPermission(result) => {
            on_select_permission(handle, tx, app_ctx, result);
        }
        AiTuiEvent::CancelGeneration => {
            on_cancel_generation(handle);
        }
        AiTuiEvent::ExecuteCommand => {
            on_execute_command(handle);
        }
        AiTuiEvent::CancelConfirmation => {
            on_cancel_confirmation(handle);
        }
        AiTuiEvent::InsertCommand => {
            on_insert_command(handle);
        }
        AiTuiEvent::Retry => {
            on_retry(handle, tx, app_ctx, client_ctx);
        }
        AiTuiEvent::Exit => {
            on_exit(handle);
        }
    }
}

fn launch_stream(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    app_ctx: &AppContext,
    client_ctx: &ClientContext,
    setup: impl FnOnce(&mut Session) + Send + 'static,
) {
    let h2 = handle.clone();
    let tx2 = tx.clone();
    let app = app_ctx.clone();
    let cc = client_ctx.clone();
    handle.update(move |state| {
        (setup)(state);
        state.start_streaming();
        let messages = state.conversation.events_to_messages();
        let sid = state.conversation.session_id.clone();
        let request = ChatRequest::new(messages, sid);
        let task: JoinHandle<()> = tokio::spawn(async move {
            run_chat_stream(h2, tx2, app, cc, request).await;
        });
        state.stream_abort = Some(task.abort_handle());
    });
}

fn on_continue_after_tools(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    app_ctx: &AppContext,
    client_ctx: &ClientContext,
) {
    launch_stream(handle, tx, app_ctx, client_ctx, |_state| {});
}

fn on_input_updated(handle: &Handle<Session>, input: String) {
    let input_blank = input.trim().is_empty();

    handle.update(move |state| {
        state.interaction.is_input_blank = input_blank;
    });
}

fn on_submit_input(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    app_ctx: &AppContext,
    client_ctx: &ClientContext,
    input: String,
) {
    let input = input.trim().to_string();
    if input.is_empty() {
        let h2 = handle.clone();
        handle.update(move |state| {
            if state.conversation.has_any_command() {
                state.exit_action = Some(ExitAction::Execute(
                    state.conversation.current_command().unwrap().to_string(),
                ));
            } else {
                state.exit_action = Some(ExitAction::Cancel);
            }
            h2.exit();
        });
        return;
    }

    if input.starts_with('/') {
        let input_clone = input.clone();
        handle.update(move |state| {
            state.conversation.handle_slash_command(&input_clone);
        });
        return;
    }

    // Start generation and spawn streaming task
    launch_stream(handle, tx, app_ctx, client_ctx, |state| {
        state.start_generating(input);
        state.interaction.is_input_blank = true;
    });
}

fn on_slash_command(handle: &Handle<Session>, command: String) {
    handle.update(move |state| {
        state.conversation.handle_slash_command(&command);
    });
}

fn on_check_tool_permission(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    app_ctx: &AppContext,
    id: String,
) {
    let h2 = handle.clone();
    let tx_for_task = tx.clone();
    let id_clone = id.clone();
    let db = app_ctx.history_db.clone();

    tokio::spawn(async move {
        // 1. Fetch the pending tool call
        let Ok(Some(tool_call)) = h2
            .fetch(move |state| state.pending_tool_call(&id).cloned())
            .await
        else {
            return;
        };

        // 2. Resolve working directory
        let Some(working_dir) = tool_call
            .target_dir()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
        else {
            return;
        };

        // 3. Create permission resolver and check
        let Ok(resolver) = PermissionResolver::new(working_dir, None).await else {
            return;
        };

        let Ok(response) = resolver.check(&tool_call.tool).await else {
            return;
        };

        // 4. Handle response
        match response {
            PermissionResponse::Allowed => {
                let outcome = tool_call.tool.execute(&db).await;
                h2.update(move |state| {
                    state.complete_tool_call(&tool_call, outcome);
                    if !state.has_unresolved_tool_calls() {
                        let _ = tx_for_task.send(AiTuiEvent::ContinueAfterTools);
                    }
                });
            }
            PermissionResponse::Denied => {
                let tx = tx_for_task.clone();
                h2.update(move |state| {
                    state
                        .conversation
                        .events
                        .push(ConversationEvent::OutOfBandOutput {
                            name: "System".to_string(),
                            content: format!("Permission denied for tool call {:?}", &id_clone),
                            command: None,
                        });
                    state.pending_tool_calls.retain(|c| c.id != id_clone);
                    if !state.has_unresolved_tool_calls() {
                        let _ = tx.send(AiTuiEvent::ContinueAfterTools);
                    }
                });
            }
            PermissionResponse::Ask => {
                h2.update(move |state| {
                    if let Some(tc) = state.pending_tool_call_mut(&id_clone) {
                        tc.mark_asking();
                    }
                });
            }
        }
    });
}

fn on_select_permission(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    app_ctx: &AppContext,
    permission: PermissionResult,
) {
    let tx = tx.clone();
    let h2 = handle.clone();
    let db = app_ctx.history_db.clone();

    match permission {
        PermissionResult::Allow => {
            // Fetch the tool call that's asking for permission, then execute it async
            let h3 = h2.clone();
            let tx2 = tx.clone();
            tokio::spawn(async move {
                let Ok(Some(tool_call)) = h3
                    .fetch(move |state| {
                        state
                            .pending_tool_calls
                            .iter()
                            .find(|tc| tc.state == ToolCallState::AskingForPermission)
                            .cloned()
                    })
                    .await
                else {
                    return;
                };

                let outcome = tool_call.tool.execute(&db).await;
                h3.update(move |state| {
                    state.complete_tool_call(&tool_call, outcome);
                    if !state.has_unresolved_tool_calls() {
                        let _ = tx2.send(AiTuiEvent::ContinueAfterTools);
                    }
                });
            });
        }
        PermissionResult::AlwaysAllowInDir => {
            //
        }
        PermissionResult::AlwaysAllow => {
            //
        }
        PermissionResult::Deny => {
            h2.update(move |state| {
                let tool_call = state
                    .pending_tool_calls
                    .iter()
                    .enumerate()
                    .find(|(_, call)| call.state == ToolCallState::AskingForPermission);

                let Some((index, _)) = tool_call else {
                    return;
                };

                let Some(call) = state.pending_tool_calls.remove(index) else {
                    return;
                };

                state.conversation.add_tool_result(
                    call.id,
                    "Permission denied on the user's system".to_string(),
                    true,
                );
                if !state.has_unresolved_tool_calls() {
                    let _ = tx.send(AiTuiEvent::ContinueAfterTools);
                }
            });
        }
    }
}

fn on_cancel_generation(handle: &Handle<Session>) {
    handle.update(|state| match state.interaction.mode {
        crate::tui::state::AppMode::Generating => {
            state.cancel_generation();
        }
        crate::tui::state::AppMode::Streaming => {
            state.cancel_streaming();
        }
        _ => {}
    });
}

fn on_execute_command(handle: &Handle<Session>) {
    let h2 = handle.clone();
    handle.update(move |state| {
        let cmd = state.conversation.current_command().map(|c| c.to_string());
        if let Some(cmd) = cmd {
            if state.conversation.is_current_command_dangerous()
                && !state.interaction.confirmation_pending
            {
                state.interaction.confirmation_pending = true;
            } else {
                state.interaction.confirmation_pending = false;
                state.exit_action = Some(ExitAction::Execute(cmd));
                h2.exit();
            }
        }
    });
}

fn on_cancel_confirmation(handle: &Handle<Session>) {
    handle.update(move |state| {
        state.interaction.confirmation_pending = false;
    });
}

fn on_insert_command(handle: &Handle<Session>) {
    let h2 = handle.clone();
    handle.update(move |state| {
        let cmd = state.conversation.current_command().map(|c| c.to_string());
        if let Some(cmd) = cmd {
            state.interaction.confirmation_pending = false;
            state.exit_action = Some(ExitAction::Insert(cmd));
            h2.exit();
        }
    });
}

fn on_retry(
    handle: &Handle<Session>,
    tx: &mpsc::Sender<AiTuiEvent>,
    app_ctx: &AppContext,
    client_ctx: &ClientContext,
) {
    launch_stream(handle, tx, app_ctx, client_ctx, |state| {
        state.retry();
    });
}

fn on_exit(handle: &Handle<Session>) {
    let h2 = handle.clone();
    handle.update(move |state| {
        if let Some(abort) = state.stream_abort.take() {
            abort.abort();
        }
        state.exit_action = Some(ExitAction::Cancel);
        h2.exit();
    });
}
