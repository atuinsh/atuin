//! What `atuin setup` draws, inline below whatever ran it: the choices, then what it's doing.

use std::io::{self, Write};
use std::time::Duration;
#[cfg(feature = "octavo")]
use std::time::Instant;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::harnesstools::Harness as _;
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Print, SetAttribute};
use eyre::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::{DefaultTerminal, Frame, TerminalOptions, Viewport};
use tokio::sync::mpsc;

#[cfg(feature = "octavo")]
use super::upload::{Gauge, Progress, State, Tracker, Upload};
use super::{
    Answer, Found, Offer, Plan, Status, Step, Sync, THREADS, Teams, Update, count, is_address,
    names,
};

const HEIGHT: u16 = 22;
const ACCENT: Color = Color::Green;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SYNC: bool = cfg!(feature = "sync");

/// Ask, starting from `plan`, then set up. `ask_import` is false when they've said not to import.
pub(super) async fn run(
    found: Found,
    plan: &Plan,
    ask_import: bool,
    store: &SqliteStore,
) -> Result<()> {
    let rows = crossterm::terminal::size().map_or(HEIGHT, |(_, rows)| rows);
    let mut terminal = ratatui::try_init_with_options(TerminalOptions {
        viewport: Viewport::Inline(HEIGHT.min(rows)),
    })?;

    let result = Box::pin(drive(&mut terminal, &found, plan, ask_import, store)).await;

    // Leave the cursor where the screen began, for the summary.
    let top = terminal.get_frame().area().y;
    let _ = terminal.clear();
    let _ = terminal.set_cursor_position((0, top));
    ratatui::restore();

    match result? {
        Some((plan, steps)) => super::print_summary(&plan, &found, &steps).await,
        None => println!("Setup cancelled, nothing changed. Run `atuin setup` any time."),
    }
    Ok(())
}

/// The plan and how each step went, `None` if they quit before choosing.
async fn drive(
    terminal: &mut ratatui::DefaultTerminal,
    found: &Found,
    plan: &Plan,
    ask_import: bool,
    store: &SqliteStore,
) -> Result<Option<(Plan, Vec<(Step, Status)>)>> {
    let mut choose = Choose::new(found, plan);
    let plan = loop {
        terminal.draw(|frame| choose.draw(frame, found))?;
        if let Some(key) = next_key().await?
            && let Some(chosen) = choose.handle(key, found)
        {
            match chosen {
                Chosen::Plan(plan) => break plan,
                Chosen::Quit => return Ok(None),
            }
        }
    };

    let (tx, mut updates) = mpsc::unbounded_channel();
    let (answer, answers) = mpsc::unbounded_channel();
    let mut work = Box::pin(super::apply(&plan, found, store, &tx, answers));

    let mut running = Running::default();
    let mut tick = 0;
    let offer = loop {
        while let Ok(update) = updates.try_recv() {
            running.update(update);
        }
        running.show(terminal, tick)?;
        tick += 1;
        tokio::select! {
            offer = work.as_mut() => break Some(offer?),
            key = next_key() => {
                let Some(key) = key? else { continue };
                match running.handle(key) {
                    Handled::Answer(a) => { let _ = answer.send(a); }
                    Handled::Abort => {
                        // A sign-in left half done would sync with a key that can't read the
                        // account, and the next setup would take it as done.
                        let signing_in = running.steps.iter().any(|(step, status)| {
                            *step == Step::Sync && matches!(status, Status::Running(_))
                        });
                        if signing_in {
                            super::sign_out().await;
                        }
                        break None;
                    }
                    Handled::Nothing => {}
                }
            }
        }
    };

    let Some(offer) = offer else {
        // ctrl-c: what was running stays as it was.
        return Ok(Some(outcome(&plan, running, &mut updates)));
    };
    // Sign-in has said what the account holds, so what's left is theirs to import.
    match offer {
        _ if offer.is_empty() => {}
        offer if !ask_import => super::skip_import(&offer, &tx),
        offer => {
            let mut import = Import::new(&offer);
            // The screen doesn't know about the sign-in link, so wouldn't clear it.
            terminal.clear()?;
            let take = loop {
                terminal.draw(|frame| import.draw(frame, &offer, found))?;
                if let Some(key) = next_key().await?
                    && let Some(take) = import.handle(key, &offer)
                {
                    break take;
                }
            };
            super::skip_import(
                &Offer {
                    history: offer.history.filter(|_| take.history.is_none()),
                    threads: offer.threads.filter(|_| take.threads.is_none()),
                },
                &tx,
            );
            if !take.is_empty() {
                let import = super::import(&take, &tx, plan.waits_for_imports());
                let imported = drawn(terminal, &mut running, &mut updates, &mut tick, import);
                if imported.await?.is_none() {
                    return Ok(Some(outcome(&plan, running, &mut updates)));
                }
            }
        }
    }

    #[cfg(feature = "octavo")]
    if let Some(Teams::On { backfill }) = plan.teams {
        let on = super::turn_on_teams(store, backfill, &tx);
        match drawn(terminal, &mut running, &mut updates, &mut tick, on).await? {
            Some(Some(upload)) => {
                watch(terminal, &mut running, &mut updates, &mut tick, &upload).await?;
            }
            Some(None) => {}
            None => return Ok(Some(outcome(&plan, running, &mut updates))),
        }
    }
    Ok(Some(outcome(&plan, running, &mut updates)))
}

/// The plan, and each step as the screen last heard of it.
fn outcome(
    plan: &Plan,
    mut running: Running,
    updates: &mut mpsc::UnboundedReceiver<Update>,
) -> (Plan, Vec<(Step, Status)>) {
    while let Ok(update) = updates.try_recv() {
        running.update(update);
    }
    (plan.clone(), running.steps)
}

