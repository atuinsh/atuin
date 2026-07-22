use crate::context::{AppContext, ClientContext};
use crate::fsm::AgentFsm;
use crate::session::{LocalSessionService, SessionManager, SessionService};
use crate::tui::app::{AiApp, ExitOutcome, IoContext};
use crate::tui::state::ConversationEvent;
use atuin_client::database::{Database, Sqlite};
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
                emit_shell_result(ExitOutcome::Cancel, output_for_hook);
                return Ok(());
            }
            SetupChoice::Cancel => {
                emit_shell_result(ExitOutcome::Cancel, output_for_hook);
                return Ok(());
            }
        }
    }

    let endpoint = match api_endpoint.as_deref() {
        Some(raw) => reqwest::Url::parse(raw).context("invalid --api-endpoint URL")?,
        None => settings
            .ai
            .endpoint
            .clone()
            .unwrap_or_else(|| atuin_client::settings::DEFAULT_HUB_URL.clone()),
    };
    let endpoint_is_hub = settings.is_hub_ai_endpoint(&endpoint);
    let api_token = api_token.as_deref().or(settings.ai.api_token.as_deref());

    let (token, token_from_hub_session) = match api_token {
        Some(token) => (token.to_string(), false),
        None if endpoint_is_hub => (ensure_hub_session(settings).await?, true),
        // An OSS server may not require auth; hit it without a token rather
        // than forcing a login flow that doesn't apply.
        None => (String::new(), false),
    };

    let history_db_path = &settings.db_path;
    let history_db = Sqlite::new(history_db_path, settings.local_timeout)
        .await
        .context("failed to open history database for AI")?;

    // Support both legacy [ai] send_cwd and new [ai.opening] send_cwd
    let send_cwd =
        settings.ai.opening.send_cwd.unwrap_or(false) || settings.ai.send_cwd.unwrap_or(false);

    let last_command = if settings.ai.opening.send_last_command.unwrap_or(false) {
        history_db.last().await.ok().flatten()
    } else {
        None
    };

    let git_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| atuin_common::utils::in_git_repo(cwd.to_str()?));

    let ctx = AppContext {
        endpoint,
        token,
        endpoint_is_hub,
        token_from_hub_session,
        send_cwd,
        last_command,
        history_db: std::sync::Arc::new(history_db),
        git_root,
        capabilities: settings.ai.capabilities.clone(),
        daemon_enabled: settings.daemon.enabled,
        yolo: settings.ai.yolo,
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

    let hub_address = settings.hub_endpoint();
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

    let session = atuin_client::hub::HubAuthSession::start(&hub_address).await?;
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
        if let Err(e) = atuin_client::hub::link_account(&hub_address, &cli_token).await {
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
) -> Result<ExitOutcome> {
    let client_ctx = ClientContext::detect();

    // Open the session service and check for a resumable session
    let service = LocalSessionService::open(&settings.ai.db_path, settings.local_timeout)
        .await
        .context("failed to open AI session database")?;

    // Cached usage renders immediately; a background fetch (spawned below,
    // once the event channel exists) replaces it unless it's fresh. OSS
    // endpoints have no usage API, so both are skipped ("fresh" suppresses
    // the fetch).
    let (cached_usage, usage_is_fresh) = if !ctx.endpoint_is_hub {
        (None, true)
    } else {
        let usage_key = crate::usage::cache_key(&ctx.token);
        match service.get_cached_usage(&usage_key).await {
            Ok(Some(cached_snapshot)) => {
                let age =
                    time::OffsetDateTime::now_utc().unix_timestamp() - cached_snapshot.written_at;
                let fresh = age < crate::usage::REFRESH_AFTER.as_secs() as i64;
                (Some(cached_snapshot.snapshot), fresh)
            }
            Ok(None) => (None, false),
            Err(e) => {
                debug!("failed to read usage cache: {e}");
                (None, false)
            }
        }
    };

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

    // ─── Build FSM ───────────────────────────────────────────────
    let (session_mgr, mut fsm, file_tracker, edit_permissions) = if let Some(stored) = resumable {
        debug!(session_id = %stored.id, "resuming AI session");
        let (mgr, mut events, server_sid, last_event_ts, invocation_id) =
            SessionManager::resume(Box::new(service), &stored).await?;

        let has_api_content = events.iter().any(|e| e.is_api_content());

        if has_api_content {
            events.push(ConversationEvent::SystemContext {
                    content: "[Note: The user has started a new invocation of Atuin AI. Prior messages from this session are from an earlier invocation.]".to_string(),
                });
            let view_start = events.len();
            let last_time = last_event_ts.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));

            let ft = if let Ok(Some(json)) =
                mgr.get_metadata(crate::file_tracker::METADATA_KEY).await
                && let Ok(tracker) = crate::file_tracker::FileReadTracker::from_json(&json)
            {
                tracker
            } else {
                Default::default()
            };

            let ep = if let Ok(Some(json)) = mgr
                .get_metadata(crate::edit_permissions::METADATA_KEY)
                .await
                && let Ok(cache) = crate::edit_permissions::EditPermissionCache::from_json(&json)
            {
                cache
            } else {
                Default::default()
            };

            let caps = ctx.capabilities_as_strings();
            let fsm = AgentFsm::from_session(
                events,
                server_sid,
                caps,
                invocation_id,
                view_start,
                true,
                last_time,
            );
            (mgr, fsm, ft, ep)
        } else {
            debug!("resumable session has no API-visible content, starting fresh");
            let caps = ctx.capabilities_as_strings();
            let fsm = AgentFsm::new(caps, invocation_id);
            (mgr, fsm, Default::default(), Default::default())
        }
    } else {
        debug!("creating new AI session");
        let mgr =
            SessionManager::create_new(Box::new(service), cwd.as_deref(), git_root_str.as_deref());
        let invocation_id = uuid::Uuid::now_v7().to_string();
        let caps = ctx.capabilities_as_strings();
        let fsm = AgentFsm::new(caps, invocation_id);
        (mgr, fsm, Default::default(), Default::default())
    };

    // `ai.model` is read once at startup, so /model in another running
    // session doesn't retarget this one mid-conversation.
    fsm.ctx.model = settings
        .ai
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    // ─── Snapshot store ─────────────────────────────────────────
    let snapshot_dir = atuin_common::utils::data_dir()
        .join("ai")
        .join("snapshots")
        .join(session_mgr.session_id());
    let snapshot_store = crate::snapshots::SnapshotStore::open(snapshot_dir).ok();

    // ─── Discover skills ───────────────────────────────────────
    let project_root = ctx
        .git_root
        .clone()
        .or_else(|| std::env::current_dir().ok());
    let skill_registry = crate::skills::SkillRegistry::discover(project_root.as_deref()).await;

    // ─── Resume notice (frozen at startup) ──────────────────────
    let resume_notice = fsm.ctx.is_resumed.then(|| match fsm.ctx.last_event_time {
        Some(t) => {
            let human = chrono_humanize::HumanTime::from(t - chrono::Utc::now());
            format!(
                "  Continuing previous session (last active {human}) - type /new to start a new session"
            )
        }
        None => "  Continuing previous session - type /new to start a new session".to_string(),
    });

    // ─── Slash commands + skills ────────────────────────────────
    let mut slash_registry = crate::tui::slash::SlashCommandRegistry::default();
    let mut skill_names = std::collections::HashSet::new();
    for skill in skill_registry.all() {
        slash_registry.register(crate::tui::slash::SlashCommand::new(
            &skill.name,
            &skill.description,
        ));
        skill_names.insert(skill.name.clone());
    }

    // ─── IO context ─────────────────────────────────────────────
    // The persist worker owns the SessionManager and applies snapshots in
    // channel order.
    let (persist, persist_worker) = crate::tui::persist::spawn_persist_worker(session_mgr);
    let io = IoContext {
        app_ctx: ctx.clone(),
        client_ctx: client_ctx.clone(),
        persist,
        file_tracker,
        edit_permissions,
        snapshot_store,
        skill_registry,
        user_context_cache: Default::default(),
    };

    println!();

    let app = AiApp::new(
        fsm,
        io,
        resume_notice,
        slash_registry,
        skill_names,
        cached_usage,
        initial_prompt,
        !usage_is_fresh,
    );
    let options =
        eye_declare::RunOptions::default().keyboard(eye_declare::KeyboardProtocol::Enhanced);
    let outcome = eye_declare::driver_tokio::run_with(app, options)
        .await
        .context("failed running AI TUI")?;

    // The app (and with it the last persist sender) dropped when the run
    // loop returned; wait for the worker to drain its queue so the final
    // session snapshot is on disk before the process can exit.
    let _ = persist_worker.await;

    Ok(outcome)
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
        event::{self, Event, KeyCode, KeyEventKind},
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
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') if selected < options.len() - 1 => {
                    selected += 1;
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

fn emit_shell_result(outcome: ExitOutcome, output_for_hook: bool) {
    if output_for_hook {
        match outcome {
            ExitOutcome::Execute(output) => eprintln!("__atuin_ai_execute__:{output}"),
            ExitOutcome::Insert(output) => eprintln!("__atuin_ai_insert__:{output}"),
            ExitOutcome::Cancel => eprintln!("__atuin_ai_cancel__"),
        }
    } else {
        match outcome {
            ExitOutcome::Execute(output) | ExitOutcome::Insert(output) => {
                println!("{output}");
            }
            ExitOutcome::Cancel => {}
        }
    }
}
