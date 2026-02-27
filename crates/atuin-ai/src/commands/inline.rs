use crate::commands::detect_shell;
use crate::tui::render::render;
use crate::tui::{
    App, AppEvent, AppMode, ConversationEvent, EventLoop, ExitAction, RenderContext, TerminalGuard,
    calculate_needed_height, install_panic_hook,
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
use std::io::Write;
use tracing::{debug, error, info, trace};

pub async fn run(
    initial_command: Option<String>,
    natural_language: bool,
    api_endpoint: Option<String>,
    api_token: Option<String>,
    keep_output: bool,
    debug_state_file: Option<String>,
    settings: &atuin_client::settings::Settings,
) -> Result<()> {
    // Install panic hook once at entry point to ensure terminal restoration
    install_panic_hook();

    // Token and endpoint priority:
    // 1. Command line arguments/environment variables
    // 2. Settings file
    // 3. Default
    let endpoint = api_endpoint.as_deref().unwrap_or(
        settings
            .ai
            .ai_endpoint
            .as_deref()
            .unwrap_or("https://hub.atuin.sh"),
    );
    let api_token = api_token.as_deref().or(settings.ai.ai_api_token.as_deref());

    let token = if let Some(token) = &api_token {
        token.to_string()
    } else {
        ensure_hub_session(settings, endpoint).await?
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
        debug_state_file,
        settings,
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
        debug!("Found Hub session, using existing token");
        return Ok(token);
    }

    info!("No Hub session found, prompting for authentication");

    println!("Atuin AI requires authenticating with Atuin Hub.");
    println!("This is separate from your sync setup.");
    println!("Press enter to begin (or esc to cancel).");
    if !wait_for_login_confirmation()? {
        bail!("authentication canceled");
    }

    debug!("Starting Atuin Hub authentication...");

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

    info!("Authentication complete, saving session token");

    atuin_client::hub::save_session(&token).await?;
    Ok(token)
}

/// SSE event received from chat endpoint
#[derive(Debug, Clone)]
enum ChatStreamEvent {
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
    /// Status update from server
    Status(String),
    /// Stream complete
    Done { session_id: String },
    /// Error from server
    Error(String),
}

fn create_chat_stream(
    hub_address: String,
    token: String,
    session_id: Option<String>,
    messages: Vec<serde_json::Value>,
    settings: &atuin_client::settings::Settings,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatStreamEvent>> + Send>> {
    let send_cwd = settings.ai.send_cwd;

    Box::pin(async_stream::stream! {
        ensure_crypto_provider();
        let endpoint = match hub_url(&hub_address, "/api/cli/chat") {
            Ok(url) => url,
            Err(e) => {
                yield Err(e);
                return;
            }
        };

        debug!("Sending SSE request to {endpoint}");

        // Build request body
        let mut request_body = serde_json::json!({
            "messages": messages,
            "context": {
                "os": detect_os(),
                "shell": detect_shell(),
                "pwd": if send_cwd { std::env::current_dir()
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned()) } else { None },
            }
        });

        // Include session_id only if present (not on first request)
        if let Some(ref sid) = session_id {
            trace!("Including session_id in request: {sid}");
            request_body["session_id"] = serde_json::json!(sid);
        }


        let client = reqwest::Client::new();
        let response = match client
            .post(endpoint.clone())
            .header("Accept", "text/event-stream")
            .bearer_auth(&token)
            .json(&request_body)
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
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // Clear saved session on auth error
            error!("SSE request failed with status: {status}, clearing session");
            let _ = atuin_client::hub::delete_session().await;
            yield Err(eyre::eyre!("Hub session expired. Re-run to authenticate again."));
            return;
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!("SSE request failed ({}): {}", status, body);
            yield Err(eyre::eyre!("SSE request failed ({}): {}", status, body));
            return;
        }

        let byte_stream = response.bytes_stream();
        let mut stream = byte_stream.eventsource();

        while let Some(event) = stream.next().await {
            match event {
                Ok(sse_event) => {
                    let event_type = sse_event.event.as_str();
                    let data = sse_event.data.clone();

                    debug!(event_type = %event_type, "SSE event received");

                    match event_type {
                        "text" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                                && let Some(content) = json.get("content").and_then(|v| v.as_str())
                            {
                                yield Ok(ChatStreamEvent::TextChunk(content.to_string()));
                            }
                        }
                        "tool_call" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let id = json.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let name = json.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let input = json.get("input").cloned().unwrap_or(serde_json::json!({}));
                                yield Ok(ChatStreamEvent::ToolCall { id, name, input });
                            }
                        }
                        "tool_result" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let tool_use_id = json.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let content = json.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let is_error = json.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                                yield Ok(ChatStreamEvent::ToolResult { tool_use_id, content, is_error });
                            }
                        }
                        "status" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                                && let Some(state) = json.get("state").and_then(|v| v.as_str())
                            {
                                yield Ok(ChatStreamEvent::Status(state.to_string()));
                            }
                        }
                        "done" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let session_id = json.get("session_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                yield Ok(ChatStreamEvent::Done { session_id });
                            } else {
                                yield Ok(ChatStreamEvent::Done { session_id: String::new() });
                            }
                            break;
                        }
                        "error" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error").to_string();
                                error!("SSE error: {}", message);
                                yield Ok(ChatStreamEvent::Error(message));
                            } else {
                                error!("SSE error: {}", data);
                                yield Ok(ChatStreamEvent::Error(data));
                            }
                            break;
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
        "windows" => "windows".to_string(),
        _ => "linux".to_string(),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Execute,
    Insert,
    Cancel,
}