/// Run `work` with the screen kept moving. `None` if they pressed ctrl-c first, which drops it.
async fn drawn<T>(
    terminal: &mut DefaultTerminal,
    running: &mut Running,
    updates: &mut mpsc::UnboundedReceiver<Update>,
    tick: &mut usize,
    work: impl Future<Output = T>,
) -> Result<Option<T>> {
    let mut work = std::pin::pin!(work);
    loop {
        while let Ok(update) = updates.try_recv() {
            running.update(update);
        }
        running.show(terminal, *tick)?;
        *tick += 1;
        tokio::select! {
            done = work.as_mut() => return Ok(Some(done)),
            key = next_key() => {
                if let Some(key) = key?
                    && matches!(running.handle(key), Handled::Abort)
                {
                    return Ok(None);
                }
            }
        }
    }
}

/// Show `upload` going until it's done or they leave, then say in the summary how far it got.
#[cfg(feature = "octavo")]
async fn watch(
    terminal: &mut DefaultTerminal,
    running: &mut Running,
    updates: &mut mpsc::UnboundedReceiver<Update>,
    tick: &mut usize,
    upload: &Upload,
) -> Result<()> {
    /// Often enough to look live, seldom enough to leave the daemon its database.
    const READ_EVERY: Duration = Duration::from_millis(500);

    while let Ok(update) = updates.try_recv() {
        running.update(update);
    }
    let uploading = Status::Running("Uploading to Atuin for Teams".into());
    running.update(Update::Step(Step::Upload, uploading));
    let mut tracker = Tracker::default();
    let mut read_at: Option<Instant> = None;
    let summary = loop {
        if read_at.is_none_or(|at| at.elapsed() >= READ_EVERY) {
            read_at = Some(Instant::now());
            match upload.backlog().await {
                Ok(backlog) => {
                    let now = Instant::now();
                    running.upload = Some(tracker.update(&backlog, upload.daemon_running(), now));
                }
                Err(e) => {
                    break Status::Failed(format!(
                        "Couldn't follow the upload ({e}). It goes on in the background"
                    ));
                }
            }
        }
        running.show(terminal, *tick)?;
        *tick += 1;
        if let Some(done) = running.upload.filter(|progress| progress.state == State::Done) {
            break Status::Done(done.summary());
        }
        if let Some(key) = next_key().await?
            && leaves(key)
        {
            let summary = running.upload.as_ref().map(Progress::summary);
            break Status::Done(
                summary.unwrap_or_else(|| "Uploading to Atuin for Teams in the background".into()),
            );
        }
    };
    running.upload = None;
    running.update(Update::Step(Step::Upload, summary));
    Ok(())
}

/// Whether `key` leaves the upload to go on in the background.
#[cfg(feature = "octavo")]
fn leaves(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
}

/// The next key pressed, `None` once a moment passes without one, so spinners keep turning.
async fn next_key() -> Result<Option<KeyEvent>> {
    if !tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(80))).await?? {
        return Ok(None);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(key)),
        _ => Ok(None),
    }
}

