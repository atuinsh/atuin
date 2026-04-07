use std::path::PathBuf;
use std::sync::mpsc;

use crate::permissions::check::{PermissionChecker, PermissionRequest, PermissionResponse};
use crate::permissions::walker::PermissionWalker;
use crate::stream::run_chat_stream;
use crate::tools::ToolCallState;
use crate::tui::ConversationEvent;
use crate::tui::events::{AiTuiEvent, PermissionResult};
use crate::tui::state::{AppState, ExitAction};
use crate::tui::view::ai_view;
use eye_declare::{Application, CtrlCBehavior};
use eyre::{Context as _, Result, bail};
use serde_json::json;
use tracing::{debug, info};

pub(crate) async fn run(
    initial_command: Option<String>,
    api_endpoint: Option<String>,
    api_token: Option<String>,
    settings: &atuin_client::settings::Settings,
    output_for_hook: bool,
) -> Result<()> {
    if !settings.ai.enabled.unwrap_or(false) {
        emit_shell_result(
            Action::Print(
                "Atuin AI is not enabled. Please enable it in your settings or run `atuin setup`."
                    .to_string(),
            ),
            output_for_hook,
        );
        return Ok(());
    }

    let endpoint = api_endpoint.as_deref().unwrap_or(
        settings
            .ai
            .endpoint
            .as_deref()
            .unwrap_or("https://hub.atuin.sh"),
    );
    let api_token = api_token.as_deref().or(settings.ai.api_token.as_deref());

    let token = if let Some(token) = &api_token {
        token.to_string()
    } else {
        ensure_hub_session(settings).await?
    };

    let action = run_inline_tui(endpoint.to_string(), token, initial_command, settings).await?;
    emit_shell_result(action, output_for_hook);

    Ok(())
}

async fn ensure_hub_session(settings: &atuin_client::settings::Settings) -> Result<String> {
    if let Some(token) = atuin_client::hub::get_session_token().await? {
        debug!("Found Hub session, using existing token");
        return Ok(token);
    }

    let hub_address = settings.active_hub_endpoint().unwrap_or_default();
    let will_sync = settings.is_hub_sync();

    info!("No Hub session found, prompting for authentication");

    println!("Atuin AI requires authenticating with Atuin Hub.");
    if will_sync {
        println!(
            "Once logged in, your shell history will be synchronized via Atuin Hub if auto_sync is enabled or when manually syncing."
        )
    }
    println!(
        "If you have an existing Atuin sync account, you can log in with your existing credentials."
    );
    println!("Press enter to begin (or esc to cancel).");
    if !wait_for_login_confirmation()? {
        bail!("authentication canceled");
    }

    debug!("Starting Atuin Hub authentication...");
    println!("Authenticating with Atuin Hub...");

    let session = atuin_client::hub::HubAuthSession::start(hub_address.as_ref()).await?;
    println!("Open this URL to continue:");
    println!("{}", session.auth_url);

    let token = session
        .wait_for_completion(
            atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
            atuin_client::hub::DEFAULT_POLL_INTERVAL,
        )
        .await?;

    info!("Authentication complete, saving session token");
    atuin_client::hub::save_session(&token).await?;

    if let Ok(meta) = atuin_client::settings::Settings::meta_store().await
        && let Ok(Some(cli_token)) = meta.session_token().await
    {
        debug!("CLI session found, attempting to link accounts");
        if let Err(e) = atuin_client::hub::link_account(hub_address.as_ref(), &cli_token).await {
            debug!("Could not link CLI account to Hub: {}", e);
        } else {
            info!("Successfully linked CLI account to Hub");
        }
    }

    Ok(token)
}

// ───────────────────────────────────────────────────────────────────
// Main TUI entry point
// ───────────────────────────────────────────────────────────────────

