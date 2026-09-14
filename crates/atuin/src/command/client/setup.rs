use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write, stdout};
use std::process::{Command, Stdio};
use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::{DEFAULT_SYNC_URL, Settings};
use clap::{Parser, ValueEnum};
use colored::Colorize;
use eyre::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};
use ratatui::{Terminal, TerminalOptions, Viewport};
use toml_edit::{DocumentMut, value};
use tracing::instrument;

#[cfg(feature = "sync")]
use super::account;
use super::import;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum CaptureChoice {
    ShellHistory,
    CommandOutput,
    AgentHistory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SearchChoice {
    #[cfg(feature = "ai")]
    AtuinAi,
    #[cfg(feature = "ai")]
    Mcp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SyncChoice {
    Hub,
    SelfHosted,
    None,
}

struct SetupChoices {
    capture: BTreeSet<CaptureChoice>,
    search: BTreeSet<SearchChoice>,
    sync: SyncChoice,
    sync_address: Option<String>,
    account: AccountChoice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccountChoice {
    Register,
    Login,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WizardStep {
    Capture,
    Search,
    Sync,
    SelfHostedUrl,
    Account,
}

#[derive(Parser, Debug, Default)]
pub struct Cmd {
    #[arg(long)]
    shell_history: Option<bool>,
    #[arg(long)]
    command_output: Option<bool>,
    #[arg(long)]
    agent_history: Option<bool>,
    #[cfg(feature = "ai")]
    #[arg(long)]
    atuin_ai: Option<bool>,
    #[cfg(feature = "ai")]
    #[arg(long)]
    mcp: Option<bool>,
    #[arg(long, value_enum)]
    sync: Option<SyncArg>,
    #[arg(long)]
    sync_address: Option<String>,
    #[arg(long, value_enum)]
    account: Option<AccountArg>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SyncArg {
    Hub,
    SelfHosted,
    None,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AccountArg {
    Register,
    Login,
    Skip,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, _settings: &Settings, db: &Sqlite, store: SqliteStore) -> Result<()> {
        if !io::stdin().is_terminal() && !self.has_overrides() {
            println!("Non-interactive environment detected — applying local defaults.");
            println!(
                "Sync needs an account, so run 'atuin setup' in a terminal later to enable it."
            );

            let capture = [CaptureChoice::ShellHistory, CaptureChoice::AgentHistory]
                .into_iter()
                .collect::<BTreeSet<_>>();
            let search = default_search_choices();

            let config_file = Settings::get_config_path()?;
            let config_str = tokio::fs::read_to_string(&config_file).await?;
            let mut doc = config_str.parse::<DocumentMut>()?;
            apply_config(&mut doc, &capture, &search, SyncChoice::None)?;
            tokio::fs::write(&config_file, doc.to_string()).await?;

            install_shell_init()?;
            install_detected_agent_hooks();
            println!("{check} Atuin is ready for new shells", check = "✓".bold().bright_green());
            return Ok(());
        }

        let SetupChoices {
            capture,
            search,
            sync,
            sync_address,
            account,
        } = if self.has_overrides() {
            self.into_choices()?
        } else {
            run_wizard().await?
        };

        if sync == SyncChoice::SelfHosted {
            let address = sync_address.as_deref().unwrap_or_default();
            validate_self_hosted_server(address).await?;
        }

        let config_file = Settings::get_config_path()?;
        let config_str = tokio::fs::read_to_string(&config_file).await?;
        let mut doc = config_str.parse::<DocumentMut>()?;

        let sync_enabled = if account == AccountChoice::Skip {
            SyncChoice::None
        } else {
            sync
        };
        apply_config(&mut doc, &capture, &search, sync_enabled)?;

        match sync {
            SyncChoice::Hub => doc["sync_address"] = value(DEFAULT_SYNC_URL.as_str()),
            SyncChoice::SelfHosted => {
                if let Some(address) = sync_address {
                    doc["sync_address"] = value(address);
                }
            }
            SyncChoice::None => {}
        }

        tokio::fs::write(&config_file, doc.to_string()).await?;
        println!("{check} Settings updated", check = "✓".bold().bright_green());

        install_shell_init()?;

        if capture.contains(&CaptureChoice::ShellHistory) {
            if has_obvious_history_file() {
                println!("\nImporting existing shell history...");
                if let Err(err) = import::Cmd::Auto.run(db).await {
                    eprintln!(
                        "History import failed: {err}. You can retry with 'atuin import auto'."
                    );
                }
            } else {
                println!("\nNo existing shell history found. New commands will be captured.");
            }
        }

        if capture.contains(&CaptureChoice::AgentHistory) {
            install_detected_agent_hooks();
        }

        #[cfg(feature = "ai")]
        if search.contains(&SearchChoice::Mcp) {
            println!(
                "\nMCP enabled: point your AI tool at `{}` when it asks for the server command.",
                "atuin mcp".bold()
            );
        }

        match sync {
            SyncChoice::Hub | SyncChoice::SelfHosted => {
                let settings = Settings::new()?;
                register_or_login(&settings, &store, account).await?;
            }
            SyncChoice::None => {
                println!("\nSkipping sync. Run 'atuin setup' later if you change your mind.");
            }
        }

        println!(
            "\n{check} All set. Restart your shell to start using Atuin.",
            check = "✓".bold().bright_green()
        );

        Ok(())
    }

    fn has_overrides(&self) -> bool {
        self.shell_history.is_some()
            || self.command_output.is_some()
            || self.agent_history.is_some()
            || self.sync.is_some()
            || self.sync_address.is_some()
            || self.account.is_some()
            || self.has_ai_overrides()
    }

    #[cfg(feature = "ai")]
    fn has_ai_overrides(&self) -> bool {
        self.atuin_ai.is_some() || self.mcp.is_some()
    }

    #[cfg(not(feature = "ai"))]
    fn has_ai_overrides(&self) -> bool {
        false
    }

    fn into_choices(self) -> Result<SetupChoices> {
        let mut capture = [CaptureChoice::ShellHistory, CaptureChoice::AgentHistory]
            .into_iter()
            .collect::<BTreeSet<_>>();
        set_choice(&mut capture, CaptureChoice::ShellHistory, self.shell_history);
        set_choice(&mut capture, CaptureChoice::CommandOutput, self.command_output);
        set_choice(&mut capture, CaptureChoice::AgentHistory, self.agent_history);

        let mut search = default_search_choices();
        #[cfg(feature = "ai")]
        {
            set_choice(&mut search, SearchChoice::AtuinAi, self.atuin_ai);
            set_choice(&mut search, SearchChoice::Mcp, self.mcp);
        }

        let sync = self.sync.map_or(SyncChoice::None, Into::into);
        if sync == SyncChoice::SelfHosted && self.sync_address.is_none() {
            eyre::bail!("--sync-address is required with --sync self-hosted");
        }

        Ok(SetupChoices {
            capture,
            search,
            sync,
            sync_address: self.sync_address,
            account: self.account.map_or(AccountChoice::Skip, Into::into),
        })
    }
}

fn set_choice<T: Ord>(choices: &mut BTreeSet<T>, choice: T, enabled: Option<bool>) {
    match enabled {
        Some(true) => {
            choices.insert(choice);
        }
        Some(false) => {
            choices.remove(&choice);
        }
        None => {}
    }
}

impl From<SyncArg> for SyncChoice {
    fn from(arg: SyncArg) -> Self {
        match arg {
            SyncArg::Hub => Self::Hub,
            SyncArg::SelfHosted => Self::SelfHosted,
            SyncArg::None => Self::None,
        }
    }
}

impl From<AccountArg> for AccountChoice {
    fn from(arg: AccountArg) -> Self {
        match arg {
            AccountArg::Register => Self::Register,
            AccountArg::Login => Self::Login,
            AccountArg::Skip => Self::Skip,
        }
    }
}

fn default_search_choices() -> BTreeSet<SearchChoice> {
    let mut search = BTreeSet::new();
    #[cfg(feature = "ai")]
    {
        search.insert(SearchChoice::AtuinAi);
        search.insert(SearchChoice::Mcp);
    }
    search
}

async fn run_wizard() -> Result<SetupChoices> {
    terminal::enable_raw_mode()?;

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = match Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(12),
        },
    ) {
        Ok(terminal) => terminal,
        Err(_) => {
            let _ = terminal::disable_raw_mode();
            return plain_wizard().await;
        }
    };

    let result = run_wizard_inner(&mut terminal).await;

    let _ = terminal.show_cursor();
    let _ = terminal::disable_raw_mode();

    result
}

async fn plain_wizard() -> Result<SetupChoices> {
    println!("Atuin setup\n");

    let capture = prompt_multi_plain(
        "What should Atuin capture?",
        &[
            (CaptureChoice::ShellHistory, "Shell history", true),
            (CaptureChoice::CommandOutput, "Command output", false),
            (CaptureChoice::AgentHistory, "Agent history", true),
        ],
    )?;

    let search = prompt_multi_plain("How do you want to search it?", &search_items_plain())?;
    let sync = prompt_sync_plain()?;
    let sync_address = if sync == SyncChoice::SelfHosted {
        loop {
            print!("Self-hosted sync URL: ");
            io::stdout().flush().ok();
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let address = input.trim().to_string();
            match validate_self_hosted_server(&address).await {
                Ok(()) => break Some(address),
                Err(err) => println!("{err}"),
            }
        }
    } else {
        None
    };
    let account = if sync == SyncChoice::None {
        AccountChoice::Skip
    } else {
        prompt_account_plain()?
    };

    Ok(SetupChoices {
        capture,
        search,
        sync,
        sync_address,
        account,
    })
}

fn prompt_multi_plain<T: Copy + Ord>(
    title: &str,
    options: &[(T, &str, bool)],
) -> Result<BTreeSet<T>> {
    println!("{title}");
    for (idx, (_, label, default)) in options.iter().enumerate() {
        let mark = if *default {
            "x"
        } else {
            " "
        };
        println!("  [{mark}] {}. {label}", idx + 1);
    }
    print!("Enter for defaults, or numbers to toggle: ");
    io::stdout().flush().ok();

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let mut selected = options
        .iter()
        .filter_map(|(choice, _, default)| default.then_some(*choice))
        .collect::<BTreeSet<_>>();
    for part in input.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()) {
        let index: usize = part.parse()?;
        let Some((choice, _, _)) = options.get(index.saturating_sub(1)) else {
            eyre::bail!("unknown option: {part}");
        };
        if !selected.insert(*choice) {
            selected.remove(choice);
        }
    }
    println!();
    Ok(selected)
}

fn search_items_plain() -> Vec<(SearchChoice, &'static str, bool)> {
    let mut items = Vec::new();
    #[cfg(feature = "ai")]
    {
        items.push((SearchChoice::AtuinAi, "Atuin AI", true));
        items.push((SearchChoice::Mcp, "MCP", true));
    }
    items
}

fn prompt_sync_plain() -> Result<SyncChoice> {
    println!("Do you want to sync?");
    println!("  1. Hub");
    println!("  2. Self hosted");
    println!("  3. No sync");
    print!("Choose 1/2/3, or Enter for Hub: ");
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(match input.trim() {
        "" | "1" => SyncChoice::Hub,
        "2" => SyncChoice::SelfHosted,
        "3" => SyncChoice::None,
        other => eyre::bail!("unknown sync option: {other}"),
    })
}

fn prompt_account_plain() -> Result<AccountChoice> {
    println!("Register or log in");
    println!("  1. Register");
    println!("  2. Log in");
    println!("  3. Skip");
    print!("Choose 1/2/3, or Enter to register: ");
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(match input.trim() {
        "" | "1" => AccountChoice::Register,
        "2" => AccountChoice::Login,
        "3" => AccountChoice::Skip,
        other => eyre::bail!("unknown account option: {other}"),
    })
}

async fn run_wizard_inner(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> Result<SetupChoices> {
    let mut state = WizardState::new();

    loop {
        terminal.draw(|frame| render_wizard(frame, &state))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if state.step == WizardStep::SelfHostedUrl {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    eyre::bail!("setup cancelled")
                }
                KeyCode::Esc => state.go_back(),
                KeyCode::Left if state.sync_address.is_empty() => state.go_back(),
                KeyCode::Backspace if state.sync_address.is_empty() => state.go_back(),
                KeyCode::Backspace => {
                    state.sync_address.pop();
                    state.url_error = None;
                }
                KeyCode::Char(c) => {
                    state.sync_address.push(c);
                    state.url_error = None;
                }
                KeyCode::Enter => {
                    let address = state.sync_address.trim().to_string();
                    state.url_checking = true;
                    state.url_error = None;
                    terminal.draw(|frame| render_wizard(frame, &state))?;
                    match validate_self_hosted_server(&address).await {
                        Ok(()) => {
                            state.url_checking = false;
                            if state.advance() {
                                return Ok(state.into_choices());
                            }
                        }
                        Err(err) => {
                            state.url_checking = false;
                            state.url_error = Some(err.to_string());
                        }
                    }
                }
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                eyre::bail!("setup cancelled")
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                state.go_back();
            }
            KeyCode::Up | KeyCode::Char('k') => state.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => state.move_cursor(1),
            KeyCode::Char(' ') => state.toggle_current(),
            KeyCode::Enter => {
                if state.advance() {
                    return Ok(state.into_choices());
                }
            }
            _ => {}
        }
    }
}

struct WizardState {
    step: WizardStep,
    cursor: usize,
    capture: BTreeSet<CaptureChoice>,
    search: BTreeSet<SearchChoice>,
    sync: SyncChoice,
    sync_address: String,
    url_error: Option<String>,
    url_checking: bool,
    account: AccountChoice,
}

impl WizardState {
    fn new() -> Self {
        Self {
            step: WizardStep::Capture,
            cursor: 0,
            capture: [CaptureChoice::ShellHistory, CaptureChoice::AgentHistory]
                .into_iter()
                .collect(),
            search: default_search_choices(),
            sync: SyncChoice::Hub,
            sync_address: String::new(),
            url_error: None,
            url_checking: false,
            account: AccountChoice::Register,
        }
    }

    fn len(&self) -> usize {
        match self.step {
            WizardStep::Capture => capture_items().len(),
            WizardStep::Search => search_items().len(),
            WizardStep::Sync => sync_items().len(),
            WizardStep::SelfHostedUrl => 0,
            WizardStep::Account => account_items().len(),
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.len();
        if len == 0 {
            return;
        }
        self.cursor = if delta < 0 {
            self.cursor.checked_sub(1).unwrap_or(len - 1)
        } else {
            (self.cursor + 1) % len
        };
    }

    fn toggle_current(&mut self) {
        match self.step {
            WizardStep::Capture => {
                let choice = capture_items()[self.cursor].0;
                if !self.capture.insert(choice) {
                    self.capture.remove(&choice);
                }
            }
            WizardStep::Search => {
                let choice = search_items()[self.cursor].0;
                if !self.search.insert(choice) {
                    self.search.remove(&choice);
                }
            }
            WizardStep::Sync => self.sync = sync_items()[self.cursor].0,
            WizardStep::Account => self.account = account_items()[self.cursor].0,
            WizardStep::SelfHostedUrl => {}
        }
    }

    fn go_back(&mut self) {
        self.step = match self.step {
            WizardStep::Capture => WizardStep::Capture,
            WizardStep::Search => WizardStep::Capture,
            WizardStep::Sync => {
                if search_items().is_empty() {
                    WizardStep::Capture
                } else {
                    WizardStep::Search
                }
            }
            WizardStep::SelfHostedUrl => WizardStep::Sync,
            WizardStep::Account => {
                if self.sync == SyncChoice::SelfHosted {
                    WizardStep::SelfHostedUrl
                } else {
                    WizardStep::Sync
                }
            }
        };
        self.cursor = 0;
    }

    fn advance(&mut self) -> bool {
        match self.step {
            WizardStep::Capture => {
                self.step = if search_items().is_empty() {
                    WizardStep::Sync
                } else {
                    WizardStep::Search
                };
            }
            WizardStep::Search => self.step = WizardStep::Sync,
            WizardStep::Sync => {
                self.sync = sync_items()[self.cursor].0;
                match self.sync {
                    SyncChoice::Hub => self.step = WizardStep::Account,
                    SyncChoice::SelfHosted => self.step = WizardStep::SelfHostedUrl,
                    SyncChoice::None => return true,
                }
            }
            WizardStep::SelfHostedUrl => {
                if url::Url::parse(self.sync_address.trim())
                    .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
                {
                    self.step = WizardStep::Account;
                }
                return false;
            }
            WizardStep::Account => {
                self.account = account_items()[self.cursor].0;
                return true;
            }
        }
        self.cursor = 0;
        false
    }

    fn into_choices(self) -> SetupChoices {
        SetupChoices {
            capture: self.capture,
            search: self.search,
            sync: self.sync,
            sync_address: (self.sync == SyncChoice::SelfHosted)
                .then(|| self.sync_address.trim().to_string()),
            account: self.account,
        }
    }
}

fn render_wizard(frame: &mut ratatui::Frame<'_>, state: &WizardState) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(frame.area());

    frame.render_widget(
        Paragraph::new("🐢 Atuin setup").style(Style::default().add_modifier(Modifier::BOLD)),
        chunks[0],
    );

    let title_help = match state.step {
        WizardStep::Capture => vec![
            Line::from("What should Atuin capture?")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            subtitle("Choose what to save."),
        ],
        WizardStep::Search => vec![
            Line::from("How do you want to search it?")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            subtitle(search_description()),
        ],
        WizardStep::Sync => vec![
            Line::from("Sync your data?").style(Style::default().add_modifier(Modifier::BOLD)),
            subtitle("Use history across machines. Keep a backup. Set up new devices faster."),
        ],
        WizardStep::SelfHostedUrl => vec![
            Line::from("Self-hosted sync URL").style(Style::default().add_modifier(Modifier::BOLD)),
            subtitle("Enter the URL for your sync server."),
        ],
        WizardStep::Account => vec![
            Line::from("Register or log in").style(Style::default().add_modifier(Modifier::BOLD)),
            subtitle("Needed before sync can run."),
        ],
    };
    frame.render_widget(Paragraph::new(title_help), chunks[1]);

    let items = match state.step {
        WizardStep::Capture => capture_items()
            .into_iter()
            .enumerate()
            .map(|(i, (choice, label, note))| {
                checkbox_item(i, state.cursor, state.capture.contains(&choice), label, note)
            })
            .collect::<Vec<_>>(),
        WizardStep::Search => search_items()
            .into_iter()
            .enumerate()
            .map(|(i, (choice, label, note))| {
                checkbox_item(i, state.cursor, state.search.contains(&choice), label, note)
            })
            .collect::<Vec<_>>(),
        WizardStep::Sync => sync_items()
            .into_iter()
            .enumerate()
            .map(|(i, (_choice, label, note))| select_item(i, state.cursor, label, note))
            .collect::<Vec<_>>(),
        WizardStep::Account => account_items()
            .into_iter()
            .enumerate()
            .map(|(i, (_choice, label, note))| select_item(i, state.cursor, label, note))
            .collect::<Vec<_>>(),
        WizardStep::SelfHostedUrl => {
            let valid = url::Url::parse(state.sync_address.trim())
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
            let note = if state.url_checking {
                "checking...".to_string()
            } else if let Some(err) = &state.url_error {
                err.clone()
            } else if state.sync_address.is_empty() || valid {
                String::new()
            } else {
                "Enter a valid http(s) URL".to_string()
            };
            vec![ListItem::new(Line::from(vec![
                Span::styled(
                    "  URL ",
                    Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                ),
                Span::raw(&state.sync_address),
                Span::styled(
                    "_",
                    Style::default().fg(Color::Blue).add_modifier(Modifier::SLOW_BLINK),
                ),
                Span::styled(format!(" {note}"), Style::default().fg(Color::Red)),
            ]))]
        }
    };
    frame.render_widget(List::new(items), chunks[2]);

    let footer = match state.step {
        WizardStep::Sync | WizardStep::Account => {
            "↑/↓ move · enter select · esc/back go back · ctrl-c exit"
        }
        WizardStep::SelfHostedUrl => "type URL · enter validate · esc/back go back · ctrl-c exit",
        _ => "↑/↓ move · space toggle · enter continue · esc/back go back · ctrl-c exit",
    };
    frame.render_widget(
        Paragraph::new(footer)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        chunks[3],
    );
}

fn subtitle(text: &'static str) -> Line<'static> {
    Line::from(text).style(Style::default().fg(Color::Blue).add_modifier(Modifier::ITALIC))
}

fn checkbox_item<'a>(
    idx: usize,
    cursor: usize,
    checked: bool,
    label: &'a str,
    note: &'a str,
) -> ListItem<'a> {
    let mark = if checked {
        "[x]"
    } else {
        "[ ]"
    };
    option_item(idx, cursor, mark, label, note)
}

fn select_item<'a>(idx: usize, cursor: usize, label: &'a str, note: &'a str) -> ListItem<'a> {
    let marker = if idx == cursor {
        "›"
    } else {
        " "
    };
    let style = if idx == cursor {
        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let mut spans = vec![Span::styled(format!("  {marker} {label}"), style)];
    if !note.is_empty() {
        spans.push(Span::styled(format!(" — {note}"), Style::default().fg(Color::DarkGray)));
    }
    ListItem::new(Line::from(spans))
}

fn option_item<'a>(
    idx: usize,
    cursor: usize,
    mark: &str,
    label: &'a str,
    note: &'a str,
) -> ListItem<'a> {
    let style = if idx == cursor {
        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let mut spans = vec![Span::styled(format!("  {mark} {label}"), style)];
    if !note.is_empty() {
        spans.push(Span::styled(format!(" — {note}"), Style::default().fg(Color::DarkGray)));
    }
    ListItem::new(Line::from(spans))
}

fn capture_items() -> Vec<(CaptureChoice, &'static str, &'static str)> {
    vec![
        (CaptureChoice::ShellHistory, "Shell history", ""),
        (CaptureChoice::AgentHistory, "Agent shell history", ""),
        (CaptureChoice::CommandOutput, "Command output", "local only; sync not yet supported"),
    ]
}

fn search_items() -> Vec<(SearchChoice, &'static str, &'static str)> {
    let mut items = Vec::new();
    #[cfg(feature = "ai")]
    {
        items.push((SearchChoice::AtuinAi, "Atuin AI", "ask for command help"));
        items.push((SearchChoice::Mcp, "MCP", "let tools search history"));
    }
    items
}

fn sync_items() -> Vec<(SyncChoice, &'static str, &'static str)> {
    vec![
        (SyncChoice::Hub, "Hub", ""),
        (SyncChoice::SelfHosted, "Self hosted", ""),
        (SyncChoice::None, "No sync", ""),
    ]
}

fn account_items() -> Vec<(AccountChoice, &'static str, &'static str)> {
    vec![
        (AccountChoice::Register, "Register", "new account"),
        (AccountChoice::Login, "Log in", "existing account"),
        (AccountChoice::Skip, "Skip", "do this later"),
    ]
}

fn search_description() -> &'static str {
    #[cfg(feature = "ai")]
    {
        "Shell keybindings are enabled. Choose any extra ways to search."
    }

    #[cfg(not(feature = "ai"))]
    {
        "Shell keybindings are enabled."
    }
}