/// Record `status` for `step`, keeping the steps in the order they're shown.
pub(super) fn set(steps: &mut Vec<(Step, Status)>, step: Step, status: Status) {
    match steps.binary_search_by_key(&step, |&(s, _)| s) {
        Ok(i) => steps[i].1 = status,
        Err(i) => steps.insert(i, (step, status)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Shell,
    Threads,
    AgentHistory,
    CtrlR,
    UpArrow,
    Mcp,
    Sync,
    Server,
    Teams,
    /// Whether Atuin for Teams takes what Atuin already has too.
    Backfill,
    /// The button that goes on to set it all up.
    Go,
}

impl Row {
    /// The question the row comes under.
    const fn heading(self) -> Option<&'static str> {
        match self {
            Self::Shell | Self::Threads | Self::AgentHistory => Some("What should Atuin capture?"),
            Self::CtrlR | Self::UpArrow | Self::Mcp => Some("How do you want to search it?"),
            Self::Sync | Self::Server | Self::Teams | Self::Backfill => {
                Some("Where should it sync?")
            }
            Self::Go => None,
        }
    }
}

const SYNC_CHOICES: [&str; 3] = ["Atuin Cloud", "Your own server", "Off"];

enum Chosen {
    Plan(Plan),
    Quit,
}

/// The first screen: what to set up.
#[expect(clippy::struct_excessive_bools, reason = "a choice each, on or off")]
struct Choose {
    shell: bool,
    threads: bool,
    agent_history: bool,
    ctrl_r: bool,
    up_arrow: bool,
    mcp: bool,
    /// Into [`SYNC_CHOICES`].
    sync: usize,
    server: String,
    /// Atuin for Teams, when setup asks about it.
    teams: Option<bool>,
    backfill: bool,
    row: Row,
    error: Option<&'static str>,
}

impl Choose {
    fn new(found: &Found, plan: &Plan) -> Self {
        let (sync, server) = match &plan.sync {
            Sync::Cloud => (0, found.server.clone()),
            Sync::Server(address) => (1, Some(address.clone())),
            Sync::Off => (2, found.server.clone()),
        };
        Self {
            shell: plan.shell,
            threads: plan.threads,
            agent_history: plan.agent_history,
            ctrl_r: plan.ctrl_r,
            up_arrow: plan.up_arrow,
            mcp: plan.mcp,
            sync,
            server: server.unwrap_or_default(),
            teams: plan.teams.map(|teams| teams != Teams::Off),
            backfill: plan.teams == Some(Teams::On { backfill: true }),
            row: Row::Shell,
            error: None,
        }
    }

    fn rows(&self, found: &Found) -> Vec<Row> {
        let agents = THREADS && !found.agents.is_empty();
        let mut rows = vec![Row::Shell];
        if agents {
            rows.extend([Row::AgentHistory, Row::Threads]);
        }
        // The keys search shell history.
        if self.shell {
            rows.extend([Row::CtrlR, Row::UpArrow]);
        }
        if agents {
            rows.push(Row::Mcp);
        }
        if SYNC {
            rows.push(Row::Sync);
            if self.sync == 1 {
                rows.push(Row::Server);
            }
            // Atuin for Teams uploads as the account sync signs in to, so it needs sync.
            if self.teams.is_some() && self.sync != 2 {
                rows.push(Row::Teams);
                if self.teams == Some(true) {
                    rows.push(Row::Backfill);
                }
            }
        }
        rows.push(Row::Go);
        rows
    }

    /// What they chose, once they have.
    fn handle(&mut self, key: KeyEvent, found: &Found) -> Option<Chosen> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Chosen::Quit);
        }
        self.error = None;
        let rows = self.rows(found);
        let at = rows.iter().position(|&row| row == self.row).unwrap_or(0);

        match key.code {
            KeyCode::Enter if self.row == Row::Go => return self.plan().map(Chosen::Plan),
            KeyCode::Enter if self.row == Row::Server => self.row = Row::Go,
            KeyCode::Backspace if self.row == Row::Server => {
                self.server.pop();
            }
            KeyCode::Char(c) if self.row == Row::Server => self.server.push(c),
            KeyCode::Esc | KeyCode::Char('q') => return Some(Chosen::Quit),
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                self.row = rows[at.saturating_sub(1)];
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                self.row = rows[(at + 1).min(rows.len() - 1)];
            }
            KeyCode::Enter | KeyCode::Char(' ' | 'l') | KeyCode::Right => self.change(1),
            KeyCode::Left | KeyCode::Char('h') => self.change(SYNC_CHOICES.len() - 1),
            _ => {}
        }
        None
    }

    /// Toggle the row, or move its choice on by `by`.
    fn change(&mut self, by: usize) {
        match self.row {
            Row::Shell => self.shell = !self.shell,
            Row::Threads => self.threads = !self.threads,
            Row::AgentHistory => self.agent_history = !self.agent_history,
            Row::CtrlR => self.ctrl_r = !self.ctrl_r,
            Row::UpArrow => self.up_arrow = !self.up_arrow,
            Row::Mcp => self.mcp = !self.mcp,
            Row::Sync => self.sync = (self.sync + by) % SYNC_CHOICES.len(),
            Row::Teams => self.teams = self.teams.map(|on| !on),
            Row::Backfill => self.backfill = !self.backfill,
            Row::Server | Row::Go => {}
        }
    }

    fn plan(&mut self) -> Option<Plan> {
        let sync = match self.sync {
            0 => Sync::Cloud,
            1 => {
                let address = self.server.trim();
                if !is_address(address) {
                    self.error = Some("Enter your server's address, starting http:// or https://");
                    self.row = Row::Server;
                    return None;
                }
                Sync::Server(address.to_string())
            }
            _ => Sync::Off,
        };
        if !self.shell && !self.threads && !self.agent_history && !self.mcp && sync == Sync::Off {
            self.error = Some("Choose something to set up, or press esc to leave");
            return None;
        }
        let teams = self.teams.map(|on| {
            if on && sync != Sync::Off {
                Teams::On {
                    backfill: self.backfill,
                }
            } else {
                Teams::Off
            }
        });
        Some(Plan {
            shell: self.shell,
            threads: self.threads,
            agent_history: self.agent_history,
            ctrl_r: self.ctrl_r,
            up_arrow: self.up_arrow,
            mcp: self.mcp,
            sync,
            teams,
        })
    }

    fn draw(&self, frame: &mut Frame<'_>, found: &Found) {
        let mut lines = title();
        lines.extend(found_lines(found));

        let mut heading = None;
        let mut focus = 0;
        for (i, row) in self.rows(found).into_iter().enumerate() {
            if row.heading() != heading && row != Row::Server {
                heading = row.heading();
                if i > 0 {
                    lines.push(Line::default());
                }
                if let Some(heading) = heading {
                    lines.push(Line::from(format!("  {heading}").bold()));
                }
            }
            let at = row == self.row;
            if at {
                focus = lines.len();
            }
            lines.push(match row {
                Row::Shell => choice(at, self.shell, "Shell history", "all your commands"),
                Row::Threads => choice(
                    at,
                    self.threads,
                    "Agent threads",
                    "full agent sessions, resumable cross harness",
                ),
                Row::AgentHistory => choice(
                    at,
                    self.agent_history,
                    "Agent shell history",
                    "all your agents' commands",
                ),
                Row::CtrlR => choice(at, self.ctrl_r, "Ctrl-R", "search shell history"),
                Row::UpArrow => choice(at, self.up_arrow, "Up arrow", "also search shell history"),
                Row::Mcp => {
                    choice(at, self.mcp, "Agent search", "search history and threads over MCP")
                }
                Row::Sync => {
                    let about = match self.sync {
                        0 if found.syncing() == Some(Sync::Cloud) => "signed in",
                        0 => "encrypted, on all your machines",
                        1 => "",
                        _ => "keep everything on this machine",
                    };
                    let value = Span::raw(format!("‹ {} ›", SYNC_CHOICES[self.sync]));
                    let value = if at {
                        value.fg(ACCENT).bold()
                    } else {
                        value
                    };
                    let spans = vec![value, dim(format!("  {about}"))];
                    toggle(at, None, self.sync != 2, "Sync", spans)
                }
                Row::Teams => choice(
                    at,
                    self.teams == Some(true),
                    "Atuin for Teams",
                    "share with your team, not end-to-end encrypted",
                ),
                Row::Backfill => choice(
                    at,
                    self.backfill,
                    "Upload what's here",
                    "the history and threads Atuin already has",
                ),
                Row::Go => button(at, "Continue"),
                Row::Server => {
                    let mut spans = vec![Span::raw(" ".repeat(8)), pad("Address", at)];
                    if self.server.is_empty() && !at {
                        spans.push(dim("https://"));
                    } else {
                        spans.push(Span::raw(self.server.clone()));
                    }
                    if at {
                        spans.push(Span::raw(" ").reversed());
                    }
                    Line::from(spans)
                }
            });
        }

        lines.push(Line::default());
        lines.push(match self.error {
            Some(error) => Line::from(Span::raw(format!("  {error}")).yellow()),
            None if self.row == Row::Go => {
                hints(&[("enter", "continue"), ("↑↓", "move"), ("esc", "quit")])
            }
            None if self.row == Row::Server => hints(&[("enter", "next"), ("esc", "quit")]),
            None if self.row == Row::Sync => {
                hints(&[("enter", "change"), ("↑↓", "move"), ("esc", "quit")])
            }
            None => hints(&[("enter", "toggle"), ("↑↓", "move"), ("esc", "quit")]),
        });

        // A short terminal can't show it all: scroll to keep the row they're on, and the two lines
        // after it (on Continue, the hint), in sight. An error, on the last line, always shows.
        let shown = if self.error.is_some() {
            lines.len()
        } else {
            (focus + 3).min(lines.len())
        };
        let scroll = shown.saturating_sub(usize::from(frame.area().height));
        let scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), frame.area());
    }
}