/// Serialize AppState to JSON for debug logging
fn state_to_json(state: &crate::tui::AppState) -> serde_json::Value {
    let events: Vec<serde_json::Value> = state.events.iter().map(|e| e.to_json()).collect();

    let mode = match state.mode {
        AppMode::Input => "Input",
        AppMode::Generating => "Generating",
        AppMode::Streaming => "Streaming",
        AppMode::Review => "Review",
        AppMode::Error => "Error",
    };

    // Get input and cursor from textarea
    let input = state.input();
    let cursor = state.textarea.cursor();

    let mut json = serde_json::json!({
        "events": events,
        "mode": mode,
        "input": input,
        "cursor_row": cursor.0,
        "cursor_col": cursor.1,
        "spinner_frame": state.spinner_frame,
        "confirmation_pending": state.confirmation_pending,
    });

    // Add streaming fields if in streaming mode
    if !state.streaming_text.is_empty() {
        json["streaming_text"] = serde_json::json!(state.streaming_text);
    }
    if let Some(ref status) = state.streaming_status {
        json["streaming_status"] = serde_json::json!(status.display_text());
    }
    if let Some(ref err) = state.error {
        json["error"] = serde_json::json!(err);
    }

    json
}

/// Debug logger that writes state changes to a file
struct DebugStateLogger {
    file: std::fs::File,
    entry_count: usize,
    width: u16,
}

impl DebugStateLogger {
    fn new(path: &str) -> Result<Self> {
        let file = std::fs::File::create(path)
            .with_context(|| format!("Failed to create debug state file: {}", path))?;
        // Get terminal width, default to 80
        let (width, _) = crossterm::terminal::size().unwrap_or((80, 24));
        Ok(Self {
            file,
            entry_count: 0,
            width,
        })
    }

    fn log(&mut self, label: &str, state: &crate::tui::AppState) {
        use crate::tui::calculate_needed_height;

        self.entry_count += 1;
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);

        // Calculate the actual content height needed for this state
        let content_height = calculate_needed_height(state);

        let mut state_json = state_to_json(state);
        // Add dimensions for accurate replay
        state_json["width"] = serde_json::json!(self.width);
        state_json["height"] = serde_json::json!(content_height);

        let entry = serde_json::json!({
            "entry": self.entry_count,
            "label": label,
            "timestamp_ms": timestamp_ms,
            "state": state_json,
        });

        // Write as JSONL (one JSON object per line)
        if let Err(e) = writeln!(self.file, "{}", entry) {
            tracing::warn!("Failed to write debug state: {}", e);
        }
        let _ = self.file.flush();
    }
}

