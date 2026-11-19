//! What `atuin setup` draws, on the alternate screen: the choices, then what it's doing.

use std::io::{self, Write};
use std::ops::Range;
use std::time::Duration;
#[cfg(feature = "octavo")]
use std::time::Instant;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::ThreadUploads;
use atuin_common::env::var_nonempty;
use crossterm::cursor::MoveTo;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Attribute, Print, SetAttribute};
use eyre::Result;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc;

use super::agents::{AnyHarness, Harness};
use super::form::{Form as _, Needs};
use super::plan::{
    AgentHistory, Agents, AtuinForTeams, Backfill, CtrlR, HOOKABLE, OutputCapture, Plan,
    ShellHistory, Shells, Threads, UpArrow,
};
use super::scroll::Scroll;
#[cfg(feature = "octavo")]
use super::upload::{Gauge, Progress, State, Upload};
use super::{Answer, Found, Offer, Scope, Status, Step, Sync, Update, count, is_address, names};

pub(super) const ACCENT: Color = Color::Green;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Whether setup takes the mouse while they choose, so that the scrollbar's handle drags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mouse {
    Capture,
    /// Leave the mouse to the terminal, as `no_mouse` in the config asks.
    Leave,
}

/// Ask, starting from `plan`, then set up. `ask_import` is false when they've said not to import.
pub(super) async fn run(
    found: Found,
    plan: &Plan,
    ask_import: bool,
    mouse: Mouse,
    store: &SqliteStore,
) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let result = Box::pin(drive(&mut terminal, &found, plan, ask_import, mouse, store)).await;
    // Back on the screen they ran it from, for the summary.
    ratatui::restore();

    match result? {
        Some((plan, steps)) => super::print_summary(&plan, &found, Scope::All, &steps).await,
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
    mouse: Mouse,
    store: &SqliteStore,
) -> Result<Option<(Plan, Vec<(Step, Status)>)>> {
    let Some(plan) = choose(terminal, found, plan, mouse).await? else {
        return Ok(None);
    };

    let (tx, mut updates) = mpsc::unbounded_channel();
    let (answer, answers) = mpsc::unbounded_channel();
    let mut work = Box::pin(super::apply(&plan, found, Scope::All, store, &tx, answers));

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
                let import = super::import(&take, &tx, plan.waits_for_imports(), plan.daemon.0);
                let imported = drawn(terminal, &mut running, &mut updates, &mut tick, import);
                if imported.await?.is_none() {
                    return Ok(Some(outcome(&plan, running, &mut updates)));
                }
            }
        }
    }

    #[cfg(feature = "octavo")]
    if let Some(super::Teams::On { backfill }) = plan.teams() {
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

/// What they choose to set up, starting from `plan`, or `None` if they quit.
async fn choose(
    terminal: &mut DefaultTerminal,
    found: &Found,
    plan: &Plan,
    mouse: Mouse,
) -> Result<Option<Plan>> {
    // Colour, unless `NO_COLOR` says not to.
    let colour = var_nonempty("NO_COLOR").is_none();
    let colorfgbg = var_nonempty("COLORFGBG").and_then(|value| value.into_string().ok());
    let shade = colour.then(|| row_shade(colorfgbg.as_deref()));
    let mut choose = Choose::new(found, plan).coloured(colour).shaded(shade);
    // Only while they choose, because the screens after it show a sign-in link to select.
    let _mouse = match mouse {
        Mouse::Capture => Some(MouseCaptured::new()?),
        Mouse::Leave => None,
    };
    loop {
        terminal.draw(|frame| choose.draw(frame, found))?;
        let chosen = match next_input().await? {
            Some(Input::Key(key)) => choose.handle(key),
            Some(Input::Mouse(event)) => {
                choose.mouse(event, found, terminal.size()?);
                None
            }
            None => None,
        };
        match chosen {
            Some(Chosen::Plan(plan)) => return Ok(Some(plan)),
            Some(Chosen::Quit) => return Ok(None),
            None => {}
        }
    }
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
    let mut tracker = upload.tracker();
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

/// The mouse, which the terminal sends to setup instead of using itself while this lives.
struct MouseCaptured;

impl MouseCaptured {
    fn new() -> io::Result<Self> {
        crossterm::execute!(io::stdout(), EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for MouseCaptured {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), DisableMouseCapture);
    }
}

/// What they did: pressed a key, or used the mouse.
enum Input {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

/// What they do next, `None` once a moment passes without anything, so spinners keep turning.
async fn next_input() -> Result<Option<Input>> {
    if !tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(80))).await?? {
        return Ok(None);
    }
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(Some(Input::Key(key))),
        // The mouse moving with no button held changes nothing.
        Event::Mouse(mouse) if mouse.kind != MouseEventKind::Moved => Ok(Some(Input::Mouse(mouse))),
        _ => Ok(None),
    }
}

/// The next key pressed, `None` once a moment passes without one, so spinners keep turning.
async fn next_key() -> Result<Option<KeyEvent>> {
    let input = next_input().await?;
    Ok(input.and_then(|input| match input {
        Input::Key(key) => Some(key),
        Input::Mouse(_) => None,
    }))
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
    /// The shells shell history hooks into: a list that opens under it.
    Shells,
    /// One of [`HOOKABLE`], in the open list of shells.
    ShellPick(usize),
    Output,
    Threads,
    AgentHistory,
    /// The agents setup hooks into and connects: a list that opens under agent shell history.
    Agents,
    /// One of [`Choose::agents`], in the open list of agents.
    AgentPick(usize),
    /// Advanced keybindings: a fold of [`Self::CtrlR`] and [`Self::UpArrow`] at the bottom of the
    /// features.
    Keys,
    CtrlR,
    UpArrow,
    /// A place to sync to, on the sync page.
    SyncTo(SyncTo),
    /// The address of their own server, under [`SyncTo::SelfHosted`] once it's picked.
    Server,
    /// Whether Atuin Hub is on, and what it uploads, on the hub page.
    Sharing(Sharing),
    /// Whether Atuin Hub uploads what Atuin has already, under [`Sharing::Always`] once it's
    /// picked.
    Backfill,
    /// The button that goes on to set it all up.
    Go,
}

impl Row {
    /// Every row but the entries of a list, in the order the screen shows them.
    const ORDER: [Self; 16] = [
        Self::Shell,
        Self::Shells,
        Self::Output,
        Self::Agents,
        Self::AgentHistory,
        Self::Threads,
        Self::Keys,
        Self::SyncTo(SyncTo::Cloud),
        Self::SyncTo(SyncTo::SelfHosted),
        Self::Server,
        Self::SyncTo(SyncTo::Off),
        Self::Sharing(Sharing::Always),
        Self::Backfill,
        Self::Sharing(Sharing::Workspaces),
        Self::Sharing(Sharing::Off),
        Self::Go,
    ];

    /// The page the row is on, `None` for the button that's on each.
    const fn page(self) -> Option<Page> {
        match self {
            Self::SyncTo(_) | Self::Server => Some(Page::Sync),
            Self::Sharing(_) | Self::Backfill => Some(Page::Hub),
            Self::Go => None,
            Self::Shell
            | Self::Shells
            | Self::ShellPick(_)
            | Self::Output
            | Self::Threads
            | Self::AgentHistory
            | Self::Agents
            | Self::AgentPick(_)
            | Self::Keys
            | Self::CtrlR
            | Self::UpArrow => Some(Page::Features),
        }
    }

    /// The list the row is or is in, if any.
    const fn list(self) -> Option<List> {
        match self {
            Self::Shells | Self::ShellPick(_) => Some(List::Shells),
            Self::Agents | Self::AgentPick(_) => Some(List::Agents),
            Self::Keys | Self::CtrlR | Self::UpArrow => Some(List::Keys),
            Self::Shell
            | Self::Output
            | Self::Threads
            | Self::AgentHistory
            | Self::SyncTo(_)
            | Self::Server
            | Self::Sharing(_)
            | Self::Backfill
            | Self::Go => None,
        }
    }

    /// Whether the row opens a list under it.
    fn opens(self) -> bool {
        self.list().is_some_and(|list| list.row() == self)
    }

    /// Whether the screen shows the row for `plan`, `None` when it doesn't, and if it does, what
    /// the row needs while the rest of the screen rules it out.
    fn state(self, plan: &Plan) -> Option<Result<(), Needs>> {
        match self {
            Self::Shell => plan.ask::<ShellHistory>(),
            Self::Shells | Self::ShellPick(_) => plan.ask::<Shells>(),
            Self::Agents | Self::AgentPick(_) => plan.ask::<Agents>(),
            Self::Output => plan.ask::<OutputCapture>(),
            Self::Threads => plan.ask::<Threads>(),
            Self::AgentHistory => plan.ask::<AgentHistory>(),
            // It opens while either key can change.
            Self::Keys => {
                let keys = [plan.ask::<CtrlR>(), plan.ask::<UpArrow>()];
                keys.into_iter().flatten().reduce(Result::or)
            }
            Self::CtrlR => plan.ask::<CtrlR>(),
            Self::UpArrow => plan.ask::<UpArrow>(),
            Self::SyncTo(_) => SyncTo::state(plan),
            Self::Server => plan.ask::<Sync>().filter(|_| matches!(plan.sync, Sync::Server(_))),
            Self::Sharing(sharing) => sharing.state(plan),
            Self::Backfill => plan.ask::<Backfill>().filter(|_| Sharing::Always.is_picked(plan)),
            Self::Go => Some(Ok(())),
        }
    }

