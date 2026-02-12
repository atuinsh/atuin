use atuin_common::tls::ensure_crypto_provider;
use clap::{Parser, Subcommand};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use eyre::{Context as _, Result, bail};
use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::Level;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub mod init;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Custom API endpoint (e.g., https://openrouter.ai/api/v1)
    #[arg(long, global = true, env = "ATUIN_AI_API_ENDPOINT")]
    api_endpoint: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize shell integration
    Init,

    /// Complete current command line
    Complete {
        /// Current command line to complete
        #[arg(value_name = "COMMAND")]
        command: Option<String>,
    },

    /// Inline completion mode with small TUI overlay
    Inline {
        /// Current command line to complete
        #[arg(value_name = "COMMAND")]
        command: Option<String>,

        /// Start in natural language mode
        #[arg(long)]
        natural_language: bool,
    },
    /// Interactive mode with TUI
    Interactive,
}

pub async fn run() -> eyre::Result<()> {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    match cli.command {
        Commands::Init => init::run().await,
        Commands::Inline {
            command,
            natural_language,
        } => run_inline(command, natural_language, cli.api_endpoint).await,
        Commands::Complete { command } => run_inline(command, false, cli.api_endpoint).await,
        Commands::Interactive => Err(eyre::eyre!("interactive mode not implemented yet")),
    }
}

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

async fn run_inline(
    initial_command: Option<String>,
    natural_language: bool,
    api_endpoint: Option<String>,
) -> Result<()> {
    let settings = atuin_client::settings::Settings::new()?;
    let endpoint = api_endpoint
        .as_deref()
        .unwrap_or(settings.hub_address.as_str());
    let token = ensure_hub_session(&settings, endpoint).await?;
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
    let mut ui = InlineUi::new()?;
    let mut prompt = initial_prompt.unwrap_or_default();
    let mut spinner_idx = 0usize;

    loop {
        ui.render_prompt(&prompt)?;
        if !event::poll(Duration::from_millis(250)).context("failed to poll for input")? {
            continue;
        }

        let ev = event::read().context("failed to read terminal event")?;
        let Event::Key(key) = ev else {
            continue;
        };

        match key.code {
            KeyCode::Esc => return Ok((Action::Cancel, String::new())),
            KeyCode::Backspace => {
                prompt.pop();
            }
            KeyCode::Enter => {
                let query = prompt.trim().to_string();
                if query.is_empty() {
                    return Ok((Action::Cancel, String::new()));
                }

                let response = loop {
                    let endpoint_clone = endpoint.clone();
                    let token_clone = token.clone();
                    let query_clone = query.clone();
                    let task = tokio::spawn(async move {
                        generate_command(&endpoint_clone, &token_clone, &query_clone).await
                    });

                    let generated = loop {
                        if task.is_finished() {
                            break task.await.context("generate task join failed")?;
                        }

                        ui.render_generating(&prompt, spinner_idx)?;
                        spinner_idx = (spinner_idx + 1) % SPINNER_FRAMES.len();

                        if event::poll(Duration::from_millis(100))
                            .context("failed to poll while generating")?
                        {
                            let ev = event::read().context("failed reading generate event")?;
                            if let Event::Key(key) = ev
                                && key.code == KeyCode::Esc
                            {
                                task.abort();
                                return Ok((Action::Cancel, String::new()));
                            }
                        }
                    };

                    match generated {
                        Ok(value) => break value,
                        Err(err) => {
                            ui.render_error(&prompt, &err.to_string())?;
                            if !wait_for_retry_or_cancel()? {
                                return Ok((Action::Cancel, String::new()));
                            }
                        }
                    }
                };

                loop {
                    ui.render_review(&prompt, &response)?;
                    if !event::poll(Duration::from_millis(250))
                        .context("failed to poll in review")?
                    {
                        continue;
                    }

                    let ev = event::read().context("failed to read review event")?;
                    let Event::Key(key) = ev else {
                        continue;
                    };

                    match key.code {
                        KeyCode::Enter => return Ok((Action::Execute, response.command.clone())),
                        KeyCode::Tab => return Ok((Action::Insert, response.command.clone())),
                        KeyCode::Esc => return Ok((Action::Cancel, String::new())),
                        KeyCode::Char('e') => break,
                        _ => {}
                    }
                }
            }
            KeyCode::Char(c) => {
                prompt.push(c);
            }
            _ => {}
        }
    }
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

fn wait_for_retry_or_cancel() -> Result<bool> {
    loop {
        let ev = event::read().context("failed to read retry/cancel key")?;
        if let Event::Key(key) = ev {
            match key.code {
                KeyCode::Enter | KeyCode::Char('r') => return Ok(true),
                KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }
}

const SPINNER_FRAMES: [&str; 4] = ["/", "-", "\\", "|"];

struct InlineUi {
    terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    anchor_col: u16,
}

impl InlineUi {
    fn new() -> Result<Self> {
        let anchor_col = cursor::position().map(|(x, _)| x).unwrap_or(0);
        enable_raw_mode().context("failed to enable raw mode for inline UI")?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(16),
            },
        )
        .context("failed to initialize inline UI")?;
        Ok(Self {
            terminal,
            anchor_col,
        })
    }

    fn render_prompt(&mut self, prompt: &str) -> Result<()> {
        self.render(Screen::Prompt {
            prompt,
            footer: "[Enter]: Accept  [Esc]: Cancel",
        })
    }

    fn render_generating(&mut self, prompt: &str, spinner_idx: usize) -> Result<()> {
        self.render(Screen::Generating {
            prompt,
            footer: "[Esc]: Cancel",
            spinner_idx,
        })
    }

    fn render_review(&mut self, prompt: &str, response: &GenerateResponse) -> Result<()> {
        self.render(Screen::Review {
            prompt,
            response,
            footer: "[Enter]: Run  [Tab]: Insert  [e]: Edit  [Esc]: Cancel",
        })
    }

    fn render_error(&mut self, prompt: &str, err: &str) -> Result<()> {
        self.render(Screen::Error {
            prompt,
            err,
            footer: "[Enter]/[r]: Retry  [Esc]: Cancel",
        })
    }

    fn render(&mut self, screen: Screen<'_>) -> Result<()> {
        self.terminal
            .draw(|f| draw_screen(f, screen, self.anchor_col))
            .context("failed rendering inline UI")?;
        Ok(())
    }
}

impl Drop for InlineUi {
    fn drop(&mut self) {
        let _ = self.terminal.clear();
        let _ = disable_raw_mode();
    }
}

enum Screen<'a> {
    Prompt {
        prompt: &'a str,
        footer: &'a str,
    },
    Generating {
        prompt: &'a str,
        footer: &'a str,
        spinner_idx: usize,
    },
    Review {
        prompt: &'a str,
        response: &'a GenerateResponse,
        footer: &'a str,
    },
    Error {
        prompt: &'a str,
        err: &'a str,
        footer: &'a str,
    },
}