async fn run_inline_tui(
    endpoint: String,
    token: String,
    initial_prompt: Option<String>,
    settings: &atuin_client::settings::Settings,
) -> Result<Action> {
    let (tx, rx) = mpsc::channel::<AiTuiEvent>();

    let mut initial_state = AppState::new(tx.clone());
    // initial_state
    //     .pending_tool_calls
    //     .push_back(crate::tools::PendingToolCall {
    //         id: "1".to_string(),
    //         state: crate::tools::ToolCallState::CheckingPermissions,
    //         tool: crate::tools::ClientToolCall::Read(crate::tools::ReadToolCall {
    //             path: std::path::PathBuf::from("test.txt"),
    //         }),
    //     });
    // initial_state
    //     .pending_tool_calls
    //     .push_back(crate::tools::PendingToolCall {
    //         id: "2".to_string(),
    //         state: crate::tools::ToolCallState::CheckingPermissions,
    //         tool: crate::tools::ClientToolCall::Shell(crate::tools::ShellToolCall {
    //             dir: None,
    //             command: "ls -lah".to_string(),
    //         }),
    //     });

    // let _ = tx.send(AiTuiEvent::CheckToolCallPermission("1".to_string()));
    // let _ = tx.send(AiTuiEvent::CheckToolCallPermission("2".to_string()));

    println!();

    // If there's an initial prompt, send it as a SubmitInput event
    // so it flows through the same path as user-typed input.
    if let Some(prompt) = initial_prompt {
        let _ = tx.send(AiTuiEvent::SubmitInput(prompt));
    }

    let (mut app, handle) = Application::builder()
        .state(initial_state)
        .view(ai_view)
        .ctrl_c(CtrlCBehavior::Deliver)
        .keyboard_protocol(eye_declare::KeyboardProtocol::Enhanced)
        .bracketed_paste(true)
        .with_context(tx)
        .extra_newlines_at_exit(1)
        .build()?;

    let send_cwd = settings.ai.send_cwd;

    // Event loop: receives AiTuiEvent from components, mutates state via Handle.
    let h = handle.clone();
    let ep = endpoint.clone();
    let tk = token.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(event) = rx.recv() {
            match event {
                AiTuiEvent::ContinueAfterTools => {
                    let ep = ep.clone();
                    let tk = tk.clone();
                    let h2 = h.clone();
                    h.update(move |state| {
                        state.start_streaming();
                        let messages = state.events_to_messages();
                        let sid = state.session_id.clone();
                        let task = tokio::spawn(async move {
                            run_chat_stream(h2, ep, tk, sid, messages, send_cwd).await;
                        });
                        state.stream_abort = Some(task.abort_handle());
                    });
                }

                AiTuiEvent::InputUpdated(input) => {
                    let input_blank = input.trim().is_empty();

                    h.update(move |state| {
                        state.is_input_blank = input_blank;
                    });
                }

                AiTuiEvent::SubmitInput(input) => {
                    let input = input.trim().to_string();
                    if input.is_empty() {
                        let h2 = h.clone();
                        h.update(move |state| {
                            if state.has_any_command() {
                                state.exit_action = Some(ExitAction::Execute(
                                    state.current_command().unwrap().to_string(),
                                ));
                            } else {
                                state.exit_action = Some(ExitAction::Cancel);
                            }
                            h2.exit();
                        });
                        continue;
                    }

                    if input.starts_with('/') {
                        let input_clone = input.clone();
                        h.update(move |state| {
                            state.handle_slash_command(&input_clone);
                        });
                        continue;
                    }

                    // Start generation and spawn streaming task
                    let ep = ep.clone();
                    let tk = tk.clone();
                    let h2 = h.clone();
                    h.update(move |state| {
                        state.start_generating(input);
                        state.start_streaming();
                        state.is_input_blank = true;
                        let messages = state.events_to_messages();
                        let sid = state.session_id.clone();
                        let task = tokio::spawn(async move {
                            run_chat_stream(h2, ep, tk, sid, messages, send_cwd).await;
                        });
                        state.stream_abort = Some(task.abort_handle());
                    });
                }

                AiTuiEvent::SlashCommand(command) => {
                    h.update(move |state| {
                        state.handle_slash_command(&command);
                    });
                }

                AiTuiEvent::CheckToolCallPermission(id) => {
                    eprintln!("Checking tool call permission: {:?}", &id);
                    let h2 = h.clone();

                    let id_clone = id.clone();
                    tokio::spawn(async move {
                        let Ok(Some(tool_call)) = h2
                            .fetch(move |state| state.get_pending_tool_call(&id).cloned())
                            .await
                        else {
                            // todo: raise error
                            eprintln!("Error getting pending tool call: {:?}", &id_clone);
                            return;
                        };

                        let Some(working_dir) = tool_call
                            .target_dir()
                            .map(PathBuf::from)
                            .or_else(|| std::env::current_dir().ok())
                        else {
                            // todo: raise error
                            eprintln!(
                                "Error getting working directory for tool call: {:?}",
                                &tool_call
                            );
                            return;
                        };

                        let mut walker = PermissionWalker::new(working_dir.clone(), None); // todo: get global dir

                        let Ok(_) = walker.walk().await else {
                            eprintln!("Error walking filesystem for permissions check");
                            // todo: raise error
                            return;
                        };

                        let checker = PermissionChecker::new(walker.rules().to_owned());
                        let request =
                            PermissionRequest::new(working_dir, Box::new(&tool_call.tool));

                        let Ok(response) = checker.check(&request).await else {
                            // todo: raise error
                            eprintln!("Error checking tool call permission");
                            return;
                        };

                        match response {
                            PermissionResponse::Allowed => {
                                eprintln!("Executing tool call: {:?}", tool_call);

                                let id_clone2 = id_clone.clone();
                                h2.update(move |state| {
                                    state.add_tool_call(
                                        id_clone2.clone(),
                                        "read".to_string(),
                                        json!({}),
                                    );

                                    let mut tool_call = state.get_pending_tool_call_mut(&id_clone2);

                                    let Some(tool_call) = tool_call.as_mut() else {
                                        eprintln!(
                                            "Error getting pending tool call: {:?}",
                                            &id_clone2
                                        );
                                        return;
                                    };

                                    tool_call.state = ToolCallState::Executing;

                                    //

                                    // state.events.push(ConversationEvent::OutOfBandOutput {
                                    //     name: "System".to_string(),
                                    //     content: format!(
                                    //         "Permission granted for tool call {:?}",
                                    //         &tool_call
                                    //     ),
                                    //     command: None,
                                    // });
                                });

                                match tool_call.tool {
                                    crate::tools::ClientToolCall::Read(read) => {
                                        let mut path = read.path.clone();

                                        if path.is_relative() {
                                            if let Ok(current_dir) = std::env::current_dir() {
                                                path = current_dir.join(path);
                                            }
                                        }

                                        if !path.exists() {
                                            let id = id_clone.clone();
                                            h2.update(move |state| {
                                                state.add_tool_result(
                                                    id.clone(),
                                                    format!(
                                                        "Error: file does not exist: {}",
                                                        path.display()
                                                    ),
                                                    true,
                                                );
                                                state.pending_tool_calls.retain(|c| c.id != id);
                                            });
                                            return;
                                        }

                                        if path.is_dir() {
                                            let Some(files) = std::fs::read_dir(&path)
                                                .map_err(|e| {
                                                    eprintln!("Error reading directory: {}", e);
                                                    e
                                                })
                                                .ok()
                                                .and_then(|entries| {
                                                    entries
                                                        .filter_map(|entry| entry.ok())
                                                        .map(|entry| {
                                                            entry
                                                                .file_name()
                                                                .to_string_lossy()
                                                                .to_string()
                                                        })
                                                        .collect::<Vec<_>>()
                                                        .into()
                                                })
                                            else {
                                                h2.update(move |state| {
                                                    state.add_tool_result(
                                                        id_clone.clone(),
                                                        format!(
                                                            "Error: could not read directory: {}",
                                                            path.display()
                                                        ),
                                                        true,
                                                    );
                                                    state
                                                        .pending_tool_calls
                                                        .retain(|c| c.id != id_clone);
                                                });
                                                return;
                                            };

                                            h2.update(move |state| {
                                                state.add_tool_result(
                                                    id_clone.clone(),
                                                    format!(
                                                        "Directory contents:\n{}",
                                                        files.join("\n")
                                                    ),
                                                    false,
                                                );
                                                state
                                                    .pending_tool_calls
                                                    .retain(|c| c.id != id_clone);

                                                let _ =
                                                    state.tx.send(AiTuiEvent::ContinueAfterTools);
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            PermissionResponse::Denied => {
                                eprintln!("Permission denied for tool call: {:?}", &tool_call);
                                h2.update(move |state| {
                                    state.events.push(ConversationEvent::OutOfBandOutput {
                                        name: "System".to_string(),
                                        content: format!(
                                            "Permission denied for tool call {:?}",
                                            &tool_call
                                        ),
                                        command: None,
                                    });
                                });
                            }
                            PermissionResponse::Ask => {
                                eprintln!("Asking for permission for tool call: {:?}", &tool_call);
                                h2.update(move |state| {
                                    let mut tool_call = state.get_pending_tool_call_mut(&id_clone);

                                    let Some(tool_call) = tool_call.as_mut() else {
                                        eprintln!(
                                            "Error getting pending tool call: {:?}",
                                            &id_clone
                                        );
                                        return;
                                    };

                                    eprintln!(
                                        "Setting tool call state to AskingForPermission: {:?}",
                                        &tool_call
                                    );
                                    tool_call.state = ToolCallState::AskingForPermission;
                                    eprintln!(
                                        "Tool call state set to AskingForPermission: {:?}",
                                        &tool_call
                                    );
                                });
                            }
                        }
                    });
                }

                AiTuiEvent::SelectPermission(permission) => {
                    // Okay, we have permssion information.
                    // If accepted, we can start executing.
                    // If denied, we can show an error message.
                    h.update(move |state| {
                        let tool_call = state
                            .pending_tool_calls
                            .iter()
                            .enumerate()
                            .find(|(_, call)| call.state == ToolCallState::AskingForPermission);

                        let Some((index, _)) = tool_call else {
                            return;
                        };

                        match permission {
                            PermissionResult::Allow => {
                                state.pending_tool_calls.remove(index);
                            }
                            PermissionResult::AlwaysAllowInDir => {
                                //
                            }
                            PermissionResult::AlwaysAllow => {
                                //
                            }
                            PermissionResult::Deny => {
                                let Some(call) = state.pending_tool_calls.remove(index) else {
                                    return;
                                };

                                state.add_tool_result(
                                    call.id,
                                    "Permission denied on the user's system".to_string(),
                                    true,
                                );
                            }
                        }
                    });
                }

                AiTuiEvent::CancelGeneration => {
                    h.update(|state| match state.mode {
                        crate::tui::state::AppMode::Generating => {
                            state.cancel_generation();
                        }
                        crate::tui::state::AppMode::Streaming => {
                            state.cancel_streaming();
                        }
                        _ => {}
                    });
                }

                AiTuiEvent::ExecuteCommand => {
                    let h2 = h.clone();
                    h.update(move |state| {
                        let cmd = state.current_command().map(|c| c.to_string());
                        if let Some(cmd) = cmd {
                            if state.is_current_command_dangerous() && !state.confirmation_pending {
                                state.confirmation_pending = true;
                            } else {
                                state.confirmation_pending = false;
                                state.exit_action = Some(ExitAction::Execute(cmd));
                                h2.exit();
                            }
                        }
                    });
                }

                AiTuiEvent::CancelConfirmation => {
                    h.update(move |state| {
                        state.confirmation_pending = false;
                    });
                }

                AiTuiEvent::InsertCommand => {
                    let h2 = h.clone();
                    h.update(move |state| {
                        let cmd = state.current_command().map(|c| c.to_string());
                        if let Some(cmd) = cmd {
                            state.confirmation_pending = false;
                            state.exit_action = Some(ExitAction::Insert(cmd));
                            h2.exit();
                        }
                    });
                }

                AiTuiEvent::Retry => {
                    let ep = ep.clone();
                    let tk = tk.clone();
                    let h2 = h.clone();
                    h.update(move |state| {
                        state.retry();
                        state.start_streaming();
                        let messages = state.events_to_messages();
                        let sid = state.session_id.clone();
                        let task = tokio::spawn(async move {
                            run_chat_stream(h2, ep, tk, sid, messages, send_cwd).await;
                        });
                        state.stream_abort = Some(task.abort_handle());
                    });
                }

                AiTuiEvent::Exit => {
                    let h2 = h.clone();
                    h.update(move |state| {
                        if let Some(abort) = state.stream_abort.take() {
                            abort.abort();
                        }
                        state.exit_action = Some(ExitAction::Cancel);
                        h2.exit();
                    });
                }
            }
        }
    });

    app.run_loop().await?;

    // Map exit action to return value
    let result = match app.state().exit_action {
        Some(ExitAction::Execute(ref cmd)) => Action::Execute(cmd.clone()),
        Some(ExitAction::Insert(ref cmd)) => Action::Insert(cmd.clone()),
        _ => Action::Cancel,
    };

    Ok(result)
}

// ───────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Action {
    Execute(String),
    Insert(String),
    Print(String),
    Cancel,
}

fn emit_shell_result(action: Action, output_for_hook: bool) {
    if output_for_hook {
        match action {
            Action::Execute(output) => eprintln!("__atuin_ai_execute__:{output}"),
            Action::Insert(output) => eprintln!("__atuin_ai_insert__:{output}"),
            Action::Print(output) => eprintln!("__atuin_ai_print__:{output}"),
            Action::Cancel => eprintln!("__atuin_ai_cancel__"),
        }
    } else {
        match action {
            Action::Execute(output) => eprintln!("{output}"),
            Action::Insert(output) => eprintln!("{output}"),
            Action::Print(output) => eprintln!("{output}"),
            Action::Cancel => eprintln!(),
        }
    }
}

fn wait_for_login_confirmation() -> Result<bool> {
    use crossterm::{
        event::{self, Event, KeyCode},
        terminal::{disable_raw_mode, enable_raw_mode},
    };

    enable_raw_mode().context("failed enabling raw mode for login prompt")?;
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }
    let _guard = Guard;

    loop {
        let ev = event::read().context("failed to read login confirmation key")?;
        if let Event::Key(key) = ev {
            match key.code {
                KeyCode::Enter => return Ok(true),
                KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }
}