    /// The row's name.
    const fn label(self) -> &'static str {
        match self {
            // The agents' too, under Agent integration.
            Self::Shell | Self::AgentHistory => "Shell history",
            Self::Shells => "Shells",
            Self::ShellPick(_) => "Shell",
            Self::Agents => "Connected agents",
            Self::AgentPick(_) => "Agent",
            Self::Output => "Command output",
            Self::Threads => "Threads",
            Self::Keys => "Advanced keybindings",
            Self::CtrlR => "Ctrl-R",
            Self::UpArrow => "Up arrow",
            Self::SyncTo(to) => to.label(),
            Self::Server => "Address",
            Self::Sharing(sharing) => sharing.label(),
            Self::Backfill => "Upload what Atuin has already",
            Self::Go => "Continue",
        }
    }

    /// The question the row comes under. The keys come under their fold's row instead.
    const fn heading(self) -> Option<&'static str> {
        match self {
            Self::Shell | Self::Shells | Self::ShellPick(_) | Self::Output => {
                Some("What should Atuin capture?")
            }
            Self::Agents | Self::AgentPick(_) | Self::AgentHistory | Self::Threads => {
                Some("Agent integration")
            }
            Self::SyncTo(_) | Self::Server => Some("Where should Atuin sync?"),
            Self::Sharing(_) | Self::Backfill => Some("Atuin Hub"),
            Self::Keys | Self::CtrlR | Self::UpArrow | Self::Go => None,
        }
    }
}

/// A place the sync page offers to sync to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncTo {
    Cloud,
    SelfHosted,
    Off,
}

impl SyncTo {
    const fn label(self) -> &'static str {
        match self {
            Self::Cloud => "Atuin Cloud",
            Self::SelfHosted => "Self-hosted Atuin",
            Self::Off => "Disabled",
        }
    }

    /// What syncing there means, if it needs saying.
    const fn about(self) -> Option<&'static str> {
        match self {
            Self::Cloud => Some("Atuin's official server."),
            Self::SelfHosted => Some("E2E sync through a self-hosted Atuin server you run."),
            Self::Off => None,
        }
    }

    fn state(plan: &Plan) -> Option<Result<(), Needs>> {
        plan.ask::<Sync>()
    }

    /// Whether it's where `sync` syncs to.
    fn is_where(self, sync: &Sync) -> bool {
        match self {
            Self::Cloud => *sync == Sync::Cloud,
            Self::SelfHosted => matches!(sync, Sync::Server(_)),
            Self::Off => *sync == Sync::Off,
        }
    }
}

/// Atuin Hub with `--hub`: on for every thread or for those started in a workspace, or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sharing {
    Always,
    Workspaces,
    Off,
}

impl Sharing {
    const fn label(self) -> &'static str {
        match self {
            Self::Always => "Always enabled",
            Self::Workspaces => "Workspace-only",
            Self::Off => "Disabled",
        }
    }

    const fn about(self) -> &'static str {
        match self {
            Self::Always => "uploads all your threads",
            Self::Workspaces => "uploads threads in directories with a .atuin.lua file",
            Self::Off => "disable Atuin Hub",
        }
    }

    /// Whether the hub page offers it, as [`Row::state`] says: with `--hub`, once Atuin Cloud is
    /// picked.
    fn state(self, plan: &Plan) -> Option<Result<(), Needs>> {
        let teams = plan.ask::<AtuinForTeams>()?;
        let shares = match self {
            Self::Always | Self::Workspaces => teams,
            Self::Off => Ok(()),
        };
        (plan.sync == Sync::Cloud).then_some(shares)
    }

    fn is_picked(self, plan: &Plan) -> bool {
        let threads = plan.upload_threads.0;
        match self {
            Self::Always => plan.atuin_for_teams.0 && threads == ThreadUploads::All,
            Self::Workspaces => plan.atuin_for_teams.0 && threads == ThreadUploads::Workspaces,
            Self::Off => !plan.atuin_for_teams.0,
        }
    }
}

/// A list of the first screen, which opens under its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum List {
    Shells,
    Agents,
    /// The keys that open Atuin's search, folded away since few change them.
    Keys,
}

impl List {
    /// The row it opens under.
    const fn row(self) -> Row {
        match self {
            Self::Shells => Row::Shells,
            Self::Agents => Row::Agents,
            Self::Keys => Row::Keys,
        }
    }
}

/// A page of the first screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    /// What Atuin captures, who searches it, and with which keys.
    Features,
    /// Where it syncs to, after the features since sign-in follows it.
    Sync,
    /// Whether Atuin Hub uploads what Atuin Cloud syncs.
    Hub,
}

impl Page {
    const ORDER: [Self; 3] = [Self::Features, Self::Sync, Self::Hub];

    const fn name(self) -> &'static str {
        match self {
            Self::Features => "Features",
            Self::Sync => "Sync",
            Self::Hub => "Hub",
        }
    }
}

enum Chosen {
    Plan(Plan),
    Quit,
}

/// The first screen: what to set up, over its pages.
struct Choose {
    plan: Plan,
    /// The address typed for their own server, which the plan takes once they continue.
    server: String,
    row: Row,
    error: Option<&'static str>,
    /// Whether the screen may colour a row the rest rules out, which it strikes through otherwise.
    colour: bool,
    /// The list that's open, if any.
    open: Option<List>,
    /// The agents the list of agents offers: those here, and any the flags named.
    agents: Vec<AnyHarness>,
    page: Page,
    /// The first line shown, while they've scrolled with the mouse. A key press brings the row
    /// they're on back into sight.
    scrolled: Option<usize>,
    /// Where they hold the scrollbar's handle, in rows from its top, while they drag it.
    held: Option<usize>,
    /// The background of the row they're on, `None` without colour.
    shade: Option<Color>,
    /// Whether Atuin is set up here already, so that turning it all off still does something:
    /// takes it out.
    set_up_here: bool,
}

impl Choose {
    fn new(found: &Found, plan: &Plan) -> Self {
        let server = match &plan.sync {
            Sync::Server(address) => Some(address.clone()),
            Sync::Cloud | Sync::Off => found.server.clone(),
        };
        // With those Atuin is in, which can be taken out of one that isn't here now.
        let mut agents = found.agents();
        let connected = found.connected();
        for &agent in plan.agents.0.iter().chain(&connected) {
            if !agents.iter().any(|here| here.name() == agent.name()) {
                agents.push(agent);
            }
        }
        let mut plan = plan.clone();
        // The hub page offers Atuin for Teams only with Atuin Cloud, where it can be on.
        let teams = matches!(plan.ask::<AtuinForTeams>(), Some(Ok(())));
        plan.atuin_for_teams.0 &= teams && plan.sync == Sync::Cloud;
        Self {
            plan,
            server: server.unwrap_or_default(),
            row: Row::Shell,
            error: None,
            colour: true,
            open: None,
            agents,
            page: Page::Features,
            scrolled: None,
            held: None,
            shade: None,
            set_up_here: found.hooked
                || !found.inits.is_empty()
                || !found.connections.is_empty()
                || found.capture
                || found.output
                || found.syncing().is_some(),
        }
    }

    #[must_use]
    fn coloured(self, colour: bool) -> Self {
        Self { colour, ..self }
    }

    #[must_use]
    fn shaded(self, shade: Option<Color>) -> Self {
        Self { shade, ..self }
    }

    /// The rows the page shows, each with what it needs while the rest rules it out.
    fn rows(&self) -> Vec<(Row, Option<Needs>)> {
        let mut rows = Vec::new();
        for row in Row::ORDER {
            if row.page().is_some_and(|page| page != self.page) {
                continue;
            }
            let Some(state) = row.state(&self.plan) else {
                continue;
            };
            let open = state.is_ok() && row.list().is_some() && row.list() == self.open;
            rows.push((row, state.err()));
            let picks = match row {
                Row::Shells if open => (0..HOOKABLE.len()).map(Row::ShellPick).collect(),
                Row::Agents if open => (0..self.agents.len()).map(Row::AgentPick).collect(),
                Row::Keys if open => vec![Row::CtrlR, Row::UpArrow],
                _ => Vec::new(),
            };
            let shown = picks.into_iter().filter_map(|pick| Some((pick, pick.state(&self.plan)?)));
            rows.extend(shown.map(|(pick, state)| (pick, state.err())));
        }
        rows
    }

