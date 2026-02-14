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
use eyre::{Context as _, Result, bail};
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
}

pub async fn run(
    initial_command: Option<String>,
    natural_language: bool,
    api_endpoint: Option<String>,
    api_token: Option<String>,
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
) -> Result<(Action, String)> {
    // Initialize terminal guard and app state
    let mut guard = TerminalGuard::new()?;
    let mut app = App::new();
    if let Some(prompt) = initial_prompt {
        app.input = prompt;
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

                // Check if generation task finished
                if let Some(task) = &generation_task
                    && task.is_finished()
                {
                    let task = generation_task.take().unwrap();
                    match task.await.context("generate task join failed")? {
                        Ok(response) => {
                            app.generation_complete(response.command, response.explanation);
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
        if app.should_exit {
            break;
        }

        // Handle generation trigger (when mode changes to Generating)
        if app.mode == AppMode::Generating && generation_task.is_none() {
            // Get the query from the most recent input block
            if let Some(input_block) = app
                .blocks
                .iter()
                .rev()
                .find(|b| b.kind == crate::tui::BlockKind::Input)
            {
                last_query = input_block.content.clone();
                let endpoint_clone = endpoint.clone();
                let token_clone = token.clone();
                let query_clone = last_query.clone();
                generation_task = Some(tokio::spawn(async move {
                    generate_command(&endpoint_clone, &token_clone, &query_clone).await
                }));
            }
        }

        // Handle cancellation during generation
        if app.mode != AppMode::Generating
            && generation_task.is_some()
            && let Some(task) = generation_task.take()
        {
            task.abort();
        }

        // Handle retry in Error mode
        if app.mode == AppMode::Generating && generation_task.is_none() && !last_query.is_empty() {
            // Retry with the same query
            let endpoint_clone = endpoint.clone();
            let token_clone = token.clone();
            let query_clone = last_query.clone();
            generation_task = Some(tokio::spawn(async move {
                generate_command(&endpoint_clone, &token_clone, &query_clone).await
            }));
        }
    }

    // Map exit action to return value
    let result = match app.exit_action {
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