/// How the browser sign-in is going.
#[derive(Default)]
enum SignIn {
    #[default]
    None,
    Browser {
        url: String,
        opened: bool,
    },
    Key {
        entered: String,
        error: Option<String>,
    },
}

enum Handled {
    Answer(Answer),
    Abort,
    Nothing,
}

/// The second screen: each step, as it goes.
#[derive(Default)]
struct Running {
    steps: Vec<(Step, Status)>,
    sign_in: SignIn,
    /// Where the sign-in link was written, last frame.
    link_at: Option<(u16, u16)>,
    /// How far the Atuin for Teams upload has got, while setup watches it.
    #[cfg(feature = "octavo")]
    upload: Option<Progress>,
}

/// The sign-in link: where it goes, and the address.
type Link = (u16, u16, String);

impl Running {
    fn update(&mut self, update: Update) {
        match update {
            Update::Step(step, status) => {
                if step == Step::Sync && !matches!(status, Status::Running(_)) {
                    self.sign_in = SignIn::None;
                }
                set(&mut self.steps, step, status);
            }
            Update::Browser { url, opened } => self.sign_in = SignIn::Browser { url, opened },
            Update::NeedKey(error) => {
                self.sign_in = SignIn::Key {
                    entered: String::new(),
                    error,
                }
            }
        }
    }

    fn handle(&mut self, key: KeyEvent) -> Handled {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Handled::Abort;
        }
        match &mut self.sign_in {
            SignIn::None => Handled::Nothing,
            SignIn::Browser { .. } => match key.code {
                KeyCode::Char('o') => Handled::Answer(Answer::OpenAgain),
                KeyCode::Char('s') | KeyCode::Esc => Handled::Answer(Answer::Skip),
                _ => Handled::Nothing,
            },
            SignIn::Key { entered, .. } => match key.code {
                KeyCode::Esc => Handled::Answer(Answer::Skip),
                KeyCode::Enter => {
                    let entered = std::mem::take(entered);
                    self.sign_in = SignIn::None;
                    Handled::Answer(Answer::Key(entered))
                }
                KeyCode::Backspace => {
                    entered.pop();
                    Handled::Nothing
                }
                KeyCode::Char(c) => {
                    entered.push(c);
                    Handled::Nothing
                }
                _ => Handled::Nothing,
            },
        }
    }

    /// Draw, then write the sign-in link out as text of its own, which the terminal wraps as one
    /// line, so it can be copied or clicked whole however narrow the terminal is.
    fn show(&mut self, terminal: &mut DefaultTerminal, tick: usize) -> Result<()> {
        let mut link = None;
        terminal.draw(|frame| link = self.draw(frame, tick))?;
        let at = link.as_ref().map(|&(x, y, _)| (x, y));
        // The screen doesn't know the link is there, so it wouldn't clear it once it moved or went.
        if self.link_at.is_some() && self.link_at != at {
            terminal.clear()?;
            terminal.draw(|frame| drop(self.draw(frame, tick)))?;
        }
        self.link_at = at;
        if let Some((x, y, url)) = link {
            let mut out = io::stdout();
            crossterm::queue!(
                out,
                MoveTo(x, y),
                SetAttribute(Attribute::Underlined),
                Print(url),
                SetAttribute(Attribute::Reset)
            )?;
            out.flush()?;
        }
        Ok(())
    }

    /// Draws all but the sign-in link, and says where that goes.
    fn draw(&self, frame: &mut Frame<'_>, tick: usize) -> Option<Link> {
        let mut lines = title();
        let mut hint = None;
        let mut url_at = None;

        for (step, status) in &self.steps {
            let line = match status {
                Status::Running(text) => Line::from(vec![
                    Span::raw("  "),
                    Span::raw(SPINNER[tick % SPINNER.len()]).fg(ACCENT),
                    Span::raw(format!(" {text}")),
                ]),
                Status::Done(text) => {
                    Line::from(vec![Span::raw("  "), "✓".fg(ACCENT), Span::raw(format!(" {text}"))])
                }
                Status::Failed(text) => Line::from(vec![
                    Span::raw("  "),
                    "!".yellow().bold(),
                    Span::raw(format!(" {text}")),
                ]),
            };
            lines.push(line);
            #[cfg(feature = "octavo")]
            if *step == Step::Upload
                && let Some(progress) = &self.upload
            {
                lines.extend(upload_lines(progress));
                hint = Some(hints(&[("q", "leave, the upload goes on in the background")]));
            }
            if *step != Step::Sync {
                continue;
            }

            match &self.sign_in {
                SignIn::None => {}
                SignIn::Browser { url, opened } => {
                    let how = if *opened {
                        "Continue in your browser, or open:"
                    } else {
                        "Open this link to sign in or create an account:"
                    };
                    lines.push(Line::from(dim(format!("    {how}"))));
                    let area = frame.area();
                    let rows = (4 + url.len()).div_ceil(usize::from(area.width.max(1)));
                    url_at = Some((area.x + 4, area.y + height(&lines, area), url.clone()));
                    lines.extend(std::iter::repeat_n(Line::default(), rows));
                    hint = Some(if *opened {
                        hints(&[("o", "open again"), ("s", "skip sync")])
                    } else {
                        hints(&[("s", "skip sync")])
                    });
                }
                SignIn::Key { entered, error } => {
                    lines.push(Line::from(
                        "    This account already has synced data. Enter its key,",
                    ));
                    lines.push(Line::from(dim("    from `atuin key` on a machine that syncs.")));
                    if let Some(error) = error {
                        lines.push(Line::from(Span::raw(format!("    {error}")).yellow()));
                    }
                    let shown = entered.chars().count().min(32);
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::raw("Key  ").bold(),
                        Span::raw("•".repeat(shown)),
                        Span::raw(" ").reversed(),
                    ]));
                    hint = Some(hints(&[("enter", "continue"), ("esc", "skip sync")]));
                }
            }
        }

        if let Some(hint) = hint {
            lines.push(Line::default());
            lines.push(hint);
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), frame.area());
        url_at
    }
}

