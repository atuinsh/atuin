use crate::tui::{
    App, AppEvent, AppMode, EventLoop, ExitAction, RenderContext, TerminalGuard,
    install_panic_hook, render_blocks,
};
use atuin_client::theme::ThemeManager;
use atuin_common::tls::ensure_crypto_provider;
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use eventsource_stream::Eventsource;
use eyre::{Context as _, Result, bail};
use futures::StreamExt;
use reqwest::Url;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct GenerateRequest {
    query: String,
    description: String,
    context: GenerateContext,
}

#[derive(Debug, Serialize)]
struct GenerateContext {
    os: String,
    shell: String,
    pwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenerateResponse {
    command: String,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(default)]
    dangerous: bool,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RefineRequest {
    events: Vec<RefineEvent>,
    #[serde(default)]
    capabilities: Vec<String>,
    context: GenerateContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RefineEvent {
    UserMessage {
        content: String,
    },
    Text {
        content: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

pub async fn run(
    initial_command: Option<String>,
    natural_language: bool,
    api_endpoint: Option<String>,
    api_token: Option<String>,
    keep_output: bool,
) -> Result<()> {
    // Install panic hook once at entry point to ensure terminal restoration
    install_panic_hook();

    let settings = atuin_client::settings::Settings::new()?;
    let endpoint = api_endpoint
        .as_deref()
        .unwrap_or(settings.hub_address.as_str());
    let token = if let Some(token) = api_token {
        token
    } else {
        ensure_hub_session(&settings, endpoint).await?
    };
    let action = run_inline_tui(
        endpoint.to_string(),
        token,
        if natural_language {
            None
        } else {
            initial_command
        },
        keep_output,
    )
    .await?;
    emit_shell_result(action.0, &action.1);

    Ok(())
}

async fn ensure_hub_session(
    settings: &atuin_client::settings::Settings,
    hub_address: &str,
) -> Result<String> {
    if let Some(token) = atuin_client::hub::get_session_token().await? {
        return Ok(token);
    }

    println!("Atuin AI requires authenticating with Atuin Hub.");
    println!("This is separate from your sync setup.");
    println!("Press enter to begin (or esc to cancel).");
    if !wait_for_login_confirmation()? {
        bail!("authentication canceled");
    }

    println!("Authenticating with Atuin Hub...");
    let mut auth_settings = settings.clone();
    auth_settings.hub_address = hub_address.to_string();
    let session = atuin_client::hub::HubAuthSession::start(&auth_settings).await?;
    println!("Open this URL to continue:");
    println!("{}", session.auth_url);

    let token = session
        .wait_for_completion(
            atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
            atuin_client::hub::DEFAULT_POLL_INTERVAL,
        )
        .await?;

    atuin_client::hub::save_session(&token).await?;
    Ok(token)
}

async fn generate_command(
    hub_address: &str,
    token: &str,
    description: &str,
) -> Result<GenerateResponse> {
    ensure_crypto_provider();
    let endpoint = hub_url(hub_address, "/api/cli/generate")?;
    let request = GenerateRequest {
        query: description.to_string(),
        description: description.to_string(),
        context: GenerateContext {
            os: detect_os(),
            shell: detect_shell(),
            pwd: std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
        },
    };

    let client = reqwest::Client::new();
    let response = client
        .post(endpoint)
        .bearer_auth(token)
        .json(&request)
        .send()
        .await
        .context("failed to call Atuin Hub generate endpoint")?;

    if response.status().is_success() {
        let generated = response
            .json::<GenerateResponse>()
            .await
            .context("failed to decode generate response")?;

        if generated.command.trim().is_empty() {
            bail!("Hub returned an empty command. Please try again with a more specific request.");
        }

        return Ok(generated);
    }

    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        atuin_client::hub::delete_session().await?;
        bail!("Hub session expired. Re-run to authenticate again.");
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("Hub request failed ({status}): {body}");
}

/// SSE event received from refine endpoint
#[derive(Debug, Clone)]
enum RefineStreamEvent {
    /// Text chunk to display
    TextChunk(String),
    /// Tool call event (need to echo back, may contain suggest_command)
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result from server-side execution
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Stream complete
    Done,
    /// Error from server
    Error(String),
}

fn create_refine_stream(
    hub_address: String,
    token: String,
    events: Vec<serde_json::Value>,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<RefineStreamEvent>> + Send>> {
    Box::pin(async_stream::stream! {
        ensure_crypto_provider();
        let endpoint = match hub_url(&hub_address, "/api/cli/refine") {
            Ok(url) => url,
            Err(e) => {
                yield Err(e);
                return;
            }
        };

        let request = RefineRequest {
            events: events.into_iter().map(|v| {
                serde_json::from_value(v).unwrap_or(RefineEvent::UserMessage {
                    content: "".to_string()
                })
            }).collect(),
            capabilities: vec![],
            context: GenerateContext {
                os: detect_os(),
                shell: detect_shell(),
                pwd: std::env::current_dir()
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned()),
            },
        };

        let client = reqwest::Client::new();
        let response = match client
            .post(endpoint.clone())
            .header("Accept", "text/event-stream")
            .bearer_auth(&token)
            .json(&request)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                yield Err(eyre::eyre!("Failed to send SSE request: {}", e));
                return;
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            yield Err(eyre::eyre!("SSE request failed ({}): {}", status, body));
            return;
        }

        let byte_stream = response.bytes_stream();
        let mut stream = byte_stream.eventsource();

        while let Some(event) = stream.next().await {
            match event {
                Ok(sse_event) => {
                    let event_type = sse_event.event.as_str();
                    let data = sse_event.data;

                    match event_type {
                        "text" => {
                            // Text chunk: {"content": "..."}
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                                && let Some(content) = json.get("content").and_then(|v| v.as_str())
                            {
                                yield Ok(RefineStreamEvent::TextChunk(content.to_string()));
                            }
                        }
                        "tool_call" => {
                            // Tool call: {"id": "...", "name": "...", "input": {...}}
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let id = json.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let name = json.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let input = json.get("input").cloned().unwrap_or(serde_json::json!({}));
                                yield Ok(RefineStreamEvent::ToolCall { id, name, input });
                            }
                        }
                        "tool_result" => {
                            // Tool result: {"tool_use_id": "...", "content": "...", "is_error": bool}
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let tool_use_id = json.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let content = json.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let is_error = json.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                                yield Ok(RefineStreamEvent::ToolResult { tool_use_id, content, is_error });
                            }
                        }
                        "done" => {
                            yield Ok(RefineStreamEvent::Done);
                            break;
                        }
                        "error" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error").to_string();
                                yield Ok(RefineStreamEvent::Error(message));
                            } else {
                                yield Ok(RefineStreamEvent::Error(data));
                            }
                            break;
                        }
                        "status" => {
                            // Status events are informational, ignore for now
                        }
                        _ => {
                            // Unknown event type, ignore
                        }
                    }
                }
                Err(e) => {
                    yield Err(eyre::eyre!("SSE error: {}", e));
                    break;
                }
            }
        }
    })
}

