//! `atuin ai resume`: an interactive picker over captured AI coding-agent sessions.
//!
//! A ratatui view built to look and behave like the history search (`atuin search -i`): the same
//! header, tabs and `[ MODE ] >` input box, preview pane, `invert`/`style`/`inline_height`, enter
//! vs tab accept, and emacs/vim keymaps from [`atuin_client::tui`]. Accepting a session asks
//! where to resume it: in its own harness, or continued in another ([`chooser`]). Resuming it
//! in its own harness first catches this machine's copy up with sync ([`catchup`]).
//!
//! It depends on two seams:
//! - [`SessionSource`] lists, searches and previews sessions;
//! - [`Resumer`] turns a session into a resume command, or says why it can't be resumed.

pub mod catchup;
pub mod chooser;
pub mod clock;
pub mod fake;
pub mod keymap;
mod markdown;
pub mod panel;
pub mod query;
pub mod render;
pub mod resumer;
pub mod sidecar;
pub mod source;
pub mod state;
mod terminal;
pub mod title;
pub mod worker;

use std::io::{IsTerminal, stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atuin_client::ai_session::{HarnessKind, HarnessSession};
use atuin_client::settings::Settings;
use atuin_client::theme::{Meaning, Theme};
use eyre::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::{Terminal, TerminalOptions, Viewport};
pub use resumer::{ResumePlan, Resumer};
pub use source::{SessionRow, SessionSource};

use self::catchup::Synced;
use self::resumer::{Continued, NotResumable, Resume};
use self::state::{CHILDREN, HEADS, InputAction, PLAN, PREVIEW, Pending, SYNC, State};
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
            // `$PWD` as it is set, which may end in a separator: rebuilt from its components, so
            // a session restored or continued here isn't written with `…/dir/` as its directory.
            cwd: Path::new(&ctx.cwd).components().collect(),
            git_root,
            branch,
            host_id: simple_host_id(&ctx.host_id),
            hostname: ctx.cmd_origin.host().into_inner().to_string(),
        })
    }
}

/// A host id in the one form the picker compares them in: a UUID's simple (unhyphenated) form,
/// which is how rows carry theirs. Anything that isn't a UUID is kept as it is.
pub fn simple_host_id(id: &str) -> String {
    uuid::Uuid::try_parse(id).map_or_else(|_| id.to_owned(), |id| id.as_simple().to_string())
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
fn request_details(state: &mut State, requests: &Requests, settle: &mut Settle) {
    state.forget_unanswered();
    let Some(row) = state.selected().cloned() else {
        return;
    };
    let handle = row.handle.clone();
    if !settle.ready(&handle, Instant::now()) {
        return;
    }
    if state.wants_preview(&handle) && state.requested.insert((handle.clone(), PREVIEW)) {
        requests.send(Request::Preview(handle.clone()));
    }
    // Inspect lists the forks; the detail pane and the preview count them. A row with nothing
    // grouped under it has none (one with only subagents has none either, which takes asking).
    let wants_children = state.tab_index == 1 || row.children > 0;
    if wants_children
        && !state.children.contains_key(&handle)
        && state.requested.insert((handle.clone(), CHILDREN))
    {
        requests.send(Request::Children(handle));
    }
    if state.tab_index == 1 {
        request_plan(state, requests, &row);
    }
    request_flatten(state, requests);
}

/// While the chooser is open, read what continuing the branch its harness lines are for (the
/// session, when it went one way) would flatten, if it isn't known yet.
fn request_flatten(state: &mut State, requests: &Requests) {
    let Some(chooser) = &state.chooser else {
        return;
    };
    if chooser.targets.is_empty() {
        return;
    }
    let key = (chooser.session.clone(), chooser.continued().map(|h| h.source_id.clone()));
    if state.flattened.contains_key(&key) || state.flattening.as_ref() == Some(&key) {
        return;
    }
    requests.send(Request::Flatten(key.0.clone(), key.1.clone(), state.context.cwd.clone()));
    state.flattening = Some(key);
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
        Response::Preview(handle, preview) => state.apply_preview(handle, preview),
        Response::Children(handle, children) => {
            state.children.insert(handle, children);
        }
        Response::Plan(handle, plan) => {
            state.plans.insert(handle, plan);
            state.settle_chooser();
        }
        Response::Synced(handle, head, synced) => {
            state.requested.remove(&(handle.clone(), SYNC));
            state.synced.insert(handle, (head, synced));
        }
        Response::Flattened(handle, head, flattened) => {
            state.flattened.insert((handle, head), flattened);
        }
        Response::Heads(handle, heads) => {
            state.requested.remove(&(handle.clone(), HEADS));
            if let Some(heads) = heads {
                for row in state.results.iter_mut().filter(|r| r.handle == handle) {
                    row.heads.clone_from(&heads.heads);
                    row.diverged = heads.diverged;
                }
            }
            state.fresh_heads.insert(handle);
        }
        // Handled by `finish_continuation`, which may end the picker.
        Response::Continued(..) => {}
        Response::HostNames(names) => state.apply_host_names(names),
    }
}