/// The gauges under the upload's step, and why it isn't moving, if it isn't.
#[cfg(feature = "octavo")]
fn upload_lines(progress: &Progress) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if progress.commands.total > 0 {
        lines.push(gauge_line("Commands", progress.commands));
    }
    match progress.threads {
        Some(threads) if threads.total > 0 => lines.push(gauge_line("Threads", threads)),
        Some(_) => {}
        None => lines.push(Line::from(vec![Span::raw("    Threads   "), dim("counting…")])),
    }
    let why = match progress.state {
        State::Uploading | State::Done => None,
        State::Paused => {
            Some(Span::raw("    Paused while your account has Atuin for Teams off").yellow())
        }
        State::Stalled {
            daemon_running: false,
        } => Some(
            Span::raw(
                "    Nothing's gone up for a while, and the daemon isn't running: run `atuin \
                 daemon start`",
            )
            .yellow(),
        ),
        State::Stalled {
            daemon_running: true,
        } => Some(dim("    Nothing's gone up for a while: the daemon keeps trying")),
    };
    lines.extend(why.map(Line::from));
    lines
}

/// `label`, a bar as full as `gauge`, and its counts.
#[cfg(feature = "octavo")]
fn gauge_line(label: &str, gauge: Gauge) -> Line<'static> {
    const WIDTH: u64 = 24;
    let filled = u128::from(gauge.done) * u128::from(WIDTH) / u128::from(gauge.total.max(1));
    let filled = u64::try_from(filled).unwrap_or(WIDTH).min(WIDTH);
    let cells = |n: u64| usize::try_from(n).unwrap_or_default();
    Line::from(vec![
        Span::raw(format!("    {label:<10}")),
        Span::raw("█".repeat(cells(filled))).fg(ACCENT),
        dim("░".repeat(cells(WIDTH - filled))),
        Span::raw(format!("  {} of {}", count(gauge.done), count(gauge.total))),
    ])
}

/// The rows `lines` take in `area`, wrapped as the screen wraps them.
fn height(lines: &[Line<'static>], area: Rect) -> u16 {
    let mut buffer = Buffer::empty(area);
    Paragraph::new(lines.to_vec()).wrap(Wrap { trim: false }).render(area, &mut buffer);
    let blank = |y| (area.left()..area.right()).all(|x| buffer[(x, y)].symbol() == " ");
    (area.top()..area.bottom()).rfind(|&y| !blank(y)).map_or(0, |y| y + 1 - area.top())
}

fn title() -> Vec<Line<'static>> {
    vec![
        Line::default(),
        Line::from(vec![Span::raw("  "), "atuin".fg(ACCENT).bold(), " setup".bold()]),
        Line::default(),
    ]
}

