use std::path::PathBuf;
use std::sync::mpsc;

use crate::context::{AppContext, ClientContext};
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

    let action = run_inline_tui(ctx, initial_command).await?;
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

    eprintln!("Atuin AI requires authenticating with Atuin Hub.");
    if will_sync {
        eprintln!(
            "Once logged in, your shell history will be synchronized via Atuin Hub if auto_sync is enabled or when manually syncing."
        );
    }
    eprintln!(
        "If you have an existing Atuin sync account, you can log in with your existing credentials."
    );
    eprintln!("Press enter to begin (or esc to cancel).");
    if !wait_for_login_confirmation()? {
        bail!("authentication canceled");
    }

    debug!("Starting Atuin Hub authentication...");
    eprintln!("Authenticating with Atuin Hub...");

    let session = atuin_client::hub::HubAuthSession::start(hub_address.as_ref()).await?;
    eprintln!("Open this URL to continue:");
    eprintln!("{}", session.auth_url);

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

async fn run_inline_tui(ctx: AppContext, initial_prompt: Option<String>) -> Result<Action> {
    let client_ctx = ClientContext::detect();

    let (tx, rx) = mpsc::channel::<AiTuiEvent>();

    let initial_state = Session::new(ctx.git_root.is_some());

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
    let h = handle.clone();
    tokio::task::spawn_blocking(move || {
        let tx = tx.clone();
        let client_ctx = client_ctx;
        while let Ok(event) = rx.recv() {
            dispatch::dispatch(&h, event, &tx, &ctx, &client_ctx);
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
            Action::Execute(output) | Action::Insert(output) | Action::Print(output) => {
                println!("{output}");
            }
            Action::Cancel => {}
        }
    }
}