fn draw_screen(frame: &mut Frame, screen: Screen<'_>, anchor_col: u16) {
    let area = frame.area();
    let desired_width = 64u16.min(area.width.saturating_sub(2)).max(32);
    let desired_height = match screen {
        Screen::Review { .. } | Screen::Error { .. } => 12u16,
        Screen::Prompt { .. } | Screen::Generating { .. } => 8u16,
    }
    .min(area.height.max(1));

    let max_x = area.x + area.width.saturating_sub(desired_width);
    let preferred_x = area.x + anchor_col.saturating_sub(2);
    let card = Rect {
        x: preferred_x.min(max_x),
        y: area.y,
        width: desired_width,
        height: desired_height,
    };

    let footer = match &screen {
        Screen::Prompt { footer, .. }
        | Screen::Generating { footer, .. }
        | Screen::Review { footer, .. }
        | Screen::Error { footer, .. } => *footer,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Describe the command you'd like to generate:")
        .title_bottom(Line::from(footer).alignment(Alignment::Right));

    let content_area = block.inner(card);
    frame.render_widget(block, card);

    let (content, show_cursor, prompt_len) = match &screen {
        Screen::Prompt { prompt, .. } => {
            (format_prompt(prompt), true, prompt.chars().count() as u16)
        }
        Screen::Generating {
            prompt,
            spinner_idx,
            ..
        } => (
            format!(
                "{}\n\n{} Generating...",
                format_prompt(prompt),
                SPINNER_FRAMES[*spinner_idx]
            ),
            false,
            0,
        ),
        Screen::Review {
            prompt, response, ..
        } => {
            let separator = "─".repeat(usize::from(content_area.width).max(1));
            let mut text = format!(
                "{}\n\n{}\n\n$ {}",
                format_prompt(prompt),
                separator,
                response.command
            );
            if let Some(explanation) = &response.explanation {
                text.push_str("\n\n");
                text.push_str(explanation);
            }
            (text, false, 0)
        }
        Screen::Error { prompt, err, .. } => (
            format!("{}\n\nRequest failed:\n{}", format_prompt(prompt), err),
            false,
            0,
        ),
    };

    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, content_area);

    if show_cursor {
        let cursor_x = content_area.x.saturating_add(2 + prompt_len);
        let cursor_y = content_area.y;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn format_prompt(prompt: &str) -> String {
    if prompt.is_empty() {
        return ">".to_string();
    }
    format!("> {prompt}")
}

fn init_tracing(verbose: bool) {
    let level = if verbose { Level::DEBUG } else { Level::INFO };

    // Create ~/.atuin/logs directory if it doesn't exist
    let dirs = directories::UserDirs::new().expect("Could not find home directory");
    let log_dir = dirs.home_dir().join(".atuin").join("logs");
    std::fs::create_dir_all(&log_dir).expect("Could not create log directory");

    // Set up file appender with daily rotation
    let file_appender = tracing_appender::rolling::daily(&log_dir, "ai.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Create env filter
    let env_filter = EnvFilter::from_default_env().add_directive(
        format!("atuin_ai={}", level.as_str().to_lowercase())
            .parse()
            .unwrap(),
    );

    // Create file layer (always logs INFO and above to file)
    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_filter(EnvFilter::new("atuin_ai=info"));

    // Create console layer (only for verbose mode)
    let console_layer = if verbose {
        Some(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .with_target(false)
                .with_filter(env_filter),
        )
    } else {
        None
    };

    // Initialize subscriber with both layers
    let subscriber = tracing_subscriber::registry().with(file_layer);

    if let Some(console) = console_layer {
        subscriber.with(console).init();
    } else {
        subscriber.init();
    }

    // Keep the guard alive for the duration of the program
    std::mem::forget(_guard);
}