/// "Found 48,210 commands in your zsh history / and 353 threads from Claude Code and Codex."
fn found_lines(found: &Found) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let agents =
        names(&found.agents().into_iter().map(|agent| agent.display_name()).collect::<Vec<_>>());
    let threads = found.threads();

    if let Some((shell, commands)) = found.history
        && commands > 0
    {
        lines.push(Line::from(vec![
            Span::raw("  Found "),
            Span::raw(count(commands)).bold(),
            Span::raw(format!(" commands in your {shell} history")),
        ]));
    }
    if !found.agents.is_empty() {
        let lead = if lines.is_empty() {
            "  Found "
        } else {
            "  and "
        };
        let mut spans = vec![Span::raw(lead)];
        if threads > 0 {
            spans.push(Span::raw(count(threads)).bold());
            spans.push(Span::raw(if threads == 1 {
                " thread from "
            } else {
                " threads from "
            }));
            spans.push(Span::raw(agents));
        } else {
            spans.push(Span::raw(format!("{agents}, with no threads yet")));
        }
        lines.push(Line::from(spans));
    }
    if let Some(last) = lines.last_mut() {
        last.push_span(Span::raw("."));
        lines.push(Line::default());
    }
    lines
}

/// The screen after sign-in: what to import of what's already here.
struct Import {
    history: bool,
    threads: bool,
    row: Row,
}

impl Import {
    fn new(offer: &Offer) -> Self {
        Self {
            history: true,
            threads: true,
            row: Self::rows(offer)[0],
        }
    }

    fn rows(offer: &Offer) -> Vec<Row> {
        let mut rows = Vec::new();
        if offer.history.is_some() {
            rows.push(Row::Shell);
        }
        if offer.threads.is_some() {
            rows.push(Row::Threads);
        }
        rows.push(Row::Go);
        rows
    }

    /// What they chose to import, once they have.
    fn handle(&mut self, key: KeyEvent, offer: &Offer) -> Option<Offer> {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        let rows = Self::rows(offer);
        let at = rows.iter().position(|&row| row == self.row).unwrap_or(0);
        match key.code {
            KeyCode::Enter if self.row == Row::Go => {
                return Some(Offer {
                    history: offer.history.filter(|_| self.history),
                    threads: offer.threads.filter(|_| self.threads),
                });
            }
            KeyCode::Esc | KeyCode::Char('q') => return Some(Offer::default()),
            _ if ctrl_c => return Some(Offer::default()),
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                self.row = rows[at.saturating_sub(1)];
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                self.row = rows[(at + 1).min(rows.len() - 1)];
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.row {
                Row::Shell => self.history = !self.history,
                Row::Threads => self.threads = !self.threads,
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn draw(&self, frame: &mut Frame<'_>, offer: &Offer, found: &Found) {
        let mut lines = title();
        lines.push(Line::from("  Import what's already here?".bold()));
        if let Some((shell, commands)) = offer.history {
            let about = format!("{} commands from {shell}", count(commands));
            lines.push(choice(self.row == Row::Shell, self.history, "Shell history", about));
        }
        if let Some(threads) = offer.threads {
            let agents = names(
                &found.agents().into_iter().map(|agent| agent.display_name()).collect::<Vec<_>>(),
            );
            let noun = if threads == 1 {
                "thread"
            } else {
                "threads"
            };
            let about = format!("{} {noun} from {agents}", count(threads));
            lines.push(choice(self.row == Row::Threads, self.threads, "Agent threads", about));
        }
        lines.push(Line::default());
        lines.push(button(self.row == Row::Go, "Import"));
        lines.push(Line::default());
        lines.push(if self.row == Row::Go {
            hints(&[("enter", "import"), ("↑↓", "move"), ("esc", "skip")])
        } else {
            hints(&[("enter", "toggle"), ("↑↓", "move"), ("esc", "skip")])
        });
        frame.render_widget(Paragraph::new(lines), frame.area());
    }
}

/// A row of the first screen: a toggle, its name and what it means.
fn choice(at: bool, on: bool, label: &'static str, about: impl Into<String>) -> Line<'static> {
    toggle(at, Some(on), on, label, vec![dim(about)])
}

/// A row: a checkbox, if `checked` is one, its name and what it means, dim when not `on`.
fn toggle(
    at: bool,
    checked: Option<bool>,
    on: bool,
    label: &'static str,
    about: Vec<Span<'static>>,
) -> Line<'static> {
    let box_ = match checked {
        Some(true) => "[✓] ".fg(ACCENT).bold(),
        Some(false) => "[ ] ".dim(),
        None => Span::raw("    "),
    };
    let mut spans = vec![Span::raw("  "), cursor(at), box_, pad(label, at)];
    spans.extend(about);
    let line = Line::from(spans);
    if on {
        line
    } else {
        line.style(Style::default().add_modifier(Modifier::DIM))
    }
}

fn cursor(at: bool) -> Span<'static> {
    if at {
        "› ".fg(ACCENT).bold()
    } else {
        Span::raw("  ")
    }
}

/// The button at the bottom of a screen, that does what it says.
fn button(at: bool, label: &'static str) -> Line<'static> {
    let text = Span::raw(format!(" {label} "));
    let text = if at {
        text.fg(Color::Black).bg(ACCENT).bold()
    } else {
        text.fg(ACCENT)
    };
    Line::from(vec![Span::raw("  "), cursor(at), text])
}

fn pad(label: &'static str, at: bool) -> Span<'static> {
    let label = Span::raw(format!("{label:<21}"));
    if at {
        label.bold()
    } else {
        label
    }
}

fn dim(text: impl Into<String>) -> Span<'static> {
    Span::raw(text.into()).dim()
}

/// "enter toggle · ↑↓ move", the keys bold.
fn hints(keys: &[(&'static str, &'static str)]) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    for (i, (key, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(dim("   "));
        }
        spans.push(Span::raw(*key).bold());
        spans.push(dim(format!(" {what}")));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use atuin_common::harnesstools::AnyHarness;
    use atuin_common::harnesstools::ccode::Ccode;
    use atuin_common::harnesstools::codex::Codex;
    use clap::Parser as _;
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn found() -> Found {
        Found {
            history: Some(("zsh", 48210)),
            imported: 0,
            threads_here: 0,
            agents: vec![(AnyHarness::ClaudeCode(Ccode), 300), (AnyHarness::Codex(Codex), 53)],
            hooked: false,
            ctrl_r: true,
            up_arrow: true,
            capture: false,
            signed_in: false,
            server: None,
        }
    }

    /// The plan `atuin setup` with no flags starts the screen from.
    fn preset(found: &Found) -> Plan {
        super::super::Cmd::parse_from(["setup"]).plan(found, false)
    }

    fn press(choose: &mut Choose, found: &Found, keys: &[KeyCode]) -> Option<Chosen> {
        keys.iter().find_map(|&code| choose.handle(KeyEvent::from(code), found))
    }

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[rstest]
    #[case::everything(&[KeyCode::Down, KeyCode::Down, KeyCode::Enter], true, true)]
    #[case::enter_toggles(&[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Enter], false, true)]
    #[case::not_threads(&[KeyCode::Down, KeyCode::Char(' '), KeyCode::Down, KeyCode::Enter], true, false)]
    #[case::esc_takes_nothing(&[KeyCode::Esc], false, false)]
    fn import_takes_what_is_left_on(
        #[case] keys: &[KeyCode],
        #[case] history: bool,
        #[case] threads: bool,
    ) {
        let offer = Offer {
            history: Some(("zsh", 48210)),
            threads: Some(353),
        };
        let mut import = Import::new(&offer);
        let take = keys.iter().find_map(|&code| import.handle(KeyEvent::from(code), &offer));
        assert_eq!(
            take,
            Some(Offer {
                history: offer.history.filter(|_| history),
                threads: offer.threads.filter(|_| threads),
            })
        );
    }

    #[rstest]
    fn the_button_takes_everything_found(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.row = Row::Go;
        let Some(Chosen::Plan(plan)) = press(&mut choose, &found, &[KeyCode::Enter]) else {
            panic!("expected a plan");
        };
        assert_eq!(plan, Plan {
            shell: true,
            threads: THREADS,
            agent_history: THREADS,
            ctrl_r: true,
            up_arrow: true,
            mcp: THREADS,
            sync: if SYNC {
                Sync::Cloud
            } else {
                Sync::Off
            },
            teams: None,
        });
    }

    #[rstest]
    fn continue_stays_in_sight_in_a_short_terminal(found: Found) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 12)).unwrap();
        let mut choose = Choose::new(&found, &preset(&found));
        while choose.row != Row::Go {
            press(&mut choose, &found, &[KeyCode::Down]);
        }
        let shown = terminal.draw(|frame| choose.draw(frame, &found)).unwrap().buffer.clone();
        let text: String = shown.content().iter().map(ratatui::buffer::Cell::symbol).collect();
        assert!(text.contains("Continue") && text.contains("enter continue"));
    }