    /// What they chose, once they have.
    fn handle(&mut self, key: KeyEvent) -> Option<Chosen> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Chosen::Quit);
        }
        self.error = None;
        self.scrolled = None;
        // The cursor passes over a row the rest rules out: there's nothing to change on it.
        let rows: Vec<_> = self
            .rows()
            .into_iter()
            .filter(|(_, needs)| needs.is_none())
            .map(|(row, _)| row)
            .collect();
        let at = rows.iter().position(|&row| row == self.row).unwrap_or(0);

        match key.code {
            KeyCode::Enter if self.row == Row::Go => return self.next(),
            KeyCode::Enter if self.row == Row::Server => self.row = Row::Go,
            KeyCode::Backspace if self.row == Row::Server => {
                self.server.pop();
            }
            KeyCode::Char(c) if self.row == Row::Server => self.server.push(c),
            KeyCode::Esc | KeyCode::Backspace if self.page != Page::Features => {
                let pages = self.pages();
                let before = pages.iter().rev().skip_while(|&&page| page != self.page).nth(1);
                self.page = before.copied().unwrap_or(Page::Features);
                self.row = Row::Go;
            }
            KeyCode::Esc | KeyCode::Char('q') => return Some(Chosen::Quit),
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                self.row = rows[at.saturating_sub(1)];
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                self.row = rows[(at + 1).min(rows.len() - 1)];
            }
            KeyCode::Right | KeyCode::Char('l') if self.row.opens() => self.open = self.row.list(),
            KeyCode::Left | KeyCode::Char('h') if self.row.list().is_some() => {
                if let Some(list) = self.row.list() {
                    self.open = None;
                    self.row = list.row();
                }
            }
            KeyCode::Enter | KeyCode::Char(' ' | 'l' | 'h') | KeyCode::Right | KeyCode::Left => {
                self.change();
            }
            _ => {}
        }
        None
    }

    /// Drag the scrollbar's handle, or scroll with the wheel, on a terminal `size` big.
    fn mouse(&mut self, event: MouseEvent, found: &Found, size: Size) {
        /// As far as a notch of the wheel scrolls in most terminals.
        const WHEEL_LINES: usize = 3;

        if event.kind == MouseEventKind::Up(MouseButton::Left) {
            self.held = None;
            return;
        }
        let (lines, focus) = self.lines(found);
        let keep = focus.map_or(0, |focus| focus.end);
        let Some(scroll) = self.scroll(lines.len(), keep, size.height) else {
            return;
        };
        let row = usize::from(event.row);
        let on_bar = event.column == size.width.saturating_sub(1) && row < scroll.height();
        let scroll = match event.kind {
            MouseEventKind::Down(MouseButton::Left) if on_bar => {
                let handle = scroll.handle();
                // By where they took it, or by its middle when they clicked the bar beside it.
                let held = if handle.contains(&row) {
                    row - handle.start
                } else {
                    handle.len() / 2
                };
                self.held = Some(held);
                scroll.handle_at(row.saturating_sub(held))
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(held) = self.held else {
                    return;
                };
                scroll.handle_at(row.saturating_sub(held))
            }
            MouseEventKind::ScrollDown => scroll.down(WHEEL_LINES),
            MouseEventKind::ScrollUp => scroll.up(WHEEL_LINES),
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => return,
        };
        self.scrolled = Some(scroll.top());
    }

    /// Toggle the row, or pick it.
    fn change(&mut self) {
        let plan = &mut self.plan;
        match self.row {
            Row::Shell => plan.shell.0 = !plan.shell.0,
            // All of it, unless it's all already: then none of it.
            Row::Shells => {
                let all = plan.shells.0.len() == HOOKABLE.len();
                plan.shells.0 = if all {
                    Vec::new()
                } else {
                    HOOKABLE.to_vec()
                };
            }
            Row::Agents => {
                let all = plan.agents.0.len() == self.agents.len();
                plan.agents.0 = if all {
                    Vec::new()
                } else {
                    self.agents.clone()
                };
            }
            Row::ShellPick(pick) => {
                plan.shells.0 = picked(&HOOKABLE, &plan.shells.0, pick, |a, b| a == b);
            }
            Row::AgentPick(pick) => {
                let same = |a: &AnyHarness, b: &AnyHarness| a.name() == b.name();
                plan.agents.0 = picked(&self.agents, &plan.agents.0, pick, same);
            }
            Row::Output => plan.output.0 = !plan.output.0,
            Row::Threads => plan.threads.0 = !plan.threads.0,
            Row::AgentHistory => plan.agent_history.0 = !plan.agent_history.0,
            Row::Keys => {
                self.open = if self.open == Some(List::Keys) {
                    None
                } else {
                    Some(List::Keys)
                };
            }
            Row::CtrlR => plan.ctrl_r.0 = !plan.ctrl_r.0,
            Row::UpArrow => plan.up_arrow.0 = !plan.up_arrow.0,
            Row::SyncTo(to) => self.pick(to),
            Row::Sharing(sharing) => self.share(sharing),
            Row::Backfill => plan.backfill.0 = !plan.backfill.0,
            Row::Server | Row::Go => {}
        }
    }

    /// Sync to `to`, which with their own server goes on to its address.
    fn pick(&mut self, to: SyncTo) {
        let moved = !to.is_where(&self.plan.sync);
        self.plan.sync = match to {
            SyncTo::Cloud => Sync::Cloud,
            SyncTo::SelfHosted => Sync::Server(self.server.clone()),
            SyncTo::Off => Sync::Off,
        };
        // Atuin for Teams comes only with Atuin Cloud, which `--hub` shares from as it can.
        if moved {
            let teams = matches!(self.plan.ask::<AtuinForTeams>(), Some(Ok(())));
            self.plan.atuin_for_teams.0 = to == SyncTo::Cloud && teams;
        }
        if to == SyncTo::SelfHosted {
            self.row = Row::Server;
        }
    }

    /// Share threads from Atuin Cloud as `sharing` says.
    const fn share(&mut self, sharing: Sharing) {
        match sharing {
            Sharing::Always => {
                self.plan.atuin_for_teams.0 = true;
                self.plan.upload_threads.0 = ThreadUploads::All;
            }
            Sharing::Workspaces => {
                self.plan.atuin_for_teams.0 = true;
                self.plan.upload_threads.0 = ThreadUploads::Workspaces;
            }
            Sharing::Off => self.plan.atuin_for_teams.0 = false,
        }
    }

    /// Go on from the page they're on: to the next page with something to ask, or with the plan.
    fn next(&mut self) -> Option<Chosen> {
        let pages = self.pages();
        let after = pages.iter().skip_while(|&&page| page != self.page).nth(1).copied();
        let plan = self.plan(after.is_none())?;
        let Some(page) = after else {
            return Some(Chosen::Plan(plan));
        };
        self.page = page;
        let first = self.rows().into_iter().find(|(_, needs)| needs.is_none());
        self.row = first.map_or(Row::Go, |(row, _)| row);
        None
    }

    /// The pages with something to ask, in order.
    fn pages(&self) -> Vec<Page> {
        let asks = |page| {
            let on_page = Row::ORDER.into_iter().filter(|row| row.page() == Some(page));
            on_page.into_iter().any(|row| matches!(row.state(&self.plan), Some(Ok(()))))
        };
        Page::ORDER.into_iter().filter(|&page| asks(page)).collect()
    }

    /// The pages, the one they're on bold, when there's more than one.
    fn crumbs(&self) -> Vec<Span<'static>> {
        let pages = self.pages();
        if pages.len() < 2 {
            return Vec::new();
        }
        let mut spans = vec![Span::raw("   ")];
        for (i, page) in pages.into_iter().enumerate() {
            if i > 0 {
                spans.push(dim(" › "));
            }
            spans.push(if page == self.page {
                Span::raw(page.name()).bold()
            } else {
                dim(page.name())
            });
        }
        spans
    }

    /// The plan, if setup can set it up, or `None` with an error on the row to change. Sync, and
    /// whether there's anything to set up at all, wait until they're `done`, on the last page.
    fn plan(&mut self, done: bool) -> Option<Plan> {
        if done && matches!(self.plan.sync, Sync::Server(_)) {
            let address = self.server.trim();
            if !is_address(address) {
                self.error = Some("Enter your server's address, starting http:// or https://");
                self.focus(Row::Server);
                return None;
            }
            self.plan.sync = Sync::Server(address.to_string());
        }
        let plan = self.plan.effective();
        if plan.shell.0 && plan.shells.0.is_empty() {
            self.error = Some("Choose a shell to hook into, or turn shell history off");
            self.focus(Row::Shells);
            return None;
        }
        let chosen =
            [plan.shell.0, plan.output.0, plan.threads.0, plan.agent_history.0, plan.mcp.0];
        if done && !self.set_up_here && !chosen.contains(&true) && plan.sync == Sync::Off {
            self.error = Some("Choose something to set up, or press ctrl-c to leave");
            return None;
        }
        Some(plan)
    }

    /// Put the cursor on `row`, on its page.
    fn focus(&mut self, row: Row) {
        self.page = row.page().unwrap_or(self.page);
        self.row = row;
    }

    fn draw(&self, frame: &mut Frame<'_>, found: &Found) {
        let (mut lines, focus) = self.lines(found);
        let focused = focus.as_ref().map(|focus| focus.start);
        let area = frame.area();
        let footer = self.footer();
        if fits(lines.len(), area.height) {
            lines.extend([Line::default(), footer]);
            frame.render_widget(Paragraph::new(lines), area);
            self.shade(frame, area, focused);
            return;
        }

        // The page needs all of the terminal, so it scrolls above the footer, which stays at the
        // bottom.
        let [body, bottom] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        frame.render_widget(Paragraph::new(footer), bottom);
        let keep = focus.map_or(0, |focus| focus.end);
        let Some(scroll) = self.scroll(lines.len(), keep, area.height) else {
            frame.render_widget(Paragraph::new(lines), body);
            self.shade(frame, body, focused);
            return;
        };
        let [page, bar] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).areas(body);
        let top = u16::try_from(scroll.top()).unwrap_or(u16::MAX);
        frame.render_widget(Paragraph::new(lines).scroll((top, 0)), page);
        frame.render_widget(Paragraph::new(scroll.bar(self.held.is_some())), bar);
        self.shade(frame, page, focused.and_then(|line| line.checked_sub(scroll.top())));
    }

    /// Shade the row they're on, `line` lines down `area`, all the way across it but where it has
    /// a background of its own, as the button does.
    fn shade(&self, frame: &mut Frame<'_>, area: Rect, line: Option<usize>) {
        let (Some(shade), Some(line)) = (self.shade, line) else {
            return;
        };
        let Ok(line) = u16::try_from(line) else {
            return;
        };
        if line < area.height {
            let row = Rect {
                y: area.y + line,
                height: 1,
                ..area
            };
            let buf = frame.buffer_mut();
            for at in row.positions() {
                if let Some(cell) = buf.cell_mut(at)
                    && cell.bg == Color::Reset
                {
                    cell.set_bg(shade);
                }
            }
        }
    }

    /// How far down a page `lines` long is scrolled on a terminal `height` tall, if it doesn't
    /// fit above the footer: where they've scrolled it to, or just far enough to show its first
    /// `keep` lines.
    fn scroll(&self, lines: usize, keep: usize, height: u16) -> Option<Scroll> {
        if fits(lines, height) {
            return None;
        }
        // All but the footer's row.
        let height = usize::from(height).saturating_sub(1);
        let top = self.scrolled.unwrap_or_else(|| keep.saturating_sub(height));
        Scroll::new(lines, height, top)
    }

    /// The page's lines, and those of the row they're on: its own, then any description under it.
    fn lines(&self, found: &Found) -> (Vec<Line<'static>>, Option<Range<usize>>) {
        let mut lines = title(self.crumbs());
        if self.page == Page::Features {
            lines.extend(found_lines(found));
        }

        let mut heading = None;
        let mut previous: Option<Row> = None;
        let mut focus = None;
        for (i, (row, needs)) in self.rows().into_iter().enumerate() {
            // The button stands apart from the keys too, which have no heading.
            if (row.heading() != heading || row == Row::Go) && row != Row::Server {
                heading = row.heading();
                if i > 0 {
                    lines.push(Line::default());
                }
                if let Some(heading) = heading {
                    lines.push(Line::from(format!("  {heading}").bold()));
                }
            }
            // The server's address stands apart from what self-hosting means.
            if row == Row::Server && matches!(previous, Some(Row::SyncTo(_))) {
                lines.push(Line::default());
            }
            // What Atuin Hub is, before the ways to have it.
            if row == Row::Sharing(Sharing::Always) {
                let about = "Atuin Hub allows you to share your threads, visualize stats, read \
                             your threads in the browser, and avoid storing your data on your \
                             computer. It is not E2E encrypted.";
                lines.extend(paragraph(about, UNDER_ARROW, Style::new()));
                lines.push(Line::default());
            }
            let at = row == self.row;
            let line = |needs| ruled_out(row, needs, self.colour);
            let start = lines.len();
            lines.push(needs.map_or_else(|| self.line(row, at, found), line));
            if let Row::SyncTo(to) = row
                && let Some(about) = to.about()
            {
                lines.extend(paragraph(about, UNDER_LABEL, Style::new().dim()));
            }
            if at {
                focus = Some(start..lines.len());
            }
            previous = Some(row);
        }
        (lines, focus)
    }

    /// The line under the page: what's wrong, or the keys they can press.
    fn footer(&self) -> Line<'static> {
        let esc = if self.page == Page::Features {
            ("esc", "quit")
        } else {
            ("esc", "back")
        };
        match (self.error, self.row) {
            (Some(error), _) => Line::from(Span::raw(format!("  {error}")).yellow()),
            (None, Row::Go) => hints(&[("enter", "continue"), ("↑↓", "move"), esc]),
            (None, Row::Server) => hints(&[("enter", "next"), esc]),
            (None, Row::SyncTo(_) | Row::Sharing(_)) => {
                hints(&[("enter", "pick"), ("↑↓", "move"), esc])
            }
            (None, Row::ShellPick(_) | Row::AgentPick(_) | Row::CtrlR | Row::UpArrow) => {
                hints(&[("enter", "toggle"), ("←", "close"), ("↑↓", "move")])
            }
            (None, Row::Shells | Row::Agents) if self.open == self.row.list() => {
                hints(&[("enter", "toggle all"), ("←", "close"), ("↑↓", "move")])
            }
            (None, Row::Shells | Row::Agents) => {
                hints(&[("enter", "toggle all"), ("→", "open"), ("↑↓", "move")])
            }
            (None, Row::Keys) if self.open == Some(List::Keys) => {
                hints(&[("enter", "close"), ("↑↓", "move"), esc])
            }
            (None, Row::Keys) => hints(&[("enter", "open"), ("↑↓", "move"), esc]),
            (None, Row::Shell | Row::Output | Row::Threads | Row::AgentHistory | Row::Backfill) => {
                hints(&[("enter", "toggle"), ("↑↓", "move"), esc])
            }
        }
    }

    /// Whether `row`'s list is open, as the arrow left of it shows.
    fn fold(&self, row: Row) -> Fold {
        if self.open == row.list() {
            Fold::Open
        } else {
            Fold::Closed
        }
    }

    /// The row of a list, `at` when it's the one they're on: what's picked of it, and whether it's
    /// open.
    fn list(&self, at: bool, row: Row, picked: &[&str]) -> Line<'static> {
        let offered = match row {
            Row::Agents => self.agents.len(),
            _ => HOOKABLE.len(),
        };
        let check = match picked.len() {
            0 => Check::Off,
            n if n >= offered => Check::On,
            _ => Check::Partly,
        };
        let fold = self.fold(row);
        let summary = if picked.is_empty() {
            "none".to_owned()
        } else {
            names(picked)
        };
        let about = vec![dim(summary)];
        toggle(at, Some(fold), Some(check), !picked.is_empty(), row.label(), about)
    }

    /// The line `row` is drawn as, `at` when it's the one they're on.
    fn line(&self, row: Row, at: bool, found: &Found) -> Line<'static> {
        match row {
            Row::Shell => choice(at, self.plan.shell.0, row.label(), "all your commands"),
            Row::Shells => {
                let picked: Vec<_> = self.plan.shells.0.iter().map(|shell| shell.name()).collect();
                self.list(at, row, &picked)
            }
            Row::ShellPick(pick) => {
                let shell = HOOKABLE[pick];
                entry(at, self.plan.shells.0.contains(&shell), shell.name())
            }
            Row::Agents => {
                let picked: Vec<_> = self.plan.agents.0.iter().map(Harness::display_name).collect();
                self.list(at, row, &picked)
            }
            Row::AgentPick(pick) => {
                let agent = self.agents[pick];
                let on = self.plan.agents.0.iter().any(|picked| picked.name() == agent.name());
                entry(at, on, agent.display_name())
            }
            Row::Output => choice(at, self.plan.output.0, row.label(), "what your commands print"),
            Row::Threads => choice(
                at,
                self.plan.threads.0,
                row.label(),
                "capture full agent sessions, resumable across harnesses",
            ),
            Row::AgentHistory => {
                choice(at, self.plan.agent_history.0, row.label(), "capture your agents' commands")
            }
            // A fold, with no checkbox since there's nothing to choose on it. It heads the rows it
            // opens to, so it starts a level out from them, without their margin.
            Row::Keys => {
                let [_margin, cursor, arrow] = lead(at, Some(self.fold(row)));
                Line::from(vec![cursor, arrow, pad(row.label(), at)])
            }
            Row::CtrlR => {
                key_choice(at, self.plan.ctrl_r.0, row.label(), "opens Atuin's history search")
            }
            Row::UpArrow => key_choice(
                at,
                self.plan.up_arrow.0,
                row.label(),
                "opens the search on your last command",
            ),
            Row::SyncTo(to) => {
                let signed_in = found.syncing().is_some_and(|syncing| to.is_where(&syncing));
                let picked = to.is_where(&self.plan.sync);
                option(at, picked, to.label(), signed_in.then_some("signed in"))
            }
            Row::Sharing(sharing) => {
                let picked = sharing.is_picked(&self.plan);
                option(at, picked, sharing.label(), Some(sharing.about()))
            }
            Row::Backfill => entry(at, self.plan.backfill.0, row.label()),
            Row::Go => button(at, row.label()),
            Row::Server => {
                let mut spans = lead(at, None).to_vec();
                spans.extend([Span::raw("    "), pad(row.label(), at)]);
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
        }
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
        let mut lines = title(Vec::new());
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

/// "atuin setup", then `after` on the same line.
fn title(after: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let mut title = vec![Span::raw("  "), "atuin".fg(ACCENT).bold(), " setup".bold()];
    title.extend(after);
    vec![Line::default(), Line::from(title), Line::default()]
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
        let mut lines = title(Vec::new());
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
    let check = if on {
        Check::On
    } else {
        Check::Off
    };
    toggle(at, None, Some(check), on, label, vec![dim(about)])
}

/// One of a choice of a few: a radio button, its name, and what it says about it.
fn option(
    at: bool,
    picked: bool,
    label: &'static str,
    aside: Option<&'static str>,
) -> Line<'static> {
    let radio = if picked {
        "(*) ".fg(ACCENT).bold()
    } else {
        "( ) ".dim()
    };
    let mut spans = lead(at, None).to_vec();
    spans.extend([radio, pad(label, at)]);
    spans.extend(aside.map(dim));
    Line::from(spans)
}

