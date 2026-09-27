//! `atuin ai resume`: an interactive picker over captured AI coding-agent sessions.
//!
//! A ratatui view built to look and behave like the history search (`atuin search -i`): the same
//! header, tabs and `[ MODE ] >` input box, preview pane, `invert`/`style`/`inline_height`, enter
//! vs tab accept, and emacs/vim keymaps from [`atuin_client::tui`].
//!
//! It depends on two seams:
//! - [`SessionSource`] lists, searches and previews sessions;
//! - [`Resumer`] turns a session into a resume command, or says why it can't be resumed.

pub mod fake;
pub mod keymap;
pub mod panel;
pub mod query;
pub mod render;
pub mod resumer;
pub mod sidecar;
pub mod source;
pub mod state;
mod terminal;
pub mod worker;

use std::io::{IsTerminal, stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atuin_client::ai_session::HarnessSession;
use atuin_client::settings::Settings;
use atuin_client::theme::{Meaning, Theme};
use crossterm::event::EventStream;
use eyre::Result;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
pub use resumer::{ResumePlan, Resumer};
pub use source::{SessionRow, SessionSource};

use self::resumer::NotResumable;
use self::state::{CHILDREN, InputAction, PLAN, PREVIEW, Pending, State};
use self::worker::{Request, Requests, Response};

/// How often the picker redraws on its own.
const TICK: std::time::Duration = std::time::Duration::from_secs(1);
/// How often an idle picker searches again, so live sessions stay current.
const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// How long after the last key press before a refresh may run.
const REFRESH_IDLE: std::time::Duration = std::time::Duration::from_secs(2);
/// While the selection moves faster than this (a held arrow key), details wait until it settles.
const SETTLE: Duration = Duration::from_millis(60);

/// Where the picker runs: what its filter modes resolve against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResumeContext {
    pub cwd: PathBuf,
    pub git_root: Option<PathBuf>,
    pub branch: Option<String>,
    pub host_id: String,
    pub hostname: String,
}

impl ResumeContext {
    /// The current directory, repository, branch and host.
    pub async fn current() -> Result<Self> {
        let ctx = atuin_client::database::query_context().await?;
        let git_root = ctx.git_root;
        let branch = git_root.as_deref().and_then(current_branch);
        Ok(Self {
            cwd: PathBuf::from(ctx.cwd),
            git_root,
            branch,
            host_id: ctx.host_id,
            hostname: ctx.cmd_origin.host().into_inner().to_string(),
        })
    }
}

/// The checked-out branch of the repository at `root`, read from `HEAD` (following a worktree's
/// `.git` file), or `None` when detached.
fn current_branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_file() {
        let contents = fs_err::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(contents.trim().strip_prefix("gitdir:")?.trim());
        if dir.is_absolute() {
            dir
        } else {
            root.join(dir)
        }
    } else {
        dot_git
    };
    let head = fs_err::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(str::to_owned)
}

/// How the picker ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Resume the session now (enter, with `enter_accept`).
    Resume(ResumePlan),
    /// Put the command on the command line (tab, or enter without `enter_accept`).
    Edit(ResumePlan),
    /// Esc, ctrl-c or ctrl-g: leave the command line as it was.
    Cancelled,
}

pub struct Picker<'a> {
    pub settings: &'a Settings,
    pub theme: &'a Theme,
    pub source: Arc<dyn SessionSource>,
    pub resumer: Arc<dyn Resumer>,
    pub context: ResumeContext,
    pub query: String,
    /// Overrides `[ai.sessions] inline_height` and the top-level `inline_height`.
    pub inline_height: Option<u16>,
    /// A session the query named by id that can't be resumed: open on it, showing why.
    pub preselect: Option<(SessionRow, NotResumable)>,
}

/// Holds back detail requests while the selection moves fast, so the sessions it passes over
/// are never loaded. A single move asks at once.
#[derive(Default)]
struct Settle {
    selected: Option<HarnessSession>,
    moved_at: Option<Instant>,
    /// When to ask for the selected session's details, if not yet.
    due: Option<Instant>,
}