/// Carry out `action` for the selected session once its plan is known, catching this machine's
/// copy up with sync first (restoring it when it isn't here; see [`catchup`]). `None` keeps the
/// picker open (the plan or the catch-up is still coming, the session can't be resumed, the user
/// is asked first, or it was a copy).
fn complete(state: &mut State, action: Pending, requests: &Requests) -> Option<Outcome> {
    let row = state.selected()?.clone();
    let Some(plan) = state.plans.get(&row.handle).cloned() else {
        state.pending = Some((row.handle, action));
        state.status = Some(("locating the session…".to_owned(), Meaning::Annotation));
        return None;
    };
    let outcome = match (plan, action) {
        (Err(why), _) => {
            state.status = Some((format!("can't resume: {why}"), Meaning::AlertError));
            state.accept = false;
            None
        }
        (Ok(resume), Pending::Copy) => {
            let line = resume_line(state, &row, &resume);
            copy(state, &line);
            None
        }
        // Couldn't be caught up, and the user was told: as it is.
        (Ok(resume), action) if state.as_is.contains(&row.handle) => Some(match action {
            Pending::Resume => Outcome::Resume(resume.plan),
            Pending::Edit | Pending::Copy => Outcome::Edit(resume.plan),
        }),
        (Ok(resume), action) => return catch_up(state, &row, &resume, action, requests),
    };
    state.pending = None;
    outcome
}

/// Resume (or edit) `row`, planned as `resume`, once its copy here is caught up with sync:
/// warning first when another host may still be working on it, then asking the worker, then
/// taking its answer.
fn catch_up(
    state: &mut State,
    row: &SessionRow,
    resume: &Resume,
    action: Pending,
    requests: &Requests,
) -> Option<Outcome> {
    let handle = &row.handle;
    let head = state.picked.get(handle).cloned();
    // An answer for another branch than the one picked now is dropped.
    if let Some((for_head, synced)) = state.synced.remove(handle)
        && for_head == head
    {
        state.pending = None;
        return finish_sync(state, row, synced, action);
    }
    if !state.confirmed.contains(handle) {
        // Whether another host is still at it is told from the heads as they are now: the row's
        // are from the last search, which may be minutes old.
        if !state.fresh_heads.remove(handle) {
            if state.requested.insert((handle.clone(), HEADS)) {
                requests.send(Request::Heads(handle.clone()));
            }
            state.pending = Some((handle.clone(), action));
            return None;
        }
        if let Some(warning) = state.live_elsewhere(row, action) {
            state.warning = Some(warning);
            state.pending = None;
            state.accept = false;
            return None;
        }
    }
    if state.requested.insert((handle.clone(), SYNC)) {
        requests.send(Request::Sync(Box::new(row.clone()), head));
    }
    state.status = Some(match &resume.restore {
        Some(restore) => {
            let note = restore.note.as_ref().map(|n| format!(": {n}")).unwrap_or_default();
            (format!("restoring from sync…{note}"), Meaning::Annotation)
        }
        None => ("catching up with sync…".to_owned(), Meaning::Annotation),
    });
    state.pending = Some((handle.clone(), action));
    None
}