async fn validate_self_hosted_server(address: &str) -> Result<()> {
    let base = url::Url::parse(address).map_err(|_| eyre::eyre!("Enter a valid http(s) URL"))?;
    if !matches!(base.scheme(), "http" | "https") {
        eyre::bail!("Enter a valid http(s) URL");
    }

    let health = base.join("healthz")?;
    let response = reqwest::Client::new()
        .get(health)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| eyre::eyre!("Could not reach an Atuin server"))?;

    if !response.status().is_success() {
        eyre::bail!("Server did not pass Atuin health check");
    }

    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| eyre::eyre!("Health check was not an Atuin response"))?;
    if body.get("status").and_then(serde_json::Value::as_str) != Some("healthy") {
        eyre::bail!("Health check was not an Atuin response");
    }

    Ok(())
}

fn apply_config(
    doc: &mut DocumentMut,
    capture: &BTreeSet<CaptureChoice>,
    search: &BTreeSet<SearchChoice>,
    sync: SyncChoice,
) -> Result<()> {
    let capture_output = capture.contains(&CaptureChoice::CommandOutput);
    if !doc.contains_key("output") {
        doc["output"] = toml_edit::table();
    }
    doc["output"]["enabled"] = value(capture_output);

    if !doc.contains_key("pty_proxy") {
        doc["pty_proxy"] = toml_edit::table();
    }
    doc["pty_proxy"]["enabled"] = value(capture_output);

    if !doc.contains_key("daemon") {
        doc["daemon"] = toml_edit::table();
    }
    doc["daemon"]["enabled"] = value(true);
    doc["daemon"]["autostart"] = value(true);
    doc["search_mode"] = value("daemon-fuzzy");

    doc["shell_up_key_binding"] = value(true);

    #[cfg(feature = "ai")]
    {
        if !doc.contains_key("ai") {
            doc["ai"] = toml_edit::table();
        }
        doc["ai"]["enabled"] = value(search.contains(&SearchChoice::AtuinAi));
    }

    doc["auto_sync"] = value(sync != SyncChoice::None);

    Ok(())
}