    #[rstest]
    fn no_agents_no_agent_rows(mut found: Found) {
        found.agents.clear();
        let choose = Choose::new(&found, &preset(&found));
        let rows = choose.rows(&found);
        assert!(![Row::Threads, Row::AgentHistory, Row::Mcp].iter().any(|row| rows.contains(row)));
        assert!(!choose.threads && !choose.agent_history && !choose.mcp);
    }

    #[rstest]
    fn search_keys_come_with_shell_history(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        assert!(choose.rows(&found).contains(&Row::UpArrow));
        assert!(press(&mut choose, &found, &[KeyCode::Enter]).is_none());
        assert!(!choose.shell);
        assert!(!choose.rows(&found).contains(&Row::CtrlR));
        assert!(!choose.rows(&found).contains(&Row::UpArrow));
    }

    #[rstest]
    fn up_arrow_can_be_left_to_the_shell(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        while choose.row != Row::UpArrow {
            press(&mut choose, &found, &[KeyCode::Down]);
        }
        press(&mut choose, &found, &[KeyCode::Enter]);
        choose.row = Row::Go;
        let Some(Chosen::Plan(plan)) = press(&mut choose, &found, &[KeyCode::Enter]) else {
            panic!("expected a plan");
        };
        assert!(plan.ctrl_r && !plan.up_arrow);
    }