/// A key's row: a toggle, the key, and what it does, or that the shell keeps it.
fn key_choice(at: bool, on: bool, label: &'static str, does: &'static str) -> Line<'static> {
    let check = if on {
        Check::On
    } else {
        Check::Off
    };
    let about = if on {
        does
    } else {
        "left to your shell"
    };
    toggle(at, None, Some(check), on, label, vec![Span::raw(about).italic().dim()])
}

/// A row the rest of the first screen rules out, with what it needs: grey, or without colour,
/// struck through. It's off while ruled out, whatever was chosen for it.
fn ruled_out(row: Row, needs: Needs, colour: bool) -> Line<'static> {
    let (name, aside) = if colour {
        (Style::new().fg(Color::DarkGray), Style::new().fg(Color::Yellow))
    } else {
        (Style::new().add_modifier(Modifier::CROSSED_OUT), Style::new())
    };
    let mark = |text: &'static str| {
        if colour {
            Span::styled(text, name)
        } else {
            text.dim()
        }
    };
    // A list's row keeps its arrow: it can open once it isn't ruled out.
    let arrow = if row.opens() {
        "▸ "
    } else {
        "  "
    };
    let box_ = match row {
        Row::SyncTo(_) | Row::Sharing(_) => "( ) ",
        Row::Keys => "",
        Row::Shell
        | Row::Shells
        | Row::ShellPick(_)
        | Row::Output
        | Row::Threads
        | Row::AgentHistory
        | Row::Agents
        | Row::AgentPick(_)
        | Row::CtrlR
        | Row::UpArrow
        | Row::Server
        | Row::Backfill
        | Row::Go => "[ ] ",
    };
    let indent = if matches!(row, Row::Server | Row::Backfill) {
        NESTED
    } else {
        ""
    };
    // The fold starts a level out, as it does once it can open.
    let margin = if row == Row::Keys {
        ""
    } else {
        "  "
    };
    let label = row.label();
    // Apart from the name, so that the padding isn't struck through. What it needs lines up with
    // the rest's, whatever is left of the name.
    let width = "  ".len() + "[ ] ".len() + LABEL_WIDTH;
    let left = margin.len() + indent.len() + box_.len() + label.len();
    let pad = " ".repeat(width.saturating_sub(left));
    Line::from(vec![
        Span::raw(margin),
        cursor(false),
        mark(arrow),
        Span::raw(indent),
        mark(box_),
        Span::styled(label, name),
        Span::raw(pad),
        Span::styled(format!("needs {}", needs.what), aside),
    ])
}