impl Settle {
    /// Whether details for `selected` may be asked for at `now`; if not, [`Self::due`] says when.
    fn ready(&mut self, selected: &HarnessSession, now: Instant) -> bool {
        if self.selected.as_ref() != Some(selected) {
            let fast = self.moved_at.is_some_and(|t| now.saturating_duration_since(t) < SETTLE);
            self.selected = Some(selected.clone());
            self.moved_at = Some(now);
            self.due = fast.then(|| now + SETTLE);
        }
        match self.due {
            Some(due) if now < due => false,
            _ => {
                self.due = None;
                true
            }
        }
    }
}

/// Ask the worker for whatever the current view needs and doesn't have yet.
fn request_details(state: &mut State, requests: &Requests, settle: &mut Settle, subagents: bool) {
    state.forget_unanswered();
    let Some(row) = state.selected().cloned() else {
        return;
    };
    let handle = row.handle.clone();
    if !settle.ready(&handle, Instant::now()) {
        return;
    }
    if !state.previews.contains_key(&handle) && state.requested.insert((handle.clone(), PREVIEW)) {
        requests.send(Request::Preview(handle.clone()));
    }
    if state.tab_index == 1 {
        if !state.children.contains_key(&handle)
            && state.requested.insert((handle.clone(), CHILDREN))
        {
            requests.send(Request::Children {
                session: handle,
                include_subagents: subagents,
            });
        }
        request_plan(state, requests, &row);
    }
}

fn request_plan(state: &mut State, requests: &Requests, row: &SessionRow) {
    if !state.plans.contains_key(&row.handle) && state.requested.insert((row.handle.clone(), PLAN))
    {
        requests.send(Request::Plan(Box::new(row.clone())));
    }
}

fn send_search(state: &mut State, requests: &Requests) {
    if let Some((generation, mode, filter)) = state.next_search() {
        requests.send(Request::Search {
            generation,
            mode,
            filter,
        });
    }
}

/// Apply a worker response.
fn apply_response(state: &mut State, response: Response, requests: &Requests) {
    match response {
        Response::Results {
            generation,
            mode,
            rows,
        } => match rows {
            Ok(rows) => {
                if state.apply_results(generation, mode, rows) {
                    // A widened workspace needs a new search.
                    send_search(state, requests);
                }
            }
            Err(e) if generation == state.issued => {
                state.status = Some((format!("search failed: {e}"), Meaning::AlertError));
            }
            Err(_) => {}
        },
        Response::Preview(handle, preview) => {
            state.previews.insert(handle, preview);
        }
        Response::Children(handle, children) => {
            state.children.insert(handle, children);
        }
        Response::Plan(handle, plan) => {
            state.plans.insert(handle, plan);
        }
    }
}

/// Carry out `action` for the selected session once its plan is known. `None` keeps the picker
/// open (the plan is still coming, the session can't be resumed, or it was a copy).
fn complete(state: &mut State, action: Pending) -> Option<Outcome> {
    let row = state.selected()?;
    let Some(plan) = state.plans.get(&row.handle) else {
        state.pending = Some((row.handle.clone(), action));
        state.status = Some(("locating the session…".to_owned(), Meaning::Annotation));
        return None;
    };
    let outcome = match (plan, action) {
        (Err(why), _) => {
            state.status = Some((format!("can't resume: {why}"), Meaning::AlertError));
            state.accept = false;
            None
        }
        (Ok(plan), Pending::Copy) => {
            let line = resumer::shell_line(plan);
            state.status = Some(match set_clipboard(&line) {
                Ok(()) => (format!("copied: {line}"), Meaning::AlertInfo),
                Err(e) => (format!("copy failed: {e}"), Meaning::AlertError),
            });
            None
        }
        (Ok(plan), Pending::Resume) => Some(Outcome::Resume(plan.clone())),
        (Ok(plan), Pending::Edit) => Some(Outcome::Edit(plan.clone())),
    };
    state.pending = None;
    outcome
}