/// The worker's answer to a catch-up: the outcome that resumes it, leaving its status line as
/// the note to print; or, when nothing could be written under a harness running it here (or
/// writing failed), stay open saying why, the next enter resuming the copy here as it is.
fn finish_sync(
    state: &mut State,
    row: &SessionRow,
    synced: Result<Synced, NotResumable>,
    action: Pending,
) -> Option<Outcome> {
    let synced = match synced {
        Ok(synced) => synced,
        Err(why) => {
            state.status = Some((format!("can't resume: {why}"), Meaning::AlertError));
            state.accept = false;
            return None;
        }
    };
    let status = synced.status(row.handle.harness, &|h| state.head_host(h));
    if synced.holds() {
        let status = status.unwrap_or_default();
        state.status = Some((
            format!("{status}; enter resumes this machine's copy as it is"),
            Meaning::AlertWarn,
        ));
        state.as_is.insert(row.handle.clone());
        state.plans.insert(row.handle.clone(), Ok(Resume::ready(synced.plan)));
        state.accept = false;
        return None;
    }
    if let Some(status) = &status {
        state.status = Some((status.clone(), Meaning::AlertInfo));
    }
    state.note = status;
    Some(match action {
        Pending::Resume => Outcome::Resume(synced.plan),
        Pending::Edit | Pending::Copy => Outcome::Edit(synced.plan),
    })
}

/// Enter or tab on a session (or ctrl-y, which copies): ask where to resume it (the "Resume in"
/// chooser, when `chooser` is on and another harness is installed to continue it in, or the
/// session went on separately on several machines, to pick a branch), or resume it in its own
/// harness.
fn accept(
    state: &mut State,
    action: Pending,
    resumer: &dyn Resumer,
    requests: &Requests,
    chooser: bool,
) -> Option<Outcome> {
    let row = state.selected()?.clone();
    request_plan(state, requests, &row);
    // A session with several branches always asks which (with only them, without `chooser`).
    if (chooser || !row.branches().is_empty()) && action != Pending::Copy {
        let targets = if chooser {
            resumer.continue_targets(&row)
        } else {
            Vec::new()
        };
        if !targets.is_empty() || !row.branches().is_empty() {
            open_chooser(state, targets, action, requests);
            return None;
        }
    }
    resume_original(state, action, resumer, requests)
}

/// Resume the selected session in its own harness ([`complete`]). When that harness can't
/// resume it here, the chooser opens instead (if another harness is installed), saying why and
/// offering the others.
fn resume_original(
    state: &mut State,
    action: Pending,
    resumer: &dyn Resumer,
    requests: &Requests,
) -> Option<Outcome> {
    let outcome = complete(state, action, requests);
    if outcome.is_none()
        && action != Pending::Copy
        && state.pending.is_none()
        && state.chooser.is_none()
        && let Some(row) = state.selected().cloned()
        && state.original_unavailable(&row.handle).is_some()
    {
        let targets = resumer.continue_targets(&row);
        if !targets.is_empty() {
            open_chooser(state, targets, action, requests);
            // The chooser's own line says why, dimmed.
            state.status = None;
        }
    }
    outcome
}

/// Open the chooser on the selected session, and read what continuing it elsewhere would
/// flatten, for the chooser to show.
fn open_chooser(
    state: &mut State,
    targets: Vec<HarnessKind>,
    action: Pending,
    requests: &Requests,
) {
    state.open_chooser(targets, action);
    request_flatten(state, requests);
}