fn has_obvious_history_file() -> bool {
    let home = atuin_common::utils::home_dir();
    let shell = std::env::var("SHELL").unwrap_or_default();

    if shell.ends_with("/bash") {
        return std::env::var_os("HISTFILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".bash_history"))
            .is_file();
    }

    if shell.ends_with("/zsh") {
        return home.join(".zsh_history").is_file()
            || home.join(".histdb/zsh-history.db").is_file();
    }

    if shell.ends_with("/fish") {
        let xdg_data = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        return xdg_data.join("fish/fish_history").is_file();
    }

    true
}

fn install_shell_init() -> Result<()> {
    let home = atuin_common::utils::home_dir();
    let zdotdir = std::env::var_os("ZDOTDIR").map_or_else(|| home.clone(), Into::into);

    append_if_missing(&zdotdir.join(".zshrc"), "atuin init zsh", "\neval \"$(atuin init zsh)\"\n")?;
    append_if_missing(&home.join(".bashrc"), "atuin init bash", "\neval \"$(atuin init bash)\"\n")?;

    let fish_config = home.join(".config/fish/config.fish");
    if fish_config.exists() {
        append_if_missing(
            &fish_config,
            "atuin init fish",
            "\nif status is-interactive\n    atuin init fish | source\nend\n",
        )?;
    }

    println!("{check} Shell integration installed", check = "✓".bold().bright_green());
    Ok(())
}

fn append_if_missing(path: &std::path::Path, needle: &str, text: &str) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if existing.contains(needle) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn install_detected_agent_hooks() {
    for (agent, name, config_dir, commands) in [
        ("claude-code", "Claude Code", ".claude", &["claude"][..]),
        ("codex", "Codex", ".codex", &["codex"][..]),
        ("opencode", "opencode", ".config/opencode", &["opencode"][..]),
        ("pi", "pi", ".pi", &["pi"][..]),
    ] {
        if agent_detected(config_dir, commands) {
            println!("Detected {name} — installing Atuin hooks...");
            if let Err(err) = super::hook::install(agent) {
                eprintln!("Failed to install hooks for {name}: {err}");
            }
        }
    }
}

fn agent_detected(config_dir: &str, commands: &[&str]) -> bool {
    let home = atuin_common::utils::home_dir();
    if home.join(config_dir).is_dir() {
        return true;
    }

    commands.iter().any(|command| {
        Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {}", shell_escape(command)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(feature = "sync")]
async fn register_or_login(
    settings: &Settings,
    store: &SqliteStore,
    account: AccountChoice,
) -> Result<()> {
    match account {
        AccountChoice::Register => {
            account::register::Cmd {
                username: None,
                password: None,
                email: None,
            }
            .run(settings, store)
            .await
        }
        AccountChoice::Login => {
            account::login::Cmd {
                username: None,
                password: None,
                key: None,
                totp_code: None,
                from_registration: false,
            }
            .run(settings, store)
            .await
        }
        AccountChoice::Skip => {
            println!("\nYou can run 'atuin register' or 'atuin login' later.");
            Ok(())
        }
    }
}

#[cfg(not(feature = "sync"))]
async fn register_or_login(
    _settings: &Settings,
    _store: &SqliteStore,
    _account: AccountChoice,
) -> Result<()> {
    println!("\nThis build of Atuin does not include sync support.");
    Ok(())
}