/// An entry of an open list, under its row's name: a checkbox and its name.
fn entry(at: bool, on: bool, name: &'static str) -> Line<'static> {
    let box_ = if on {
        "[✓] ".fg(ACCENT).bold()
    } else {
        "[ ] ".dim()
    };
    let name = if at {
        Span::raw(name).bold()
    } else {
        Span::raw(name)
    };
    let mut spans = lead(at, None).to_vec();
    spans.extend([Span::raw(NESTED), box_, name]);
    let line = Line::from(spans);
    if on {
        line
    } else {
        line.style(Style::default().add_modifier(Modifier::DIM))
    }
}

/// What's `picked` of `offered` once its entry `pick` is toggled, in the order offered.
fn picked<T: Copy>(
    offered: &[T],
    picked: &[T],
    pick: usize,
    same: impl Fn(&T, &T) -> bool,
) -> Vec<T> {
    let was = |item: &T| picked.iter().any(|picked| same(picked, item));
    let toggled = offered.iter().enumerate().filter(|&(i, item)| (i == pick) != was(item));
    toggled.map(|(_, &item)| item).collect()
}

/// What a checkbox shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Check {
    On,
    /// Some of a list, not all of it.
    Partly,
    Off,
}

/// Whether a list is open, as the arrow left of its row's checkbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fold {
    Open,
    Closed,
}

/// What's left of a row's checkbox: the cursor, if `at` it, and room for a list's arrow.
fn lead(at: bool, fold: Option<Fold>) -> [Span<'static>; 3] {
    let arrow = match fold {
        Some(Fold::Open) => "▾ ",
        Some(Fold::Closed) => "▸ ",
        None => "  ",
    };
    [Span::raw("  "), cursor(at), Span::raw(arrow)]
}

/// A row: a list's arrow, if `fold` is one, a checkbox, if `checked` is one, its name and what it
/// means, dim when not `on`.
fn toggle(
    at: bool,
    fold: Option<Fold>,
    checked: Option<Check>,
    on: bool,
    label: &'static str,
    about: Vec<Span<'static>>,
) -> Line<'static> {
    let box_ = match checked {
        Some(Check::On) => "[✓] ".fg(ACCENT).bold(),
        Some(Check::Partly) => "[-] ".fg(ACCENT).bold(),
        Some(Check::Off) => "[ ] ".dim(),
        None => Span::raw("    "),
    };
    let mut spans = lead(at, fold).to_vec();
    spans.extend([box_, pad(label, at)]);
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
    let mut spans = lead(at, None).to_vec();
    spans.push(text);
    Line::from(spans)
}

/// How wide a row's name is padded to, so that what it means lines up.
const LABEL_WIDTH: usize = 21;

/// Whether a page `lines` long fits a terminal `height` tall, with a line's gap and the footer under
/// it.
fn fits(lines: usize, height: u16) -> bool {
    lines + 2 <= usize::from(height)
}

/// Greys of the 256-colour palette a step off black, and off white, to shade the row they're on.
const SHADE_ON_DARK: Color = Color::Indexed(236);
const SHADE_ON_LIGHT: Color = Color::Indexed(254);

/// The shade under the row they're on, for a terminal whose background `colorfgbg` gives, as
/// `COLORFGBG` does in the terminals that set it. Most don't, so a dark one is taken.
fn row_shade(colorfgbg: Option<&str>) -> Color {
    // "fg;bg", or "fg;default;bg", in the 16 ANSI colours, of which 7 and 15 are white.
    let background = colorfgbg.and_then(|value| value.rsplit(';').next());
    if matches!(background, Some("7" | "15")) {
        SHADE_ON_LIGHT
    } else {
        SHADE_ON_DARK
    }
}

/// Where text that runs on wraps, so that it reads in an 80-column terminal at the deepest indent.
const WRAP_COLUMNS: usize = 62;

/// `text`, wrapped, each line `indent` columns in and in `style`.
fn paragraph(text: &str, indent: usize, style: Style) -> Vec<Line<'static>> {
    let indent = " ".repeat(indent);
    let lines = wrap(text, WRAP_COLUMNS).into_iter();
    lines
        .map(|line| Line::from(vec![Span::raw(indent.clone()), Span::styled(line, style)]))
        .collect()
}