/// Continue the selected session in `target`, then carry out `action` (see
/// [`finish_continuation`]). Copying writes nothing: the command copied continues it when run.
fn start_continuation(
    state: &mut State,
    target: HarnessKind,
    action: Pending,
    requests: &Requests,
) {
    let Some(row) = state.selected().cloned() else {
        return;
    };
    let label = source::harness_label(target);
    if action == Pending::Copy {
        let id = resumer::quote(row.handle.session.as_ref());
        let into = source::harness_arg(target).unwrap_or_default();
        let branch = branch_arg(&row, state.continue_from.get(&row.handle));
        copy(state, &format!("atuin ai resume {id} --in {into}{branch}"));
        return;
    }
    let head = state.continue_from.get(&row.handle).cloned();
    let what = match state.flattened.get(&(row.handle.clone(), head.clone())) {
        Some(Ok(flattened)) if !flattened.summary().is_empty() => {
            format!(": {}", flattened.summary())
        }
        _ => String::new(),
    };
    state.status = Some((format!("continuing in {label}{what}…"), Meaning::Annotation));
    state.continuing = Some((row.handle.clone(), target, action));
    requests.send(Request::Continue(Box::new(row), target, head));
}

/// A continuation is written (or failed): the outcome that resumes it, with the status line to
/// leave behind, or `None` to stay open, saying why.
fn finish_continuation(
    state: &mut State,
    handle: &HarnessSession,
    result: Result<Continued, NotResumable>,
) -> Option<(Outcome, String)> {
    let (_, target, action) = state.continuing.take_if(|(waiting, ..)| waiting == handle)?;
    match result {
        Ok(continued) => {
            let mut status = continued.status();
            if let Some(note) = &continued.note {
                status.push_str(&format!(" ({note})"));
            }
            state.status = Some((status.clone(), Meaning::AlertInfo));
            let outcome = match action {
                Pending::Resume => Outcome::Resume(continued.plan),
                Pending::Edit | Pending::Copy => Outcome::Edit(continued.plan),
            };
            Some((outcome, status))
        }
        Err(why) => {
            let message = match why {
                NotResumable::Continue(..) => why.to_string(),
                why => format!("can't continue in {}: {why}", source::harness_label(target)),
            };
            state.status = Some((message, Meaning::AlertError));
            state.accept = false;
            None
        }
    }
}

/// What ctrl-y copies to resume `row`, planned as `resume`, in its own harness. Copying writes
/// nothing, so when running the harness's own command wouldn't do (the session has to be
/// restored from sync first, or a branch was picked of one with several), it is `atuin ai
/// resume <id>`, which does that when run, naming the branch picked with `--branch`.
fn resume_line(state: &State, row: &SessionRow, resume: &Resume) -> String {
    let branch = branch_arg(row, state.picked.get(&row.handle));
    if resume.restore.is_none() && branch.is_empty() {
        return resumer::shell_line(&resume.plan);
    }
    format!("atuin ai resume {}{branch}", resumer::quote(row.handle.session.as_ref()))
}

/// ` --branch <selector>` for `head`, when it is one of the branches of `row` (a session that went
/// on separately on several machines), else nothing. The selector is the start of the branch's
/// id ([`catchup::branch_selector`]), which names it for good, where its host may not.
fn branch_arg(row: &SessionRow, head: Option<&atuin_client::ai_session::SourceId>) -> String {
    let branches = row.branches();
    head.and_then(|p| branches.iter().find(|h| &h.source_id == p)).map_or_else(String::new, |h| {
        format!(" --branch {}", resumer::quote(&catchup::branch_selector(branches, h)))
    })
}

/// Put `line` on the clipboard, saying so in the status row.
fn copy(state: &mut State, line: &str) {
    state.status = Some(match set_clipboard(line) {
        Ok(()) => (format!("copied: {line}"), Meaning::AlertInfo),
        Err(e) => (format!("copy failed: {e}"), Meaning::AlertError),
    });
}

