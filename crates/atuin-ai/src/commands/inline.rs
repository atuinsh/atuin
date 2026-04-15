use std::path::PathBuf;
use std::sync::mpsc;

use crate::context::{AppContext, ClientContext};
use crate::session::{LocalSessionService, SessionManager, SessionService};
use crate::tui::dispatch;
use crate::tui::events::AiTuiEvent;
use crate::tui::state::{ExitAction, Session};
use crate::tui::view::ai_view;
use atuin_client::database::{Database, Sqlite};
use eye_declare::{Application, CtrlCBehavior};
use eyre::{Context as _, Result, bail};
use tracing::{debug, info};

pub(crate) async fn run(
    initial_command: Option<String>,
    api_endpoint: Option<String>,
    api_token: Option<String>,
    settings: &atuin_client::settings::Settings,
    output_for_hook: bool,
) -> Result<()> {
    if settings.ai.enabled == Some(false) {
        return Ok(());
    }

    if settings.ai.enabled.is_none() {
        match prompt_ai_setup()? {
            SetupChoice::EnableAi => {
                set_ai_enabled(true).await?;
            }
            SetupChoice::DisableKeybind => {
                set_ai_enabled(false).await?;
                emit_shell_result(Action::Cancel, output_for_hook);
                return Ok(());
            }
            SetupChoice::Cancel => {
                emit_shell_result(Action::Cancel, output_for_hook);
                return Ok(());
            }
        }
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

    let history_db_path = PathBuf::from(settings.db_path.as_str());
    let history_db = Sqlite::new(history_db_path, settings.local_timeout)
        .await
        .context("failed to open history database for AI")?;

    // Support both legacy [ai] send_cwd and new [ai.opening] send_cwd
    let send_cwd =
        settings.ai.opening.send_cwd.unwrap_or(false) || settings.ai.send_cwd.unwrap_or(false);

    let last_command = if settings.ai.opening.send_last_command.unwrap_or(false) {
        history_db.last().await.ok().flatten().map(|h| h.command)
    } else {
        None
    };

    let git_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| atuin_common::utils::in_git_repo(cwd.to_str()?));

    let ctx = AppContext {
        endpoint: endpoint.to_string(),
        token,
        send_cwd,
        last_command,
        history_db: std::sync::Arc::new(history_db),
        git_root,
        capabilities: settings.ai.capabilities.clone(),
    };

    let action = run_inline_tui(ctx, initial_command, settings).await?;
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
        );
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