fn hub_url(base: &str, path: &str) -> Result<Url> {
    let base_with_slash = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    let stripped = path.strip_prefix('/').unwrap_or(path);
    Url::parse(&base_with_slash)?
        .join(stripped)
        .context("failed to build hub URL")
}

fn detect_os() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        "linux" => "linux".to_string(),
        _ => "linux".to_string(),
    }
}

fn detect_shell() -> String {
    if let Ok(shell) = std::env::var("ATUIN_SHELL")
        && !shell.trim().is_empty()
    {
        return shell;
    }

    let shell = std::env::var("SHELL")
        .ok()
        .and_then(|value| {
            std::path::Path::new(&value)
                .file_name()
                .map(std::ffi::OsStr::to_string_lossy)
                .map(std::borrow::Cow::into_owned)
        })
        .filter(|value| !value.trim().is_empty());

    match shell.as_deref() {
        Some("zsh") => "zsh".to_string(),
        Some("fish") => "fish".to_string(),
        Some("bash") => "bash".to_string(),
        _ => "bash".to_string(),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Execute,
    Insert,
    Cancel,
}

async fn run_inline_tui(
    endpoint: String,
    token: String,
    initial_prompt: Option<String>,
    keep_output: bool,
) -> Result<(Action, String)> {
    // Initialize terminal guard and app state
    let mut guard = TerminalGuard::new(keep_output)?;
    let mut app = App::new();
    if let Some(prompt) = initial_prompt {
        app.state.input = prompt;
    }

    // Load theme
    let settings = atuin_client::settings::Settings::new()?;
    let mut theme_manager = ThemeManager::new(None, None);
    let theme = theme_manager.load_theme(&settings.theme.name, None);

    // Initialize event loop
    let mut event_loop = EventLoop::new();

    // Track generation task
    let mut generation_task: Option<tokio::task::JoinHandle<Result<GenerateResponse>>> = None;
    let mut last_query = String::new();

    // Track refine stream and pending command from suggest_command tool
    let mut refine_stream: Option<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<RefineStreamEvent>> + Send>>,
    > = None;
    let mut pending_command: Option<String> = None;

    loop {
        // Render current state
        let anchor_col = guard.anchor_col();
        let ctx = RenderContext { theme, anchor_col };
        guard.terminal().draw(|frame| {
            render_blocks(frame, &app, &ctx);
        })?;

        // Get next event
        let event = event_loop.run().await?;

        // Handle event based on app mode
        match event {
            AppEvent::Key(key) => {
                app.handle_key(key);
            }
            AppEvent::Tick => {
                app.tick();

                // Poll refine stream if active
                if app.state.mode == AppMode::Streaming
                    && refine_stream.is_some()
                    && let Some(stream) = &mut refine_stream
                {
                    // Try to get next event without blocking
                    let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
                    match stream.as_mut().poll_next(&mut cx) {
                        std::task::Poll::Ready(Some(Ok(event))) => {
                            match event {
                                RefineStreamEvent::TextChunk(text) => {
                                    app.append_to_streaming_block(&text);
                                }
                                RefineStreamEvent::ToolCall { id, name, input } => {
                                    // Add to conversation events for echo
                                    app.add_tool_call_event(id, name.clone(), input.clone());
                                    // Check for suggest_command to extract the new command
                                    if name == "suggest_command" {
                                        let command = input
                                            .get("command")
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string());
                                        let conversation_only = input
                                            .get("conversation_only")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        if !conversation_only && let Some(cmd) = command {
                                            pending_command = Some(cmd);
                                        }
                                    }
                                }
                                RefineStreamEvent::ToolResult {
                                    tool_use_id,
                                    content,
                                    is_error,
                                } => {
                                    // Add to conversation events for echo
                                    app.add_tool_result_event(tool_use_id, content, is_error);
                                }
                                RefineStreamEvent::Done => {
                                    refine_stream = None;
                                    // Finalize with command if we got one
                                    if let Some(cmd) = pending_command.take() {
                                        app.finalize_streaming_with_command(cmd);
                                    } else {
                                        app.finalize_streaming();
                                    }
                                }
                                RefineStreamEvent::Error(msg) => {
                                    refine_stream = None;
                                    app.streaming_error(msg);
                                }
                            }
                        }
                        std::task::Poll::Ready(Some(Err(e))) => {
                            refine_stream = None;
                            app.streaming_error(e.to_string());
                        }
                        std::task::Poll::Ready(None) => {
                            refine_stream = None;
                            // Stream ended without done event
                            if let Some(cmd) = pending_command.take() {
                                app.finalize_streaming_with_command(cmd);
                            } else {
                                app.finalize_streaming();
                            }
                        }
                        std::task::Poll::Pending => {}
                    }
                }

                // Check if generation task finished
                if let Some(task) = &generation_task
                    && task.is_finished()
                {
                    let task = generation_task.take().unwrap();
                    match task.await.context("generate task join failed")? {
                        Ok(response) => {
                            app.generation_complete(
                                response.command,
                                response.explanation,
                                response.dangerous,
                                response.warnings,
                            );
                        }
                        Err(err) => {
                            app.generation_error(err.to_string());
                        }
                    }
                }
            }
            _ => {}
        }

        // Check exit condition
        if app.state.should_exit {
            break;
        }

        // Handle generation/refine trigger
        if app.state.mode == AppMode::Generating
            && generation_task.is_none()
            && refine_stream.is_none()
            && let Some(last_msg) = app
                .state
                .messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, crate::tui::MessageRole::User))
        {
            last_query = last_msg.content.clone();

            if app.state.is_refine_mode {
                // Conversation events already contain the user_message from start_generating()
                let events = app.state.conversation_events.clone();

                // Transition to streaming mode
                app.start_streaming_response();

                // Start the refine stream
                refine_stream = Some(create_refine_stream(
                    endpoint.clone(),
                    token.clone(),
                    events,
                ));
            } else {
                // Initial generation (existing logic)
                let endpoint_clone = endpoint.clone();
                let token_clone = token.clone();
                let query_clone = last_query.clone();
                generation_task = Some(tokio::spawn(async move {
                    generate_command(&endpoint_clone, &token_clone, &query_clone).await
                }));
            }
        }

        // Handle cancellation during generation
        if app.state.mode != AppMode::Generating
            && generation_task.is_some()
            && let Some(task) = generation_task.take()
        {
            task.abort();
        }

        // Handle streaming cancellation
        if app.state.mode != AppMode::Streaming && refine_stream.is_some() {
            refine_stream = None;
        }

        // Handle retry in Error mode (only for non-refine mode)
        if app.state.mode == AppMode::Generating
            && generation_task.is_none()
            && refine_stream.is_none()
            && !last_query.is_empty()
            && !app.state.is_refine_mode
        {
            // Retry with the same query (only for initial generation)
            let endpoint_clone = endpoint.clone();
            let token_clone = token.clone();
            let query_clone = last_query.clone();
            generation_task = Some(tokio::spawn(async move {
                generate_command(&endpoint_clone, &token_clone, &query_clone).await
            }));
        }
    }

    // Map exit action to return value
    let result = match app.state.exit_action {
        Some(ExitAction::Execute(cmd)) => (Action::Execute, cmd),
        Some(ExitAction::Insert(cmd)) => (Action::Insert, cmd),
        _ => (Action::Cancel, String::new()),
    };

    Ok(result)
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn emit_shell_result(action: Action, command: &str) {
    match action {
        Action::Execute => eprintln!("__atuin_ai_execute__:{command}"),
        Action::Insert => eprintln!("__atuin_ai_insert__:{command}"),
        Action::Cancel => eprintln!("__atuin_ai_cancel__"),
    }
}

fn wait_for_login_confirmation() -> Result<bool> {
    enable_raw_mode().context("failed enabling raw mode for login prompt")?;
    let _guard = RawModeGuard;

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