    #[cfg(feature = "sync")]
    #[rstest]
    fn own_server_needs_an_address(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.row = Row::Sync;
        assert!(press(&mut choose, &found, &[KeyCode::Enter]).is_none());
        choose.row = Row::Go;
        assert!(press(&mut choose, &found, &[KeyCode::Enter]).is_none());
        assert_eq!(choose.row, Row::Server);
        assert!(choose.error.is_some());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, HEIGHT)).unwrap();
        let shown = terminal.draw(|frame| choose.draw(frame, &found)).unwrap().buffer.clone();
        let text: String = shown.content().iter().map(ratatui::buffer::Cell::symbol).collect();
        assert!(text.contains("Enter your server's address"));

        let typed: Vec<_> = "https://atuin.example.com".chars().map(KeyCode::Char).collect();
        press(&mut choose, &found, &typed);
        let Some(Chosen::Plan(plan)) =
            press(&mut choose, &found, &[KeyCode::Enter, KeyCode::Enter])
        else {
            panic!("expected a plan");
        };
        assert_eq!(plan.sync, Sync::Server("https://atuin.example.com".into()));
    }

    #[cfg(feature = "sync")]
    #[rstest]
    #[case::left_on(&[], Some(Teams::On { backfill: false }))]
    #[case::with_backfill(&[Row::Backfill], Some(Teams::On { backfill: true }))]
    #[case::turned_off(&[Row::Teams], Some(Teams::Off))]
    #[case::turned_off_with_sync(&[Row::Backfill, Row::Sync, Row::Sync], Some(Teams::Off))]
    fn teams_is_chosen_beside_sync(
        found: Found,
        #[case] toggled: &[Row],
        #[case] expected: Option<Teams>,
    ) {
        let plan = Plan {
            teams: Some(Teams::On { backfill: false }),
            ..preset(&found)
        };
        let mut choose = Choose::new(&found, &plan);
        for &row in toggled {
            choose.row = row;
            press(&mut choose, &found, &[KeyCode::Enter]);
        }
        choose.row = Row::Go;
        let Some(Chosen::Plan(plan)) = press(&mut choose, &found, &[KeyCode::Enter]) else {
            panic!("expected a plan");
        };
        assert_eq!(plan.teams, expected);
    }

    #[cfg(feature = "sync")]
    #[rstest]
    #[case::not_asked(None, false, &[])]
    #[case::on(Some(Teams::On { backfill: false }), false, &[Row::Teams, Row::Backfill])]
    #[case::off(Some(Teams::Off), false, &[Row::Teams])]
    #[case::sync_off(Some(Teams::On { backfill: false }), true, &[])]
    fn teams_rows_follow_the_flag_and_sync(
        found: Found,
        #[case] teams: Option<Teams>,
        #[case] sync_off: bool,
        #[case] expected: &[Row],
    ) {
        let mut choose = Choose::new(&found, &Plan {
            teams,
            ..preset(&found)
        });
        if sync_off {
            choose.sync = 2;
        }
        let rows = choose.rows(&found);
        let shown: Vec<_> =
            rows.into_iter().filter(|row| matches!(row, Row::Teams | Row::Backfill)).collect();
        assert_eq!(shown, expected);
    }

    #[rstest]
    fn nothing_chosen_is_not_a_plan(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.shell = false;
        choose.threads = false;
        choose.agent_history = false;
        choose.mcp = false;
        choose.sync = 2;
        choose.row = Row::Go;
        assert!(press(&mut choose, &found, &[KeyCode::Enter]).is_none());
        assert!(choose.error.is_some());
    }

    #[rstest]
    #[case::both(true, 353, &["  Found 48,210 commands in your zsh history", "  and 353 threads from Claude Code and Codex.", ""])]
    #[case::no_threads_yet(true, 0, &["  Found 48,210 commands in your zsh history", "  and Claude Code and Codex, with no threads yet.", ""])]
    #[case::no_history(false, 1, &["  Found 1 thread from Claude Code and Codex.", ""])]
    fn found_reads_as_a_sentence(
        mut found: Found,
        #[case] history: bool,
        #[case] threads: u64,
        #[case] expected: &[&str],
    ) {
        if !history {
            found.history = None;
        }
        found.agents[0].1 = threads;
        found.agents[1].1 = 0;
        assert_eq!(text(&found_lines(&found)), expected);
    }

    #[cfg(feature = "octavo")]
    #[rstest]
    #[case::uploading(State::Uploading, None)]
    #[case::paused(State::Paused, Some("    Paused while your account has Atuin for Teams off"))]
    #[case::no_daemon(
        State::Stalled { daemon_running: false },
        Some("    Nothing's gone up for a while, and the daemon isn't running: run `atuin daemon start`"),
    )]
    #[case::retrying(
        State::Stalled { daemon_running: true },
        Some("    Nothing's gone up for a while: the daemon keeps trying"),
    )]
    fn the_upload_shows_a_bar_per_count_and_why_it_stalled(
        #[case] state: State,
        #[case] why: Option<&str>,
    ) {
        let progress = Progress {
            commands: Gauge {
                done: 4_210,
                total: 6_000,
            },
            threads: None,
            held: 0,
            state,
        };
        let mut expected = vec![
            format!("    Commands  {}{}  4,210 of 6,000", "█".repeat(16), "░".repeat(8)),
            "    Threads   counting…".to_owned(),
        ];
        expected.extend(why.map(str::to_owned));
        assert_eq!(text(&upload_lines(&progress)), expected);
    }

    #[cfg(feature = "octavo")]
    #[rstest]
    fn the_upload_draws_under_its_step_with_how_to_leave() {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 22)).unwrap();
        let running = Running {
            steps: vec![(Step::Upload, Status::Running("Uploading to Atuin for Teams".into()))],
            upload: Some(Progress {
                commands: Gauge { done: 0, total: 0 },
                threads: Some(Gauge { done: 1, total: 4 }),
                held: 0,
                state: State::Uploading,
            }),
            ..Running::default()
        };
        let shown = terminal.draw(|frame| drop(running.draw(frame, 0))).unwrap().buffer.clone();
        let rows: Vec<String> = shown
            .content()
            .chunks(100)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .map(|row| row.trim_end().to_owned())
            .collect();
        let at = rows.iter().position(|row| row.ends_with("Uploading to Atuin for Teams"));
        let at = at.expect("the step is drawn");
        assert_eq!(
            rows[at + 1],
            format!("    Threads   {}{}  1 of 4", "█".repeat(6), "░".repeat(18))
        );
        assert!(
            rows.iter().any(|row| row.contains("q leave, the upload goes on in the background"))
        );
    }

    #[rstest]
    fn steps_stay_in_order() {
        let mut steps = Vec::new();
        set(&mut steps, Step::Sync, Status::Running("a".into()));
        set(&mut steps, Step::Shells, Status::Done("b".into()));
        set(&mut steps, Step::Sync, Status::Done("c".into()));
        assert_eq!(steps, [
            (Step::Shells, Status::Done("b".into())),
            (Step::Sync, Status::Done("c".into())),
        ]);
    }
}