async fn run_inline_tui(
    endpoint: String,
    token: String,
    initial_prompt: Option<String>,
    keep_output: bool,
    debug_state_file: Option<String>,
    settings: &atuin_client::settings::Settings,
) -> Result<(Action, String)> {
    // Initialize terminal guard and app state
    let mut guard = TerminalGuard::new(keep_output)?;
    let mut app = App::new();
    if let Some(prompt) = initial_prompt {
        // Set initial text in textarea
        let mut textarea = tui_textarea::TextArea::from(prompt.lines());
        // Disable underline on cursor line
        textarea.set_cursor_line_style(ratatui::style::Style::default());
        // Enable word wrapping
        textarea.set_wrap_mode(tui_textarea::WrapMode::Word);
        // Move cursor to end
        textarea.move_cursor(tui_textarea::CursorMove::End);
        app.state.textarea = textarea;
    }

    // Initialize debug state logger if requested
    let mut debug_logger = debug_state_file
        .map(|path| DebugStateLogger::new(&path))
        .transpose()?;

    // Helper macro to log state changes
    macro_rules! log_state {
        ($label:expr) => {
            if let Some(ref mut logger) = debug_logger {
                logger.log($label, &app.state);
            }
        };
    }

    // Log initial state
    log_state!("init");

    // Load theme
    let mut theme_manager = ThemeManager::new(None, None);
    let theme = theme_manager.load_theme(&settings.theme.name, None);

    // Initialize event loop
    let mut event_loop = EventLoop::new();

    // Track chat stream
    let mut chat_stream: Option<
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatStreamEvent>> + Send>>,
    > = None;

    loop {
        // Ensure viewport is large enough for current content (capped at terminal height)
        let needed_height = calculate_needed_height(&app.state);
        let actual_height = guard.ensure_height(needed_height)?;

        // Render current state
        let anchor_col = guard.anchor_col();
        let ctx = RenderContext {
            theme,
            anchor_col,
            textarea: Some(&app.state.textarea),
            max_height: actual_height,
        };
        // Handle draw errors gracefully - cursor position reads can fail during resize
        if let Err(e) = guard.terminal().draw(|frame| {
            render(frame, &app.state, &ctx);
        }) {
            let err_msg = e.to_string();
            if err_msg.contains("cursor position") {
                // Cursor position read failed (common during terminal resize)
                // Skip this frame and continue - next frame will likely succeed
                tracing::debug!(
                    "Skipping frame due to cursor position read error: {}",
                    err_msg
                );
                continue;
            }
            return Err(e.into());
        }

        // Get next event
        let event = event_loop.run().await?;

        // Handle event based on app mode
        match event {
            AppEvent::Key(key) => {
                app.handle_key(key);
                log_state!("key");
            }
            AppEvent::Tick => {
                app.state.tick();

                // Poll chat stream if active - keep polling until done regardless of mode
                // (mode may change to Review before we receive the done event with session_id)
                if let Some(stream) = &mut chat_stream {
                    let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
                    match stream.as_mut().poll_next(&mut cx) {
                        std::task::Poll::Ready(Some(Ok(event))) => match event {
                            ChatStreamEvent::TextChunk(text) => {
                                trace!(text = %text, "Processing TextChunk");
                                app.state.append_streaming_text(&text);
                                log_state!("text_chunk");
                            }
                            ChatStreamEvent::ToolCall { id, name, input } => {
                                trace!(id = %id, name = %name, "Processing ToolCall");
                                app.state.add_tool_call(id, name, input);
                                log_state!("tool_call");
                            }
                            ChatStreamEvent::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                trace!(tool_use_id = %tool_use_id, "Processing ToolResult");
                                app.state.add_tool_result(tool_use_id, content, is_error);
                                log_state!("tool_result");
                            }
                            ChatStreamEvent::Status(status) => {
                                trace!(status = %status, "Processing Status");
                                app.state.update_streaming_status(&status);
                                log_state!("status");
                            }
                            ChatStreamEvent::Done { session_id } => {
                                trace!(session_id = %session_id, "Processing Done");
                                chat_stream = None;
                                if !session_id.is_empty() {
                                    app.state.store_session_id(session_id);
                                }
                                app.state.finalize_streaming();
                                log_state!("done");
                            }
                            ChatStreamEvent::Error(msg) => {
                                trace!(error = %msg, "Processing Error");
                                chat_stream = None;
                                app.state.streaming_error(msg);
                                log_state!("error");
                            }
                        },
                        std::task::Poll::Ready(Some(Err(e))) => {
                            chat_stream = None;
                            app.state.streaming_error(e.to_string());
                            log_state!("stream_error");
                        }
                        std::task::Poll::Ready(None) => {
                            chat_stream = None;
                            app.state.finalize_streaming();
                            log_state!("stream_end");
                        }
                        std::task::Poll::Pending => {}
                    }
                }
            }
            _ => {}
        }

        // Handle user cancellation (Esc during streaming) - drop the stream
        if app.state.was_interrupted && chat_stream.is_some() {
            debug!("User cancelled streaming, dropping chat stream");
            chat_stream = None;
            app.state.was_interrupted = false; // Reset the flag
        }

        // Check exit condition
        if app.state.should_exit {
            break;
        }

        // Handle generation trigger - unified path for all turns
        if app.state.mode == AppMode::Generating && chat_stream.is_none() {
            // Get the last user message from events
            let last_user_content = app.state.events.iter().rev().find_map(|e| {
                if let ConversationEvent::UserMessage { content } = e {
                    Some(content.clone())
                } else {
                    None
                }
            });

            if last_user_content.is_some() {
                // Build messages in Claude API format
                let messages = app.state.events_to_messages();

                // Transition to streaming mode
                app.state.start_streaming();
                log_state!("start_streaming");

                // Start the chat stream
                chat_stream = Some(create_chat_stream(
                    endpoint.clone(),
                    token.clone(),
                    app.state.session_id.clone(),
                    messages,
                    settings,
                ));
            }
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