async fn run_inline_tui(
    ctx: AppContext,
    initial_prompt: Option<String>,
    settings: &atuin_client::settings::Settings,
) -> Result<Action> {
    let client_ctx = ClientContext::detect();

    // Open the session service and check for a resumable session
    let service = LocalSessionService::open(&settings.ai.db_path, settings.local_timeout)
        .await
        .context("failed to open AI session database")?;

    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let git_root_str = ctx
        .git_root
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());

    let session_window_mins = settings.ai.session_continue_minutes.max(0); // treat negative values as 0 to avoid confusion
    let max_age_secs: i64 = session_window_mins * 60;

    let resumable = service
        .find_resumable(cwd.as_deref(), git_root_str.as_deref(), max_age_secs)
        .await?;

    let (mut session_mgr, initial_state) = if let Some(stored) = resumable {
        debug!(session_id = %stored.id, "resuming AI session");
        let (mgr, events, server_sid, last_event_ts, invocation_id) =
            SessionManager::resume(Box::new(service), &stored).await?;

        // Only treat this as a meaningful resume if there are API-visible events
        // (not just OutOfBandOutput or SystemContext).
        let has_api_content = events.iter().any(|e| e.is_api_content());

        if has_api_content {
            let mut session = Session::new(ctx.git_root.is_some(), Some(invocation_id));
            session.conversation.events = events;
            session.conversation.session_id = server_sid;
            // Inject an invocation boundary so the LLM knows prior messages
            // are from an earlier interaction.
            session.conversation.events.push(
                crate::tui::state::ConversationEvent::SystemContext {
                    content: "[Note: The user has started a new invocation of Atuin AI. Prior messages from this session are from an earlier invocation.]".to_string(),
                },
            );
            session.view_start_index = session.conversation.events.len();
            session.is_resumed = true;
            session.last_event_time =
                last_event_ts.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));
            (mgr, session)
        } else {
            // No meaningful content — treat as a fresh session
            debug!("resumable session has no API-visible content, starting fresh");
            (
                mgr,
                Session::new(ctx.git_root.is_some(), Some(invocation_id)),
            )
        }
    } else {
        debug!("creating new AI session");
        let mgr =
            SessionManager::create_new(Box::new(service), cwd.as_deref(), git_root_str.as_deref());
        (mgr, Session::new(ctx.git_root.is_some(), None))
    };

    let (tx, rx) = mpsc::channel::<AiTuiEvent>();

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
        .with_context(tx.clone())
        .extra_newlines_at_exit(1)
        .build()?;

    // Event loop: receives AiTuiEvent from components, mutates state via Handle.
    // The dispatch thread processes events synchronously, including async persistence
    // via block_on. It signals exit via an AtomicBool rather than querying the handle
    // (which would hang if the TUI thread has already stopped processing).
    let h = handle.clone();
    let dispatch_handle = tokio::task::spawn_blocking(move || {
        let mut dctx = dispatch::DispatchContext {
            handle: &h,
            tx: &tx,
            app_ctx: &ctx,
            client_ctx: &client_ctx,
            session_mgr: &mut session_mgr,
            exiting: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        while let Ok(event) = rx.recv() {
            if !dispatch::dispatch(&mut dctx, event) {
                break;
            }
        }
    });

    let run_result = app.run_loop().await;

    // Wait for the dispatch thread to finish its final persist before the
    // tokio runtime tears down. This prevents panics from block_on calls
    // racing with runtime shutdown — including on the error path.
    let _ = dispatch_handle.await;

    run_result?;

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

enum SetupChoice {
    EnableAi,
    DisableKeybind,
    Cancel,
}

fn prompt_ai_setup() -> Result<SetupChoice> {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode},
        terminal,
    };

    let options = ["Enable Atuin AI", "Disable ? Keybind", "Cancel"];
    let mut selected: usize = 0;
    let mut stdout = std::io::stdout();

    // Print header before raw mode so newlines render correctly.
    // Use stdout because the shell hook swaps stdout/stderr — stdout goes
    // to the terminal in both hook and non-hook modes.
    println!();
    println!("  Atuin AI is not yet configured.");
    println!();

    terminal::enable_raw_mode().context("failed to enable raw mode")?;
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
        }
    }
    let _guard = Guard;

    crossterm::execute!(stdout, cursor::Hide)?;

    loop {
        render_setup_options(&mut stdout, &options, selected)?;

        let ev = event::read().context("failed to read key event")?;

        crossterm::execute!(stdout, cursor::MoveUp(options.len() as u16))?;

        if let Event::Key(key) = ev {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < options.len() - 1 {
                        selected += 1;
                    }
                }
                KeyCode::Enter => break,
                KeyCode::Esc => {
                    selected = 2;
                    break;
                }
                _ => {}
            }
        }
    }

    // Final render with selection visible
    render_setup_options(&mut stdout, &options, selected)?;
    crossterm::execute!(stdout, cursor::Show)?;

    Ok(match selected {
        0 => SetupChoice::EnableAi,
        1 => SetupChoice::DisableKeybind,
        _ => SetupChoice::Cancel,
    })
}

fn render_setup_options(
    w: &mut impl std::io::Write,
    options: &[&str],
    selected: usize,
) -> Result<()> {
    use crossterm::{
        style::Stylize,
        terminal::{Clear, ClearType},
    };

    for (i, option) in options.iter().enumerate() {
        if i == selected {
            write!(w, "\r  {}", format!("> {option}").bold().cyan())?;
        } else {
            write!(w, "\r    {option}")?;
        }
        crossterm::execute!(w, Clear(ClearType::UntilNewLine))?;
        write!(w, "\r\n")?;
    }
    w.flush()?;
    Ok(())
}

async fn set_ai_enabled(enabled: bool) -> Result<()> {
    let config_file = atuin_client::settings::Settings::get_config_path()?;
    let config_str = tokio::fs::read_to_string(&config_file).await?;
    let mut doc = config_str.parse::<toml_edit::DocumentMut>()?;

    if !doc.contains_key("ai") {
        doc["ai"] = toml_edit::table();
    }
    doc["ai"]["enabled"] = toml_edit::value(enabled);

    tokio::fs::write(&config_file, doc.to_string()).await?;

    if !enabled {
        println!(
            "Atuin AI keybind disabled. You can re-enable with `atuin config set ai.enabled true`.",
        );
        println!("Restart your shell for changes to take effect.");
        // Two printlns to ensure the message is visible above the shell prompt after program ends.
        println!();
        println!();
    }

    Ok(())
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

#[derive(Clone)]
enum Action {
    Execute(String),
    Insert(String),
    Cancel,
}

fn emit_shell_result(action: Action, output_for_hook: bool) {
    if output_for_hook {
        match action {
            Action::Execute(output) => eprintln!("__atuin_ai_execute__:{output}"),
            Action::Insert(output) => eprintln!("__atuin_ai_insert__:{output}"),
            Action::Cancel => eprintln!("__atuin_ai_cancel__"),
        }
    } else {
        match action {
            Action::Execute(output) | Action::Insert(output) => {
                println!("{output}");
            }
            Action::Cancel => {}
        }
    }
}