/// `text` in lines at most `columns` wide, broken between words. A word longer than that has a
/// line to itself.
fn wrap(text: &str, columns: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr as _;

    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > columns {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// How much further in an entry under a row starts than the row.
const NESTED: &str = "    ";

/// The column a row's checkbox starts at, past the cursor and a list's arrow.
const UNDER_ARROW: usize = 6;

/// The column a row's name starts at, past the cursor, a list's arrow and the checkbox.
const UNDER_LABEL: usize = 10;

fn pad(label: &'static str, at: bool) -> Span<'static> {
    let label = Span::raw(format!("{label:<LABEL_WIDTH$}"));
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

    use super::super::super::init::Shell;
    #[cfg(feature = "octavo")]
    use super::super::Teams;
    use super::super::plan::{Asks, Backfill, Mcp, UploadThreads};
    use super::super::{SYNC, THREADS};
    use super::*;

    #[fixture]
    fn found() -> Found {
        Found {
            history: Some(("zsh", 48210)),
            imported: 0,
            threads_here: 0,
            agents: vec![(AnyHarness::ClaudeCode(Ccode), 300), (AnyHarness::Codex(Codex), 53)],
            hooked: false,
            inits: Vec::new(),
            shells: vec![Shell::Zsh, Shell::Bash],
            ctrl_r: true,
            up_arrow: true,
            capture: false,
            output: false,
            saves_output: false,
            daemon: false,
            daemon_running: false,
            daemon_off: false,
            signed_in: false,
            server: None,
            upload_threads: ThreadUploads::All,
            connections: Vec::new(),
            teams: false,
        }
    }

    /// The plan `atuin setup` with no flags starts the screen from.
    fn preset(found: &Found) -> Plan {
        super::super::Cmd::parse_from(["setup"]).plan(found, false).unwrap()
    }

    fn press(choose: &mut Choose, keys: &[KeyCode]) -> Option<Chosen> {
        keys.iter().find_map(|&code| choose.handle(KeyEvent::from(code)))
    }

    /// Continue from each page, as they've left it, to the plan.
    fn finish(choose: &mut Choose) -> Plan {
        for _ in Page::ORDER {
            choose.row = Row::Go;
            if let Some(Chosen::Plan(plan)) = press(choose, &[KeyCode::Enter]) {
                return plan;
            }
        }
        panic!("expected a plan, got {:?}", choose.error);
    }

    /// `page`, as Continue gets to it from `plan`.
    fn on_page(found: &Found, plan: &Plan, page: Page) -> Choose {
        let mut choose = Choose::new(found, plan);
        while choose.page != page {
            choose.row = Row::Go;
            assert!(press(&mut choose, &[KeyCode::Enter]).is_none(), "{page:?} isn't asked");
        }
        choose
    }

    /// The first page of `atuin setup` with no flags, on Ctrl-R in the open fold of keys.
    fn on_the_keys(found: &Found) -> Choose {
        let mut choose = Choose::new(found, &preset(found));
        choose.row = Row::Keys;
        press(&mut choose, &[KeyCode::Enter, KeyCode::Down]);
        choose
    }

    /// The plan `atuin setup --hub` starts the screen from.
    #[cfg(feature = "octavo")]
    fn teams(found: &Found) -> Plan {
        super::super::Cmd::parse_from(["setup", "--hub"]).plan(found, false).unwrap()
    }

    /// The text of each row of the screen.
    fn shown(choose: &Choose, found: &Found) -> Vec<String> {
        shown_on(choose, found, 40)
    }

    /// A screen 80 columns wide and `height` rows tall.
    fn screen(choose: &Choose, found: &Found, height: u16) -> Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, height)).unwrap();
        terminal.draw(|frame| choose.draw(frame, found)).unwrap().buffer.clone()
    }

    /// The text of each row of a screen `height` rows tall.
    fn shown_on(choose: &Choose, found: &Found, height: u16) -> Vec<String> {
        let shown = screen(choose, found, height);
        let rows = shown.content().chunks(80);
        rows.map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect()).collect()
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
        assert_eq!(finish(&mut choose), Plan {
            asks: Asks {
                agents: true,
                teams: false,
                searchable: false,
                unhooks: false,
            },
            shell: ShellHistory(true),
            shells: Shells(vec![Shell::Zsh, Shell::Bash]),
            output: OutputCapture(false),
            threads: Threads(THREADS),
            agent_history: AgentHistory(THREADS),
            agents: Agents(found.agents()),
            ctrl_r: CtrlR(true),
            up_arrow: UpArrow(true),
            mcp: Mcp(THREADS),
            daemon: super::super::plan::Daemon(true),
            sync: if SYNC {
                Sync::Cloud
            } else {
                Sync::Off
            },
            atuin_for_teams: AtuinForTeams(false),
            upload_threads: UploadThreads(ThreadUploads::All),
            backfill: Backfill(false),
        });
    }

    #[rstest]
    #[case::features(Page::Features)]
    #[cfg_attr(feature = "sync", case::sync(Page::Sync))]
    fn continue_stays_in_sight_in_a_short_terminal(found: Found, #[case] page: Page) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 12)).unwrap();
        let mut choose = on_page(&found, &preset(&found), page);
        while choose.row != Row::Go {
            press(&mut choose, &[KeyCode::Down]);
        }
        let shown = terminal.draw(|frame| choose.draw(frame, &found)).unwrap().buffer.clone();
        let text: String = shown.content().iter().map(ratatui::buffer::Cell::symbol).collect();
        assert!(text.contains("Continue") && text.contains("enter continue"));
    }

    #[rstest]
    #[case::fits(40, false)]
    #[case::too_short(12, true)]
    fn a_page_taller_than_the_terminal_has_a_scrollbar(
        found: Found,
        #[case] height: u16,
        #[case] expected: bool,
    ) {
        let shown = shown_on(&on_the_keys(&found), &found, height);
        let side: String = shown.iter().filter_map(|row| row.chars().last()).collect();
        assert_eq!(side.contains('█'), expected, "{side}");
    }

    #[rstest]
    #[case::dragging_the_handle(&[
        (MouseEventKind::Down(MouseButton::Left), 0),
        (MouseEventKind::Drag(MouseButton::Left), 7),
        (MouseEventKind::Up(MouseButton::Left), 7),
    ])]
    #[case::clicking_the_bar_under_it(&[
        (MouseEventKind::Down(MouseButton::Left), 6),
        (MouseEventKind::Up(MouseButton::Left), 6),
    ])]
    #[case::the_wheel(&[(MouseEventKind::ScrollDown, 5); 10])]
    fn the_scrollbar_scrolls_until_a_key_is_pressed(
        found: Found,
        #[case] used: &[(MouseEventKind, u16)],
    ) {
        // Short enough that the first row scrolls out of sight whichever rows this build offers.
        const SIZE: Size = Size::new(80, 8);
        let mut choose = Choose::new(&found, &preset(&found));
        for &(kind, row) in used {
            let event = MouseEvent {
                kind,
                column: 79,
                row,
                modifiers: KeyModifiers::NONE,
            };
            choose.mouse(event, &found, SIZE);
        }
        let shows = |choose: &Choose, text| {
            shown_on(choose, &found, SIZE.height).iter().any(|row| row.contains(text))
        };
        assert!(shows(&choose, "Continue") && !shows(&choose, "all your commands"));
        // On the first row already, so Up only brings it back.
        press(&mut choose, &[KeyCode::Up]);
        assert!(shows(&choose, "all your commands"));
    }

    #[rstest]
    #[case::at_the_top(0)]
    #[case::at_the_bottom(usize::MAX)]
    fn the_footer_stays_at_the_bottom_as_the_page_scrolls(found: Found, #[case] top: usize) {
        let mut choose = on_the_keys(&found);
        choose.scrolled = Some(top);
        let shown = shown_on(&choose, &found, 12);
        assert!(shown.last().is_some_and(|row| row.contains("enter toggle")), "{shown:#?}");
    }

    #[rstest]
    fn the_footer_follows_a_page_that_fits(found: Found) {
        let shown = shown_on(&on_the_keys(&found), &found, 40);
        let continues = shown.iter().position(|row| row.contains("Continue")).expect("shown");
        assert!(shown[continues + 2].contains("enter toggle"), "{shown:#?}");
    }

    #[rstest]
    #[case::fits(40)]
    #[case::scrolled(12)]
    fn the_row_they_are_on_is_shaded(found: Found, #[case] height: u16) {
        let choose = on_the_keys(&found).shaded(Some(SHADE_ON_DARK));
        let shown = screen(&choose, &found, height);
        let rows = shown_on(&choose, &found, height);
        let y = rows.iter().position(|row| row.contains("Ctrl-R")).expect("the row is shown");
        let bg = |x: u16, y: usize| shown[(x, u16::try_from(y).unwrap())].bg;
        assert_eq!([bg(0, y), bg(70, y), bg(0, y + 1)], [
            SHADE_ON_DARK,
            SHADE_ON_DARK,
            Color::Reset
        ]);
    }

    #[rstest]
    fn the_shade_leaves_the_button_its_own(found: Found) {
        let mut choose = on_the_keys(&found).shaded(Some(SHADE_ON_DARK));
        choose.row = Row::Go;
        let shown = screen(&choose, &found, 40);
        let rows = shown_on(&choose, &found, 40);
        let y = rows.iter().position(|row| row.contains("Continue")).expect("the button is shown");
        let x = rows[y].chars().position(|c| c == 'C').expect("the button's label");
        let cell = |x: usize| &shown[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())];
        assert_eq!((cell(x).fg, cell(x).bg, cell(0).bg), (Color::Black, ACCENT, SHADE_ON_DARK));
    }

    #[rstest]
    #[case::unset(None, SHADE_ON_DARK)]
    #[case::dark("15;0", SHADE_ON_DARK)]
    #[case::light("0;15", SHADE_ON_LIGHT)]
    #[case::light_with_default("0;default;7", SHADE_ON_LIGHT)]
    #[case::unreadable("light", SHADE_ON_DARK)]
    fn the_shade_suits_the_terminal(
        #[case] colorfgbg: impl Into<Option<&'static str>>,
        #[case] expected: Color,
    ) {
        assert_eq!(row_shade(colorfgbg.into()), expected);
    }

    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    fn the_agents_have_a_section_of_their_own(found: Found) {
        let shown = shown(&Choose::new(&found, &preset(&found)), &found);
        // Output capture needs the pty proxy, which runs only on unix.
        let output = cfg!(all(feature = "pty-proxy", unix));
        let order = [
            "What should Atuin capture?",
            "Command output",
            "Agent integration",
            "Connected agents",
            "capture your agents' commands",
            "capture full agent sessions",
        ];
        let order = order.into_iter().filter(|&text| output || text != "Command output");
        let at: Vec<_> =
            order.map(|text| shown.iter().position(|row| row.contains(text))).collect();
        assert!(at.is_sorted() && !at.contains(&None), "{shown:#?}");
    }

    #[rstest]
    fn no_agents_no_agent_rows(mut found: Found) {
        found.agents.clear();
        let choose = Choose::new(&found, &preset(&found));
        let rows: Vec<_> = choose.rows().into_iter().map(|(row, _)| row).collect();
        assert!(![Row::Threads, Row::AgentHistory].iter().any(|row| rows.contains(row)));
        let plan = &choose.plan;
        assert!(!plan.threads.0 && !plan.agent_history.0 && !plan.mcp.0);
    }

    /// What `choose` shows of each of `rows`, and what each needs while it's ruled out.
    fn states(choose: &Choose, rows: &[Row]) -> Vec<(Row, Option<&'static str>)> {
        let shown = choose.rows().into_iter().filter(|(row, _)| rows.contains(row));
        shown.map(|(row, needs)| (row, needs.map(|needs| needs.what))).collect()
    }

    #[rstest]
    fn a_list_opens_under_its_row_and_takes_what_is_picked(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.row = Row::Shells;
        press(&mut choose, &[KeyCode::Right]);
        let picks = [Row::ShellPick(0), Row::ShellPick(1), Row::ShellPick(2)];
        assert_eq!(states(&choose, &picks), picks.map(|pick| (pick, None)));

        press(&mut choose, &[KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(choose.plan.shells.0, [Shell::Zsh, Shell::Bash, Shell::Fish]);
        press(&mut choose, &[KeyCode::Up, KeyCode::Up, KeyCode::Char(' ')]);
        assert_eq!(choose.plan.shells.0, [Shell::Bash, Shell::Fish]);

        press(&mut choose, &[KeyCode::Left]);
        assert_eq!((choose.row, choose.open), (Row::Shells, None));
        assert_eq!(states(&choose, &picks), []);
    }

    /// The text of the line the screen draws for `row`.
    fn drawn(choose: &Choose, found: &Found, row: Row) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 40)).unwrap();
        let shown = terminal.draw(|frame| choose.draw(frame, found)).unwrap().buffer.clone();
        let label = row.label();
        shown
            .content()
            .chunks(80)
            .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .find(|line| line.contains(label))
            .expect("the row is drawn")
    }

    #[rstest]
    #[case::all(&[Shell::Zsh, Shell::Bash, Shell::Fish, Shell::PowerShell], None, "▸ [✓] Shells")]
    #[case::some(&[Shell::Bash], None, "▸ [-] Shells")]
    #[case::none(&[], None, "▸ [ ] Shells")]
    #[case::open(&[Shell::Bash], Some(List::Shells), "▾ [-] Shells")]
    fn a_list_checks_all_some_or_none(
        found: Found,
        #[case] shells: &[Shell],
        #[case] open: Option<List>,
        #[case] expected: &str,
    ) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.plan.shells = Shells(shells.to_vec());
        choose.open = open;
        let line = drawn(&choose, &found, Row::Shells);
        assert!(line.contains(expected), "{line}");
    }

    #[rstest]
    fn toggling_a_list_takes_all_of_it_then_none(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.row = Row::Shells;
        press(&mut choose, &[KeyCode::Char(' ')]);
        assert_eq!(choose.plan.shells.0, HOOKABLE);
        press(&mut choose, &[KeyCode::Enter]);
        assert_eq!(choose.plan.shells.0, []);
        assert_eq!(choose.open, None);
    }

    #[rstest]
    fn a_list_waits_on_its_row(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.plan.shell = ShellHistory(false);
        choose.open = Some(List::Shells);
        let rows = [Row::Shells, Row::ShellPick(0)];
        assert_eq!(states(&choose, &rows), [(Row::Shells, Some("shell history"))]);
    }

    #[rstest]
    fn an_empty_list_is_not_a_plan(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.plan.shells = Shells(Vec::new());
        choose.row = Row::Go;
        assert!(press(&mut choose, &[KeyCode::Enter]).is_none());
        assert_eq!(choose.row, Row::Shells);
        assert!(choose.error.is_some());
    }

    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    fn a_connected_agent_gets_its_hooks_and_atuins_mcp_server(found: Found) {
        let plan = finish(&mut Choose::new(&found, &preset(&found)));
        assert_eq!((plan.agent_history, plan.mcp), (AgentHistory(true), Mcp(true)));
    }

    #[cfg(all(feature = "ai", feature = "daemon"))]
    #[rstest]
    fn with_no_agent_connected_there_is_nothing_to_hook_or_connect(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        choose.plan.agents = Agents(Vec::new());
        let rows = [Row::AgentHistory, Row::Threads];
        assert_eq!(states(&choose, &rows), [
            (Row::AgentHistory, Some("a connected agent")),
            (Row::Threads, None),
        ]);
        let plan = finish(&mut choose);
        assert_eq!((plan.agent_history, plan.mcp), (AgentHistory(false), Mcp(false)));
    }

    #[rstest]
    fn the_keys_open_at_the_bottom_of_the_features(found: Found) {
        let mut choose = Choose::new(&found, &preset(&found));
        let rows =
            |choose: &Choose| choose.rows().into_iter().map(|(row, _)| row).collect::<Vec<_>>();
        assert!(rows(&choose).ends_with(&[Row::Keys, Row::Go]), "{:?}", rows(&choose));

        choose.row = Row::Keys;
        press(&mut choose, &[KeyCode::Enter]);
        let open = [Row::Keys, Row::CtrlR, Row::UpArrow, Row::Go];
        assert!(rows(&choose).ends_with(&open), "{:?}", rows(&choose));
        let line = drawn(&choose, &found, Row::Keys);
        assert!(line.starts_with("› ▾ Advanced keybindings"), "{line}");

        press(&mut choose, &[KeyCode::Down, KeyCode::Left]);
        assert_eq!((choose.row, choose.open), (Row::Keys, None));
    }

    #[rstest]
    fn the_keys_wait_on_shell_history(found: Found) {
        let mut choose = on_the_keys(&found);
        choose.row = Row::Shell;
        press(&mut choose, &[KeyCode::Enter]);
        assert!(!choose.plan.shell.0);
        let rows = [Row::Keys, Row::CtrlR, Row::UpArrow];
        assert_eq!(states(&choose, &rows), [(Row::Keys, Some("shell history"))]);
        let line = drawn(&choose, &found, Row::Keys);
        assert!(line.starts_with("  ▸ Advanced keybindings       needs shell history"), "{line}");
        // Something to set up without shell history.
        choose.plan.sync = Sync::Cloud;
        let plan = finish(&mut choose);
        assert!(!plan.ctrl_r.0 && !plan.up_arrow.0);
    }

    #[rstest]
    #[case::coloured(true)]
    #[case::without_colour(false)]
    fn a_ruled_out_row_says_what_it_needs(found: Found, #[case] colour: bool) {
        let mut choose = Choose::new(&found, &preset(&found)).coloured(colour);
        choose.plan.shell = ShellHistory(false);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        let shown = terminal.draw(|frame| choose.draw(frame, &found)).unwrap().buffer.clone();
        let rows: Vec<String> = shown
            .content()
            .chunks(80)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect();
        let y = rows.iter().position(|row| row.contains("Shells")).expect("Shells is shown");
        assert!(rows[y].contains("needs shell history"), "{}", rows[y]);
        let at = rows[y].find("Shells").expect("Shells is on its row");
        let x = rows[y][..at].chars().count();
        let cell = &shown[(u16::try_from(x).unwrap(), u16::try_from(y).unwrap())];
        if colour {
            assert_eq!(cell.fg, Color::DarkGray);
        } else {
            assert!(cell.modifier.contains(Modifier::CROSSED_OUT), "{cell:?}");
        }
    }

    #[rstest]
    fn up_arrow_can_be_left_to_the_shell(found: Found) {
        let mut choose = on_the_keys(&found);
        press(&mut choose, &[KeyCode::Down, KeyCode::Enter]);
        assert!(drawn(&choose, &found, Row::UpArrow).contains("left to your shell"));
        let plan = finish(&mut choose);
        assert!(plan.ctrl_r.0 && !plan.up_arrow.0);
    }

    #[cfg(feature = "sync")]
    #[rstest]
    fn own_server_needs_an_address(found: Found) {
        let mut choose = on_page(&found, &preset(&found), Page::Sync);
        choose.row = Row::SyncTo(SyncTo::SelfHosted);
        press(&mut choose, &[KeyCode::Enter]);
        assert_eq!(choose.row, Row::Server);
        choose.row = Row::Go;
        assert!(press(&mut choose, &[KeyCode::Enter]).is_none());
        assert_eq!(choose.row, Row::Server);
        assert!(choose.error.is_some());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        let shown = terminal.draw(|frame| choose.draw(frame, &found)).unwrap().buffer.clone();
        let text: String = shown.content().iter().map(ratatui::buffer::Cell::symbol).collect();
        assert!(text.contains("Enter your server's address"));

        let typed: Vec<_> = "https://atuin.example.com".chars().map(KeyCode::Char).collect();
        press(&mut choose, &typed);
        press(&mut choose, &[KeyCode::Enter]);
        assert_eq!(choose.row, Row::Go);
        let plan = finish(&mut choose);
        assert_eq!(plan.sync, Sync::Server("https://atuin.example.com".into()));
    }

    #[cfg(feature = "octavo")]
    #[rstest]
    #[case::always(&[], Some(Teams::On { backfill: false }), ThreadUploads::All)]
    #[case::workspace_only(
        &[Row::Sharing(Sharing::Workspaces)],
        Some(Teams::On { backfill: false }),
        ThreadUploads::Workspaces,
    )]
    #[case::disabled(&[Row::Sharing(Sharing::Off)], Some(Teams::Off), ThreadUploads::All)]
    #[case::self_hosted(&[Row::SyncTo(SyncTo::SelfHosted)], Some(Teams::Off), ThreadUploads::All)]
    #[case::back_to_atuin_cloud(
        &[Row::SyncTo(SyncTo::Off), Row::SyncTo(SyncTo::Cloud)],
        Some(Teams::On { backfill: false }),
        ThreadUploads::All,
    )]
    #[case::atuin_cloud_picked_again(
        &[Row::Sharing(Sharing::Off), Row::SyncTo(SyncTo::Cloud)],
        Some(Teams::Off),
        ThreadUploads::All,
    )]
    #[case::backfill(&[Row::Backfill], Some(Teams::On { backfill: true }), ThreadUploads::All)]
    #[case::no_backfill_of_workspaces(
        &[Row::Backfill, Row::Sharing(Sharing::Workspaces)],
        Some(Teams::On { backfill: false }),
        ThreadUploads::Workspaces,
    )]
    fn atuin_cloud_shares_threads_as_picked(
        found: Found,
        #[case] picked: &[Row],
        #[case] expected: Option<Teams>,
        #[case] threads: ThreadUploads,
    ) {
        let mut choose = on_page(&found, &teams(&found), Page::Sync);
        choose.server = "https://atuin.example.com".into();
        for &row in picked {
            choose.row = row;
            press(&mut choose, &[KeyCode::Enter]);
        }
        let plan = finish(&mut choose);
        assert_eq!((plan.teams(), plan.upload_threads.0), (expected, threads));
    }

    #[cfg(feature = "sync")]
    #[rstest]
    #[case::without_teams(false, SyncTo::Cloud, &[Page::Features, Page::Sync])]
    #[cfg_attr(
        feature = "octavo",
        case::atuin_cloud(true, SyncTo::Cloud, &[Page::Features, Page::Sync, Page::Hub]),
    )]
    #[cfg_attr(
        feature = "octavo",
        case::self_hosted(true, SyncTo::SelfHosted, &[Page::Features, Page::Sync]),
    )]
    #[cfg_attr(feature = "octavo", case::disabled(true, SyncTo::Off, &[Page::Features, Page::Sync]))]
    fn the_hub_page_follows_atuin_cloud_with_the_flag(
        found: Found,
        #[case] teams: bool,
        #[case] to: SyncTo,
        #[case] expected: &[Page],
    ) {
        let flags: &[&str] = if teams {
            &["setup", "--hub"]
        } else {
            &["setup"]
        };
        let plan = super::super::Cmd::parse_from(flags).plan(&found, false).unwrap();
        let mut choose = on_page(&found, &plan, Page::Sync);
        let rows: Vec<_> = choose.rows().into_iter().map(|(row, _)| row).collect();
        assert_eq!(rows, [
            Row::SyncTo(SyncTo::Cloud),
            Row::SyncTo(SyncTo::SelfHosted),
            Row::SyncTo(SyncTo::Off),
            Row::Go,
        ]);
        choose.row = Row::SyncTo(to);
        press(&mut choose, &[KeyCode::Enter]);
        assert_eq!(choose.pages(), expected);
    }

    #[cfg(feature = "sync")]
    #[rstest]
    fn each_place_to_sync_says_what_it_means(found: Found) {
        let choose = on_page(&found, &preset(&found), Page::Sync);
        let shown = shown(&choose, &found);
        let explained = choose.rows().into_iter().filter_map(|(row, _)| match row {
            Row::SyncTo(to) => Some((to.label(), to.about()?)),
            _ => None,
        });
        let unexplained: Vec<_> = explained
            .filter(|(label, about)| {
                let first = wrap(about, WRAP_COLUMNS).into_iter().next().unwrap_or_default();
                let at = shown.iter().position(|row| row.contains(&format!(") {label}")));
                at.is_none_or(|at| !shown[at + 1].contains(&first))
            })
            .collect();
        assert_eq!(unexplained, [], "{shown:#?}");
    }

    #[cfg(feature = "octavo")]
    #[rstest]
    fn the_hub_page_says_what_atuin_hub_is_and_what_each_choice_uploads(found: Found) {
        let choose = on_page(&found, &teams(&found), Page::Hub);
        let shown = shown(&choose, &found);
        assert!(shown[1].contains("Features › Sync › Hub"), "{shown:#?}");
        let at = shown.iter().position(|row| row.trim_end() == "  Atuin Hub").expect("a heading");
        let intro = |row: &&String| row.contains("Atuin Hub allows you to share your threads");
        assert!(intro(&&shown[at + 1]), "{shown:#?}");
        assert_eq!(shown.iter().filter(intro).count(), 1, "{shown:#?}");

        let rows: Vec<_> = choose.rows().into_iter().map(|(row, _)| row).collect();
        assert_eq!(rows, [
            Row::Sharing(Sharing::Always),
            Row::Backfill,
            Row::Sharing(Sharing::Workspaces),
            Row::Sharing(Sharing::Off),
            Row::Go,
        ]);
        for sharing in [Sharing::Always, Sharing::Workspaces, Sharing::Off] {
            let line = choose.line(Row::Sharing(sharing), false, &found).to_string();
            let expected = format!("{:<LABEL_WIDTH$}{}", sharing.label(), sharing.about());
            assert!(line.contains(&expected), "{line}");
        }
    }

    #[cfg(feature = "octavo")]
    #[rstest]
    #[case::always(Sharing::Always, true)]
    #[case::workspace_only(Sharing::Workspaces, false)]
    #[case::disabled(Sharing::Off, false)]
    fn backfill_is_asked_with_every_thread(
        found: Found,
        #[case] sharing: Sharing,
        #[case] asked: bool,
    ) {
        let mut choose = on_page(&found, &teams(&found), Page::Hub);
        choose.row = Row::Sharing(sharing);
        press(&mut choose, &[KeyCode::Enter]);
        assert_eq!(states(&choose, &[Row::Backfill]).len() == 1, asked);
        if asked {
            let line = drawn(&choose, &found, Row::Backfill);
            assert!(line.contains("        [ ] Upload what Atuin has already"), "{line}");
        }
    }

    #[rstest]
    #[case::fits("a few words", 20, &["a few words"])]
    #[case::between_words("one two three", 7, &["one two", "three"])]
    #[case::a_long_word_alone("a extraordinarily b", 5, &["a", "extraordinarily", "b"])]
    #[case::nothing("", 5, &[])]
    fn wrap_breaks_between_words(
        #[case] text: &str,
        #[case] columns: usize,
        #[case] expected: &[&str],
    ) {
        assert_eq!(wrap(text, columns), expected);
    }

    #[cfg(feature = "sync")]
    #[rstest]
    fn esc_goes_back_a_page_at_a_time(found: Found) {
        let mut choose = on_page(&found, &preset(&found), Page::Sync);
        press(&mut choose, &[KeyCode::Esc]);
        assert_eq!((choose.page, choose.row), (Page::Features, Row::Go));
        assert!(matches!(press(&mut choose, &[KeyCode::Esc]), Some(Chosen::Quit)));
    }

    #[cfg(feature = "sync")]
    #[rstest]
    fn the_crumbs_name_every_page(found: Found) {
        let choose = Choose::new(&found, &preset(&found));
        assert!(shown(&choose, &found)[1].contains("Features › Sync"));
    }

    #[rstest]
    /// With nothing to set up, there's nothing to do, unless Atuin is set up here already: then
    /// turning it all off takes it out.
    #[rstest]
    #[case::nothing_here(&[], false)]
    #[case::set_up_here(&[Shell::Zsh], true)]
    fn nothing_chosen_is_a_plan_only_to_take_atuin_out(
        mut found: Found,
        #[case] inits: &[Shell],
        #[case] is_plan: bool,
    ) {
        found.inits = inits.to_vec();
        let mut choose = Choose::new(&found, &preset(&found));
        choose.plan.shell = ShellHistory(false);
        choose.plan.threads = Threads(false);
        choose.plan.agent_history = AgentHistory(false);
        choose.plan.mcp = Mcp(false);
        choose.plan.sync = Sync::Off;
        // Sync may yet give it something to set up, so only the last page says so.
        choose.page = *choose.pages().last().expect("a page");
        choose.row = Row::Go;
        let chosen = press(&mut choose, &[KeyCode::Enter]);
        assert_eq!(
            (matches!(chosen, Some(Chosen::Plan(_))), choose.error.is_none()),
            (is_plan, is_plan)
        );
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