impl Picker<'_> {
    #[allow(clippy::too_many_lines)]
    pub async fn run(self) -> Result<Outcome> {
        let settings = self.settings;
        let sessions = &settings.ai.sessions;
        let inline_height =
            self.inline_height.or(sessions.inline_height).unwrap_or(settings.inline_height);
        // Fullscreen when the inline viewport doesn't fit, or stdout is captured (inline needs
        // cursor position queries on the terminal).
        let inline_height = if !stdout().is_terminal() {
            0
        } else if let Ok((_, rows)) = crossterm::terminal::size()
            && inline_height >= rows
        {
            0
        } else {
            inline_height
        };

        let out = terminal::TuiStdout::new(inline_height > 0, settings.no_mouse)?;
        let mut terminal = Terminal::with_options(CrosstermBackend::new(out), TerminalOptions {
            viewport: if inline_height > 0 {
                Viewport::Inline(inline_height)
            } else {
                Viewport::Fullscreen
            },
        })?;

        let mut state = State::new(settings, self.context, &self.query);
        if let Some((row, why)) = self.preselect {
            state.pin(row, why);
        }
        let (requests, mut responses) = worker::spawn(self.source, self.resumer);

        if inline_height > 0 {
            terminal.clear()?;
        }
        // Paint before the first search answers, so the picker shows up immediately.
        terminal.draw(|f| state.draw(f, settings, self.theme))?;
        send_search(&mut state, &requests);

        let mut events = EventStream::new();
        // Ticks keep relative times and live dots current, and refresh the list now and then so
        // running sessions move and their previews catch up.
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_input = std::time::Instant::now();
        let mut last_refresh = std::time::Instant::now();
        let mut settle = Settle::default();
        let outcome = 'render: loop {
            request_details(&mut state, &requests, &mut settle, sessions.show_subagents);
            terminal.draw(|f| state.draw(f, settings, self.theme))?;

            tokio::select! {
                event = events.next() => {
                    let Some(event) = event else { break Outcome::Cancelled };
                    last_input = std::time::Instant::now();
                    let action = state.handle_input(settings, &event?);
                    let pending = match action {
                        InputAction::Continue => None,
                        InputAction::Redraw => {
                            if state.tab_index != 1 {
                                terminal.clear()?;
                            }
                            None
                        }
                        InputAction::Resume(_) => Some(Pending::Resume),
                        InputAction::ReturnCommand(_) => Some(Pending::Edit),
                        InputAction::Copy(_) => Some(Pending::Copy),
                        InputAction::ReturnOriginal | InputAction::Exit => break Outcome::Cancelled,
                    };
                    if let Some(pending) = pending {
                        if let Some(row) = state.selected().cloned() {
                            request_plan(&mut state, &requests, &row);
                        }
                        if let Some(outcome) = complete(&mut state, pending) {
                            break 'render outcome;
                        }
                    }
                }
                // The selection settled: ask for its details (at the top of the loop).
                () = tokio::time::sleep_until(settle.due.unwrap_or_else(Instant::now).into()),
                    if settle.due.is_some() => {}
                _ = tick.tick() => {
                    // Not while typing or browsing: the list shouldn't move under the cursor.
                    if last_input.elapsed() >= REFRESH_IDLE
                        && last_refresh.elapsed() >= REFRESH_EVERY
                        && let Some((generation, mode, filter)) = state.refresh()
                    {
                        last_refresh = std::time::Instant::now();
                        requests.send(Request::Search { generation, mode, filter });
                    }
                }
                response = responses.recv() => {
                    if let Some(response) = response {
                        apply_response(&mut state, response, &requests);
                    }
                    // An enter/tab/ctrl-y waiting on this session's plan can finish now.
                    if let Some((handle, pending)) = state.pending.clone()
                        && state.plans.contains_key(&handle)
                        && let Some(outcome) = complete(&mut state, pending)
                    {
                        break 'render outcome;
                    }
                }
            }
            // The selection moved away from a pending action: drop it.
            if let Some((handle, _)) = &state.pending
                && state.selected().is_none_or(|r| &r.handle != handle)
            {
                state.pending = None;
                state.status = None;
            }
            send_search(&mut state, &requests);
        };

        if inline_height > 0 {
            terminal.clear()?;
            let origin = terminal.get_frame().area().as_position();
            terminal.set_cursor_position(origin)?;
        }
        Ok(outcome)
    }
}

#[cfg(all(
    feature = "clipboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
fn set_clipboard(s: &str) -> Result<()> {
    let mut ctx = arboard::Clipboard::new()?;
    ctx.set_text(s.to_owned())?;
    // Read it back so the clipboard owner keeps it after we exit.
    ctx.get_text()?;
    Ok(())
}

#[cfg(not(all(
    feature = "clipboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
)))]
fn set_clipboard(_s: &str) -> Result<()> {
    eyre::bail!("this build has no clipboard support")
}

#[cfg(test)]
mod tests;