impl Picker<'_> {
    /// Run the picker until a session is chosen or it's cancelled. Also returns what to tell the
    /// user once it's gone: the status line of a session continued in another harness.
    #[allow(clippy::too_many_lines)]
    pub async fn run(self) -> Result<(Outcome, Option<String>)> {
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

        let mouse = sessions.mouse.unwrap_or(!settings.no_mouse);
        let out = terminal::TuiStdout::new(inline_height > 0, !mouse)?;
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
        let (requests, mut responses) = worker::spawn(self.source, self.resumer.clone());
        let resumer = self.resumer.as_ref();

        if inline_height > 0 {
            terminal.clear()?;
        }
        // Paint before the first search answers, so the picker shows up immediately.
        terminal.draw(|f| state.draw(f, settings, self.theme))?;
        send_search(&mut state, &requests);

        let mut events = terminal::Events::new();
        // Ticks keep relative times and live dots current, and refresh the list now and then so
        // running sessions move and their previews catch up.
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_input = std::time::Instant::now();
        let mut last_refresh = std::time::Instant::now();
        let mut settle = Settle::default();
        // What to tell the user once the picker is gone (a continuation's status line; a
        // catch-up's is left in `state.note`).
        let mut note = None;
        let outcome = 'render: loop {
            request_details(&mut state, &requests, &mut settle);
            terminal.draw(|f| state.draw(f, settings, self.theme))?;
            state.preview_drawn(Instant::now());
            let hold_ends = state.hold_ends();

            tokio::select! {
                event = events.next() => {
                    let Some(event) = event else { break Outcome::Cancelled };
                    last_input = std::time::Instant::now();
                    let action = state.handle_input(settings, &event?);
                    let pending = match action {
                        InputAction::Continue => None,
                        // ctrl-l, or the terminal resized: start from a blank screen.
                        InputAction::Redraw => {
                            terminal.clear()?;
                            None
                        }
                        InputAction::Resume(_) => Some(Pending::Resume),
                        InputAction::ReturnCommand(_) => Some(Pending::Edit),
                        InputAction::Copy(_) => Some(Pending::Copy),
                        InputAction::Pick(None, action) => {
                            if let Some(outcome) =
                                resume_original(&mut state, action, resumer, &requests)
                            {
                                break 'render outcome;
                            }
                            None
                        }
                        InputAction::Pick(Some(target), action) => {
                            start_continuation(&mut state, target, action, &requests);
                            None
                        }
                        InputAction::ReturnOriginal | InputAction::Exit => break Outcome::Cancelled,
                    };
                    if let Some(action) = pending
                        && let Some(outcome) = accept(
                            &mut state,
                            action,
                            resumer,
                            &requests,
                            sessions.resume_chooser,
                        )
                    {
                        break 'render outcome;
                    }
                }
                // The selection settled: ask for its details (at the top of the loop).
                () = tokio::time::sleep_until(settle.due.unwrap_or_else(Instant::now).into()),
                    if settle.due.is_some() => {}
                // The preview held over from the last selection gives way to the new one's `…`.
                () = tokio::time::sleep_until(hold_ends.unwrap_or_else(Instant::now).into()),
                    if hold_ends.is_some() => {}
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
                    match response {
                        Some(Response::Continued(handle, result)) => {
                            if let Some((outcome, status)) =
                                finish_continuation(&mut state, &handle, result)
                            {
                                note = Some(status);
                                break 'render outcome;
                            }
                        }
                        Some(response) => apply_response(&mut state, response, &requests),
                        None => {}
                    }
                    // An enter/tab/ctrl-y waiting on this session's plan can finish now.
                    if let Some((handle, pending)) = state.pending.clone()
                        && state.plans.contains_key(&handle)
                        && let Some(outcome) =
                            resume_original(&mut state, pending, resumer, &requests)
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

        // Stop reading input before the terminal is handed back.
        drop(events);
        if inline_height > 0 {
            // Clear from the viewport's origin down and leave the cursor there. Not with
            // `Terminal::clear`, which first asks the terminal where the cursor is: nothing here
            // needs the answer, and a late one would fail a picker that has already finished.
            let origin = terminal.get_frame().area().as_position();
            terminal.set_cursor_position(origin)?;
            crossterm::execute!(
                terminal.backend_mut(),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
            )?;
        }
        Ok((outcome, note.or(state.note)))
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
