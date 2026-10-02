//! The picker's state: input, filter mode, results, and key handling.
//!
//! Mirrors the history search's `State` (resolve a key to an action, then execute it), minus the
//! parts that only make sense for history (search modes, contexts, deletion).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use atuin_client::ai_session::HarnessSession;
use atuin_client::settings::{AiSessionFilterMode as FilterMode, KeymapMode, Settings};
use atuin_client::theme::Meaning;
use atuin_client::tui::cursor::Cursor;
use atuin_client::tui::key::{KeyCodeValue, KeyInput, SingleKey};
use atuin_common::time::OffsetDateTimeExt as _;
use atuin_domain::record::HostId;
use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use time::OffsetDateTime;

use super::keymap::{Action, Keymap, KeymapSet};
use super::query::{self, ParsedQuery};
use super::rebuild::Rebuilding;
use super::resumer::{NotResumable, Resume};
use super::source::{SessionFilter, SessionPreview, SessionRow};
use super::{ResumeContext, panel};

pub const TAB_TITLES: [&str; 2] = ["Search", "Inspect"];

/// Sessions updated this recently are live: a dot in the row, and refreshed while open.
pub const LIVE_SECS: u64 = 120;

/// What [`State::requested`] tracks per session.
pub const PREVIEW: u8 = 0;
pub const CHILDREN: u8 = 1;
pub const PLAN: u8 = 2;
/// Restoring the session's transcript from sync, once an action is waiting on it.
pub const RESTORE: u8 = 3;

/// How many rows a search asks for.
pub const SEARCH_LIMIT: usize = 500;

/// How long the preview keeps showing the session it showed after the selection moves to one
/// whose preview isn't read yet: long enough to cover the read (and a held arrow key), so the
/// preview never blanks between two sessions, but short enough never to pass for the new one's.
pub const HOLD: Duration = Duration::from_millis(300);

/// How many lines a turn of the mouse wheel scrolls a preview.
pub const WHEEL_LINES: usize = 3;

/// The panes whose text scrolls: the preview strip under the list, the detail pane beside it on
/// wide terminals, and Inspect's conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Strip = 0,
    Side = 1,
    Inspect = 2,
}

const PANES: [Pane; 3] = [Pane::Strip, Pane::Side, Pane::Inspect];

/// A pane's scroll position, and where and how it was drawn last.
///
/// The position is the session's: moving to another session starts it at the top again. At the
/// top, a pane shows its overview (the first prompt, match and last reply sharing the lines);
/// scrolled, it shows the parts in full, one after another.
#[derive(Debug, Default, Clone)]
pub struct PaneScroll {
    /// The session scrolled.
    pub session: Option<HarnessSession>,
    /// The first line shown of the parts in full; 0 is the overview.
    pub offset: usize,
    /// Where the pane was drawn this frame (screen coordinates, so an inline viewport's rows are
    /// as the mouse reports them), or `None` when it wasn't.
    pub area: Option<Rect>,
    /// How many lines of text it had room for.
    pub height: usize,
    /// The lines of the parts in full rendered so far.
    pub len: usize,
    /// More lines than `len`, not rendered yet (they are as it scrolls down).
    pub more: bool,
}

impl PaneScroll {
    /// Where `session`'s text starts: 0 for a session other than the one scrolled.
    pub fn offset_for(&self, session: &HarnessSession) -> usize {
        if self.session.as_ref() == Some(session) {
            self.offset
        } else {
            0
        }
    }

    /// The furthest the pane can scroll, as far as it was rendered: past the end is let through
    /// while there's more to render.
    pub fn max_offset(&self) -> usize {
        if self.more {
            self.len
        } else {
            self.len.saturating_sub(self.height)
        }
    }

    /// Scroll by `by` lines (up when negative), within the text.
    fn scroll(&mut self, by: isize) {
        let offset = self.offset.saturating_add_signed(by);
        self.offset = offset.min(self.max_offset());
    }
}

/// What the event loop should do after an input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Continue,
    Redraw,
    /// Resume the session acted on ([`State::target`]) now.
    Resume,
    /// Put the session's resume command on the command line.
    ReturnCommand,
    /// Copy the session's resume command, and stay open.
    Copy,
    ReturnOriginal,
    Exit,
}

/// An action waiting for the selected session's resume plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    Resume,
    Edit,
    Copy,
}

/// Why the filter shows more than the configured mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Widened {
    /// Not in a git repository, so workspace can't apply.
    NoRepo,
    /// The workspace had no sessions (matching the query).
    NoMatches,
}

/// Inspect's list of the forks grouped under the session inspected, once expanded (`c`): it has
/// the arrow keys, with a cursor, and scrolls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildrenView {
    /// The session whose children these are: selecting another collapses the list.
    pub session: HarnessSession,
    pub cursor: usize,
    /// The first row shown.
    pub offset: usize,
    /// How many rows showed last time it was drawn (a page).
    pub height: usize,
}

/// The selection and scroll position of the session list.
#[derive(Debug, Default)]
pub struct ListState {
    pub offset: usize,
    pub selected: usize,
    pub max_entries: usize,
}

#[allow(clippy::struct_excessive_bools)]
pub struct State {
    pub input: Cursor,
    pub keymap_mode: KeymapMode,
    keymaps: KeymapSet,
    pending_vim_key: Option<char>,
    pub tab_index: usize,
    pub list: ListState,
    pub results: Vec<SessionRow>,

    pub context: ResumeContext,
    pub mode: FilterMode,
    pub widened: Option<Widened>,
    /// Widen workspace to global when it has no matches. Only for the default filter, and only
    /// until the user picks a mode with ctrl-r.
    auto_widen: bool,

    /// The generation of the newest search sent to the worker.
    pub issued: u64,
    /// The generation whose results are on screen.
    pub applied: u64,
    last_filter: Option<SessionFilter>,
    /// The generation of a refresh in flight, whose results keep the selection.
    refreshing: Option<u64>,

    pub previews: HashMap<HarnessSession, SessionPreview>,
    /// Previews read again on a refresh (their sessions are live): shown as they are until the
    /// new ones come.
    pub stale: HashSet<HarnessSession>,
    /// The session whose preview was shown last, and when a frame was first drawn with another
    /// selected: the preview keeps showing it for [`HOLD`] after the selection moves to one not
    /// read yet.
    pub shown: Option<(SessionRow, Option<Instant>)>,
    /// The tallest the preview strip has been (with `preview.strategy = "auto"`): it doesn't
    /// shrink again, so the list doesn't jump as the selection moves.
    pub strip_height: u16,
    /// Each scrolling pane's position, by [`Pane`].
    pub scrolls: [PaneScroll; 3],
    /// The forks grouped under each session, once read (see
    /// [`super::source::SessionSource::children`]).
    pub children: HashMap<HarnessSession, Vec<SessionRow>>,
    /// Inspect's list of forks, while it's expanded.
    pub children_view: Option<ChildrenView>,
    /// Details asked of the worker and not answered yet. The worker drops a request superseded by
    /// a newer one of its kind, so only the selected session's entries, and those of the session
    /// an action waits on, are kept (see [`Self::forget_unanswered`]).
    pub requested: HashSet<(HarnessSession, u8)>,
    /// Sessions named by id, kept first while the query is still the id they were named by: one
    /// that can't be resumed, so the picker opens on it and says why, or the several an id
    /// names, to pick one.
    pinned: Option<(Vec<SessionRow>, String)>,
    /// Resume plans, fetched for the selected session only (planning may walk directories). A
    /// plan to restore the session from sync is replaced by the plain one once it is restored.
    pub plans: HashMap<HarnessSession, Result<Resume, NotResumable>>,
    /// An enter/tab/ctrl-y waiting for its session's plan.
    pub pending: Option<(HarnessSession, Pending)>,

    /// A one-line message in the status row (copied, can't resume, search failed).
    pub status: Option<(String, Meaning)>,
    /// The daemon is rebuilding the session index, so results may be incomplete: said in the
    /// status row while it has nothing else to say.
    pub rebuilding: Option<Rebuilding>,
    pub now: Box<dyn Fn() -> OffsetDateTime + Send>,
}

impl State {
    pub fn new(settings: &Settings, context: ResumeContext, query: &str) -> Self {
        let sessions = &settings.ai.sessions;
        let mut input = Cursor::from(query.to_owned());
        input.end();

        let mut state = Self {
            input,
            keymap_mode: match settings.keymap_mode {
                KeymapMode::Auto => KeymapMode::Emacs,
                mode => mode,
            },
            keymaps: KeymapSet::defaults(settings),
            pending_vim_key: None,
            tab_index: 0,
            list: ListState::default(),
            results: Vec::new(),
            context,
            mode: FilterMode::Global,
            widened: None,
            auto_widen: false,
            issued: 0,
            applied: 0,
            last_filter: None,
            refreshing: None,
            previews: HashMap::new(),
            stale: HashSet::new(),
            shown: None,
            strip_height: 0,
            scrolls: Default::default(),
            children: HashMap::new(),
            children_view: None,
            requested: HashSet::new(),
            pinned: None,
            plans: HashMap::new(),
            pending: None,
            status: None,
            rebuilding: None,
            now: if settings.prefers_reduced_motion {
                let now = OffsetDateTime::now_utc();
                Box::new(move || now)
            } else {
                Box::new(OffsetDateTime::now_utc)
            },
        };
        state.set_initial_mode(sessions.filter_mode);
        state
    }

    /// Pick the opening filter: the configured one if it can apply here, else workspace, widening
    /// to global outside a repository.
    fn set_initial_mode(&mut self, configured: Option<FilterMode>) {
        if let Some(mode) = configured.filter(|m| self.mode_available(*m)) {
            self.mode = mode;
            return;
        }
        if self.mode_available(FilterMode::Workspace) {
            self.mode = FilterMode::Workspace;
            self.auto_widen = configured.is_none();
        } else {
            self.mode = FilterMode::Global;
            if configured.is_none() || configured == Some(FilterMode::Workspace) {
                self.widened = Some(Widened::NoRepo);
            }
        }
    }

    pub fn mode_available(&self, mode: FilterMode) -> bool {
        match mode {
            FilterMode::Workspace => self.context.git_root.is_some(),
            FilterMode::Branch => self.context.git_root.is_some() && self.context.branch.is_some(),
            FilterMode::Global | FilterMode::Host | FilterMode::Directory => true,
        }
    }

    /// The mode ctrl-r goes to next: the next available one in [`FilterMode::CYCLE`], wrapping
    /// around. `None` when there is no other.
    pub fn next_mode(&self) -> Option<FilterMode> {
        let modes = FilterMode::CYCLE;
        let at = modes.iter().position(|m| *m == self.mode).unwrap_or(modes.len() - 1);
        (1..=modes.len())
            .map(|step| modes[(at + step) % modes.len()])
            .find(|mode| self.mode_available(*mode))
            .filter(|mode| *mode != self.mode)
    }

    /// ctrl-r: the next available mode in [`FilterMode::CYCLE`].
    pub fn cycle_filter_mode(&mut self) {
        if let Some(mode) = self.next_mode() {
            self.mode = mode;
        }
        self.widened = None;
        self.auto_widen = false;
    }

    pub fn parsed_query(&self) -> ParsedQuery {
        query::parse(self.input.as_str())
    }

    /// The source filter for the current mode and query. Forks are always grouped under their
    /// roots.
    pub fn filter(&self) -> SessionFilter {
        let q = self.parsed_query();
        let ctx = &self.context;
        let mut filter = SessionFilter {
            text: q.text,
            limit: SEARCH_LIMIT,
            ..SessionFilter::default()
        };
        let db = &mut filter.db;
        db.harness = q.harness;
        db.model = q.model;
        db.branch = q.branch;
        db.roots_only = true;
        match self.mode {
            FilterMode::Global => {}
            FilterMode::Host => {
                // This host's sessions from before hosts were recorded are its own. An id that
                // isn't a UUID names no recorded host: only those.
                let id = uuid::Uuid::try_parse(&ctx.host_id).unwrap_or_default();
                db.host = Some(HostId(id));
                db.or_unrecorded = true;
            }
            FilterMode::Workspace => db.workspace.clone_from(&ctx.git_root),
            FilterMode::Directory => db.directory = Some(ctx.cwd.clone()),
            FilterMode::Branch => {
                db.workspace.clone_from(&ctx.git_root);
                if db.branch.is_none() {
                    db.branch.clone_from(&ctx.branch);
                }
            }
        }
        filter
    }

    /// The next search to run, if the filter changed since the last one sent. Bumps the
    /// generation, so results for anything older are dropped when they arrive.
    pub fn next_search(&mut self) -> Option<(u64, FilterMode, SessionFilter)> {
        let filter = self.filter();
        if self.last_filter.as_ref() == Some(&filter) {
            return None;
        }
        self.last_filter = Some(filter.clone());
        self.issued += 1;
        Some((self.issued, self.mode, filter))
    }

    /// Apply a search's results. Stale generations are dropped (the list on screen stays until
    /// the newest search answers). An empty workspace widens to global once, returning `true` so
    /// the caller searches again.
    pub fn apply_results(
        &mut self,
        generation: u64,
        mode: FilterMode,
        mut rows: Vec<SessionRow>,
    ) -> bool {
        if generation != self.issued {
            return false;
        }
        if let Some((pinned, query)) = &self.pinned {
            if self.input.as_str() == query {
                rows.retain(|r| pinned.iter().all(|p| p.handle != r.handle));
                rows.splice(0..0, pinned.iter().cloned());
            } else {
                self.pinned = None;
                self.status = None;
            }
        }
        if rows.is_empty() && self.auto_widen && mode == FilterMode::Workspace {
            self.auto_widen = false;
            self.mode = FilterMode::Global;
            self.widened = Some(Widened::NoMatches);
            return true;
        }
        let selected = self.selected().map(|r| r.handle.clone());
        self.results = rows;
        self.applied = generation;
        // A refresh keeps the selection on the same session; a new query starts at the best
        // match, as the history search does.
        let keep = self.refreshing.take() == Some(generation);
        let index = selected
            .filter(|_| keep)
            .and_then(|h| self.results.iter().position(|r| r.handle == h))
            .unwrap_or(0);
        self.list.selected = index;
        true
    }

    /// Search again with the same filter, so live sessions move and their previews catch up.
    /// Skipped while a search is still out.
    pub fn refresh(&mut self) -> Option<(u64, FilterMode, SessionFilter)> {
        if self.issued != self.applied {
            return None;
        }
        self.last_filter = None;
        let next = self.next_search()?;
        self.refreshing = Some(next.0);
        let now = (self.now)();
        let live: Vec<HarnessSession> = self
            .results
            .iter()
            .filter(|r| now.saturating_duration_since(r.updated_at).as_secs() < LIVE_SECS)
            .map(|r| r.handle.clone())
            .collect();
        // Read again, and shown as they are meanwhile: dropping them blanked the preview (and
        // shrank it, moving the list) until the new ones came.
        for handle in live {
            self.requested.remove(&(handle.clone(), PREVIEW));
            if self.previews.contains_key(&handle) {
                self.stale.insert(handle);
            }
        }
        Some(next)
    }

    /// Whether `session`'s preview is to be read: it isn't yet, or a refresh wants it again.
    pub fn wants_preview(&self, session: &HarnessSession) -> bool {
        !self.previews.contains_key(session) || self.stale.contains(session)
    }

    /// A preview read.
    pub fn apply_preview(&mut self, session: HarnessSession, preview: SessionPreview) {
        self.stale.remove(&session);
        self.previews.insert(session, preview);
    }

    /// The session the preview shows at `now`: the selected one once its preview is read. Until
    /// then, the one it showed before, for up to [`HOLD`] after the selection left it, rather
    /// than nothing; past that, the selected one, waiting (`…`).
    pub fn preview_row_at(&self, now: Instant) -> Option<&SessionRow> {
        let selected = self.selected()?;
        if self.previews.contains_key(&selected.handle) {
            return Some(selected);
        }
        match &self.shown {
            Some((row, left))
                if left.is_none_or(|at| now.saturating_duration_since(at) < HOLD)
                    && self.previews.contains_key(&row.handle) =>
            {
                Some(row)
            }
            _ => Some(selected),
        }
    }

    /// The session the preview shows now (see [`Self::preview_row_at`]).
    pub fn preview_row(&self) -> Option<&SessionRow> {
        self.preview_row_at(Instant::now())
    }

    /// Note a frame drawn at `now`: while the selected session's preview is read, it is the one
    /// to hold when the selection moves on; once it has, the hold runs from the first frame
    /// drawn without it.
    pub fn preview_drawn(&mut self, now: Instant) {
        let Some(selected) = self.selected().cloned() else {
            return;
        };
        if self.previews.contains_key(&selected.handle) {
            // The row as it is now: a refresh may have changed it.
            self.shown = Some((selected, None));
        } else if let Some((row, left @ None)) = &mut self.shown
            && row.handle != selected.handle
        {
            *left = Some(now);
        }
    }

    /// When a preview held for a selection not read yet gives way to it (`…`): the picker draws
    /// again then.
    pub fn hold_ends(&self) -> Option<Instant> {
        let selected = self.selected()?;
        if self.previews.contains_key(&selected.handle) {
            return None;
        }
        let (row, left) = self.shown.as_ref()?;
        let ends = (*left)? + HOLD;
        (row.handle != selected.handle && ends > Instant::now()).then_some(ends)
    }

    /// The scrolling pane drawn last: the one the preview keys move.
    fn drawn_pane(&self) -> Option<Pane> {
        PANES.into_iter().find(|p| self.scrolls[*p as usize].area.is_some())
    }

    /// Scroll `pane` by `by` lines (up when negative), within what it was drawn with.
    pub fn scroll_pane(&mut self, pane: Pane, by: isize) {
        self.scrolls[pane as usize].scroll(by);
    }

    pub fn selected(&self) -> Option<&SessionRow> {
        self.results.get(self.list.selected)
    }

    /// The session enter, tab and ctrl-y act on: the fork highlighted in Inspect's expanded list
    /// of them, or else the selected session.
    pub fn target(&self) -> Option<&SessionRow> {
        if let Some(view) = self.expanded_children()
            && let Some(children) = self.children.get(&view.session)
            && let Some(&(_, i)) = panel::tree_order(&view.session, children).get(view.cursor)
        {
            return children.get(i);
        }
        self.selected()
    }

    /// Open on `row`, which the query named by id but which can't be resumed: it is selected, its
    /// plan is the reason (shown in the status row), and it stays first until the query changes.
    pub fn pin(&mut self, row: SessionRow, why: NotResumable) {
        self.status =
            Some((format!("can't resume {}: {why}", row.handle.session), Meaning::AlertError));
        self.plans.insert(row.handle.clone(), Err(why));
        self.pin_rows(vec![row]);
    }

    /// Open on `rows`, the sessions the query names by id (a prefix of several ids, or an id
    /// several agents have): they come first, the first selected, until the query changes.
    pub fn pin_matches(&mut self, rows: Vec<SessionRow>) {
        self.status = Some((
            format!("{} names {} sessions: pick one", self.input.as_str().trim(), rows.len()),
            Meaning::AlertInfo,
        ));
        self.pin_rows(rows);
    }

    fn pin_rows(&mut self, rows: Vec<SessionRow>) {
        self.results.clone_from(&rows);
        self.list.selected = 0;
        self.pinned = Some((rows, self.input.as_str().to_owned()));
    }

    /// Forget unanswered requests for sessions other than the selected one: the worker drops
    /// those once a newer request of the same kind arrives, so they must be asked for again. Not
    /// those of the session an enter, tab or ctrl-y waits on (a fork, in Inspect): only another
    /// action supersedes what it asked for, so asking again would only do it twice.
    pub fn forget_unanswered(&mut self) {
        let selected = self.selected().map(|r| r.handle.clone());
        let waiting = self.pending.as_ref().map(|(handle, _)| handle.clone());
        self.requested.retain(|(handle, _)| {
            selected.as_ref() == Some(handle) || waiting.as_ref() == Some(handle)
        });
    }

    /// What the status row says: the latest message, else that the index is being rebuilt.
    pub fn status_line(&self) -> Option<(String, Meaning)> {
        self.status.clone().or_else(|| self.rebuilding.map(|r| (r.status(), Meaning::AlertWarn)))
    }

    /// The label in the input's `[ MODE ]` prefix.
    pub fn mode_label(&self) -> &'static str {
        match self.widened {
            Some(Widened::NoMatches | Widened::NoRepo) => "WS→GLOBAL",
            None => self.mode.as_str(),
        }
    }

    // --- input ---------------------------------------------------------------------------------

    #[must_use]
    pub fn handle_input(&mut self, settings: &Settings, event: &Event) -> InputAction {
        match event {
            Event::Key(k) => self.handle_key_input(settings, k),
            Event::Mouse(m) => self.handle_mouse_input(settings, *m),
            Event::Paste(text) => {
                if self.tab_index == 0 {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        self.input.insert(c);
                    }
                }
                InputAction::Continue
            }
            Event::Resize(..) => InputAction::Redraw,
            _ => InputAction::Continue,
        }
    }

    /// The wheel over a preview scrolls it; anywhere else (the list, the input) it moves the
    /// selection, as in the history search. The mouse reports screen positions, which is what
    /// the panes' areas are, inline or not.
    fn handle_mouse_input(&mut self, settings: &Settings, event: MouseEvent) -> InputAction {
        let down = match event.kind {
            MouseEventKind::ScrollDown => true,
            MouseEventKind::ScrollUp => false,
            _ => return InputAction::Continue,
        };
        let at = Position::new(event.column, event.row);
        let over = PANES
            .into_iter()
            .find(|p| self.scrolls[*p as usize].area.is_some_and(|a| a.contains(at)));
        if let Some(pane) = over {
            let lines = isize::try_from(WHEEL_LINES).unwrap_or(1);
            self.scroll_pane(
                pane,
                if down {
                    lines
                } else {
                    -lines
                },
            );
            return InputAction::Continue;
        }
        let action = if down {
            Action::SelectNext
        } else {
            Action::SelectPrevious
        };
        self.execute_action(action, settings)
    }

    fn mode_keymap(&self) -> &Keymap {
        if self.tab_index == 1 {
            &self.keymaps.inspector
        } else {
            self.keymaps.for_mode(self.keymap_mode)
        }
    }

    fn is_insert_mode(&self) -> bool {
        self.tab_index == 0 && matches!(self.keymap_mode, KeymapMode::Emacs | KeymapMode::VimInsert)
    }

    #[must_use]
    pub fn handle_key_input(&mut self, settings: &Settings, input: &KeyEvent) -> InputAction {
        if input.kind == KeyEventKind::Release {
            return InputAction::Continue;
        }
        let Some(single) = SingleKey::from_event(input) else {
            return InputAction::Continue;
        };
        let ctx = self.input.as_str().is_empty();
        let pending = self.pending_vim_key.take();
        let keymap = self.mode_keymap();

        let (action, new_pending) = if let Some(pending_char) = pending {
            let first = SingleKey {
                code: KeyCodeValue::Char(pending_char),
                ctrl: false,
                alt: false,
                shift: false,
                super_key: false,
            };
            let seq = KeyInput::Sequence(vec![first, single.clone()]);
            let action = keymap
                .resolve(&seq, ctx)
                .or_else(|| keymap.resolve(&KeyInput::Single(single.clone()), ctx));
            (action, None)
        } else if let KeyCodeValue::Char(c) = single.code
            && !single.ctrl
            && !single.alt
            && keymap.has_sequence_starting_with(&single)
        {
            (Some(Action::Noop), Some(c))
        } else {
            (keymap.resolve(&KeyInput::Single(single.clone()), ctx), None)
        };
        self.pending_vim_key = new_pending;

        if let Some(action) = action {
            return self.execute_action(action, settings);
        }
        if self.is_insert_mode() && !single.ctrl && !single.alt {
            match single.code {
                KeyCodeValue::Char(c) => self.input.insert(c),
                KeyCodeValue::Space => self.input.insert(' '),
                _ => {}
            }
        }
        InputAction::Continue
    }

    /// Move the selection toward index 0 (the best match, drawn at the bottom unless inverted).
    fn scroll_down(&mut self, n: usize) {
        self.list.selected = self.list.selected.saturating_sub(n);
    }

    fn scroll_up(&mut self, n: usize) {
        let last = self.results.len().saturating_sub(1);
        self.list.selected = (self.list.selected + n).min(last);
    }

    fn set_vim_mode(&mut self, mode: KeymapMode) {
        self.keymap_mode = mode;
    }

    /// The expanded children list, if it is the selected session's.
    pub fn expanded_children(&self) -> Option<&ChildrenView> {
        let selected = self.selected()?;
        self.children_view.as_ref().filter(|v| v.session == selected.handle && self.tab_index == 1)
    }

    /// `c` in Inspect: expand the selected session's children, or collapse them.
    fn toggle_children(&mut self) {
        if self.expanded_children().is_some() {
            self.children_view = None;
            return;
        }
        let Some(row) = self.selected() else {
            return;
        };
        // Until the forks are read, a row with anything grouped under it may have some.
        let forks = self
            .children
            .get(&row.handle)
            .map_or(usize::try_from(row.children).unwrap_or(1), Vec::len);
        if forks == 0 {
            return;
        }
        self.children_view = Some(ChildrenView {
            session: row.handle.clone(),
            cursor: 0,
            offset: 0,
            height: 1,
        });
    }

    /// A key while Inspect's children list is expanded: moving keys move in it, and esc
    /// collapses it. `None` for the keys it leaves alone.
    fn children_action(&mut self, action: Action) -> Option<InputAction> {
        let len = self
            .expanded_children()
            .map(|v| &v.session)
            .map(|s| self.children.get(s).map_or(0, Vec::len))?;
        let view = self.children_view.as_mut()?;
        let page = view.height.max(1);
        let last = len.saturating_sub(1);
        match action {
            Action::SelectNext => view.cursor = (view.cursor + 1).min(last),
            Action::SelectPrevious => view.cursor = view.cursor.saturating_sub(1),
            Action::ScrollPageDown => view.cursor = (view.cursor + page).min(last),
            Action::ScrollPageUp => view.cursor = view.cursor.saturating_sub(page),
            Action::ScrollHalfPageDown => view.cursor = (view.cursor + page / 2).min(last),
            Action::ScrollHalfPageUp => view.cursor = view.cursor.saturating_sub(page / 2),
            Action::ScrollToTop => view.cursor = 0,
            Action::ScrollToBottom => view.cursor = last,
            Action::Exit => self.children_view = None,
            _ => return None,
        }
        Some(InputAction::Continue)
    }

    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub fn execute_action(&mut self, action: Action, settings: &Settings) -> InputAction {
        if let Some(outcome) = self.children_action(action) {
            return outcome;
        }
        let invert = settings.invert;
        let page = self.list.max_entries.saturating_sub(settings.scroll_context_lines).max(1);
        let words = (settings.word_chars.as_str(), settings.word_jump_mode);

        match action {
            Action::CursorLeft => {
                self.input.left();
            }
            Action::CursorRight => self.input.right(),
            Action::CursorWordLeft => self.input.prev_word(words.0, words.1),
            Action::CursorWordRight => self.input.next_word(words.0, words.1),
            Action::CursorWordEnd => self.input.word_end(words.0),
            Action::CursorStart => self.input.start(),
            Action::CursorEnd => self.input.end(),
            Action::DeleteCharBefore => {
                self.input.back();
            }
            Action::DeleteCharAfter => {
                self.input.remove();
            }
            Action::DeleteWordBefore => self.input.remove_prev_word(words.0, words.1),
            Action::DeleteWordAfter => self.input.remove_next_word(words.0, words.1),
            Action::DeleteToWordBoundary => {
                while matches!(self.input.back(), Some(c) if c.is_whitespace()) {}
                while self.input.left() {
                    if self.input.char().is_some_and(char::is_whitespace) {
                        self.input.right();
                        break;
                    }
                    self.input.remove();
                }
            }
            Action::ClearLine => self.input.clear(),
            Action::ClearToEnd => self.input.clear_to_end(),

            Action::SelectNext if invert => self.scroll_up(1),
            Action::SelectNext => self.scroll_down(1),
            Action::SelectPrevious if invert => self.scroll_down(1),
            Action::SelectPrevious => self.scroll_up(1),
            Action::ScrollPageDown if invert => self.scroll_up(page),
            Action::ScrollPageDown => self.scroll_down(page),
            Action::ScrollPageUp if invert => self.scroll_down(page),
            Action::ScrollPageUp => self.scroll_up(page),
            Action::ScrollHalfPageDown if invert => self.scroll_up(page / 2),
            Action::ScrollHalfPageDown => self.scroll_down(page / 2),
            Action::ScrollHalfPageUp if invert => self.scroll_down(page / 2),
            Action::ScrollHalfPageUp => self.scroll_up(page / 2),
            Action::ScrollToTop if invert => self.list.selected = 0,
            Action::ScrollToTop => self.list.selected = self.results.len().saturating_sub(1),
            Action::ScrollToBottom if invert => {
                self.list.selected = self.results.len().saturating_sub(1);
            }
            Action::ScrollToBottom => self.list.selected = 0,

            Action::PreviewUp
            | Action::PreviewDown
            | Action::PreviewPageUp
            | Action::PreviewPageDown => {
                if let Some(pane) = self.drawn_pane() {
                    let page = self.scrolls[pane as usize].height.saturating_sub(1).max(1);
                    let page = isize::try_from(page).unwrap_or(1);
                    let by = match action {
                        Action::PreviewUp => -1,
                        Action::PreviewDown => 1,
                        Action::PreviewPageUp => -page,
                        _ => page,
                    };
                    self.scroll_pane(pane, by);
                }
            }

            Action::Resume => return InputAction::Resume,
            Action::ReturnCommand => return InputAction::ReturnCommand,
            Action::Copy => return InputAction::Copy,
            Action::ReturnOriginal => return InputAction::ReturnOriginal,
            // Nothing to clear: every frame is drawn whole, and clearing the terminal first only
            // flashed it blank.
            Action::Exit if self.tab_index == 1 => self.tab_index = 0,
            Action::Exit => return InputAction::Exit,
            Action::Redraw => return InputAction::Redraw,
            Action::CycleFilterMode => self.cycle_filter_mode(),
            Action::CycleAgent => {
                self.input = Cursor::from(query::cycle_harness(self.input.as_str()));
                self.input.end();
            }
            Action::ToggleTab => {
                self.tab_index = (self.tab_index + 1) % TAB_TITLES.len();
                self.children_view = None;
            }
            Action::ToggleChildren => self.toggle_children(),

            Action::VimEnterNormal => self.set_vim_mode(KeymapMode::VimNormal),
            Action::VimEnterInsert => self.set_vim_mode(KeymapMode::VimInsert),
            Action::VimEnterInsertAfter => {
                self.input.right();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimEnterInsertAtStart => {
                self.input.start();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimEnterInsertAtEnd => {
                self.input.end();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimSearchInsert => {
                self.input.clear();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::VimChangeToEnd => {
                self.input.clear_to_end();
                self.set_vim_mode(KeymapMode::VimInsert);
            }
            Action::Noop => {}
        }
        InputAction::Continue
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use atuin_client::ai_session::HarnessKind;
    use crossterm::event::{KeyCode, KeyModifiers};
    use rstest::rstest;

    use super::*;
    use crate::resume_tui::fake;

    fn settings() -> Settings {
        Settings::utc()
    }

    fn state_in(context: ResumeContext) -> State {
        State::new(&settings(), context, "")
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn rows(n: usize) -> Vec<SessionRow> {
        (0..n).map(|i| fake::row(HarnessKind::ClaudeCode, &format!("s{i}"), "t")).collect()
    }

    #[rstest]
    fn default_mode_is_workspace_in_a_repo() {
        let state = state_in(fake::context());
        assert_eq!(state.mode, FilterMode::Workspace);
        assert_eq!(state.widened, None);
        assert_eq!(state.filter().db.workspace, Some(PathBuf::from(fake::REPO)));
    }

    #[rstest]
    fn default_mode_widens_outside_a_repo() {
        let mut ctx = fake::context();
        ctx.git_root = None;
        ctx.branch = None;
        let state = state_in(ctx);
        assert_eq!(state.mode, FilterMode::Global);
        assert_eq!(state.widened, Some(Widened::NoRepo));
        assert_eq!(state.mode_label(), "WS→GLOBAL");
        assert_eq!(state.filter().db.workspace, None);
    }

    #[rstest]
    fn empty_workspace_widens_to_global_once() {
        let mut state = state_in(fake::context());
        let (generation, mode, _) = state.next_search().unwrap();
        assert!(state.apply_results(generation, mode, Vec::new()), "should re-search");
        assert_eq!(state.mode, FilterMode::Global);
        assert_eq!(state.widened, Some(Widened::NoMatches));

        let (generation, mode, filter) = state.next_search().unwrap();
        assert_eq!(filter.db.workspace, None);
        state.apply_results(generation, mode, rows(2));
        assert_eq!(state.results.len(), 2);

        // Once widened, an empty global result is just empty.
        state.input = Cursor::from("nothing matches this".to_owned());
        let (generation, mode, _) = state.next_search().unwrap();
        state.apply_results(generation, mode, Vec::new());
        assert_eq!(state.mode, FilterMode::Global);
        assert!(state.results.is_empty());
    }

    #[rstest]
    fn a_configured_mode_never_widens() {
        let mut settings = settings();
        settings.ai.sessions.filter_mode = Some(FilterMode::Workspace);
        let mut state = State::new(&settings, fake::context(), "");
        let (generation, mode, _) = state.next_search().unwrap();
        assert!(state.apply_results(generation, mode, Vec::new()));
        assert_eq!(state.mode, FilterMode::Workspace);
        assert_eq!(state.widened, None);
    }

    #[rstest]
    fn ctrl_r_cycles_available_modes_and_clears_widening() {
        let mut state = state_in(fake::context());
        let s = settings();
        let seen: Vec<_> = (0..5)
            .map(|_| {
                let _ = state.handle_input(&s, &key(KeyCode::Char('r'), KeyModifiers::CONTROL));
                state.mode
            })
            .collect();
        // From the workspace straight to every session.
        assert_eq!(seen, vec![
            FilterMode::Global,
            FilterMode::Host,
            FilterMode::Directory,
            FilterMode::Branch,
            FilterMode::Workspace,
        ]);

        let mut ctx = fake::context();
        ctx.git_root = None;
        ctx.branch = None;
        let mut state = state_in(ctx);
        assert_eq!(state.widened, Some(Widened::NoRepo));
        state.cycle_filter_mode();
        assert_eq!(state.mode, FilterMode::Host);
        assert_eq!(state.widened, None);
        state.cycle_filter_mode();
        state.cycle_filter_mode();
        // Workspace and branch are skipped without a repository.
        assert_eq!(state.mode, FilterMode::Global);
    }

    #[rstest]
    fn modes_resolve_to_filters() {
        let mut state = state_in(fake::context());
        state.mode = FilterMode::Host;
        let host = state.filter().db.host.map(|h| h.0.as_simple().to_string());
        assert_eq!(host.as_deref(), Some(fake::THIS_HOST_ID));
        assert!(state.filter().db.or_unrecorded);
        state.mode = FilterMode::Directory;
        assert_eq!(state.filter().db.directory, Some(PathBuf::from(fake::REPO)));
        state.mode = FilterMode::Branch;
        assert_eq!(state.filter().db.branch.as_deref(), Some("ai-resume"));
        state.input = Cursor::from("b:main flaky agent:codex".to_owned());
        let filter = state.filter();
        assert_eq!(filter.db.branch.as_deref(), Some("main"));
        assert_eq!(filter.text, "flaky");
        assert_eq!(filter.db.harness, Some(HarnessKind::Codex));
        assert!(filter.db.roots_only);
    }

    #[rstest]
    fn stale_generations_are_dropped_and_the_old_list_kept() {
        let mut state = state_in(fake::context());
        let (g1, mode, _) = state.next_search().unwrap();
        state.apply_results(g1, mode, rows(3));
        state.input = Cursor::from("a".to_owned());
        let (g2, _, _) = state.next_search().unwrap();
        state.input = Cursor::from("ab".to_owned());
        let (g3, _, _) = state.next_search().unwrap();
        assert!(g1 < g2 && g2 < g3);

        assert!(!state.apply_results(g2, mode, rows(1)));
        assert_eq!(state.results.len(), 3, "old list stays until the newest answers");
        assert_eq!(state.applied, g1);
        assert!(state.apply_results(g3, mode, rows(2)));
        assert_eq!(state.results.len(), 2);
        assert_eq!(state.applied, g3);
    }

    #[rstest]
    fn refresh_keeps_the_selection_and_reloads_live_previews() {
        let mut state = state_in(fake::context());
        state.now = Box::new(fake::now);
        let (g, mode, _) = state.next_search().unwrap();
        let mut rs = rows(3);
        rs[2].updated_at = fake::now();
        state.apply_results(g, mode, rs.clone());
        state.list.selected = 1;
        for r in &rs {
            state.previews.insert(r.handle.clone(), SessionPreview::default());
        }

        let (g, mode, _) = state.refresh().unwrap();
        assert!(state.refresh().is_none(), "one refresh at a time");
        // The live preview is read again, and shown as it was meanwhile.
        assert!(state.wants_preview(&rs[2].handle), "the live preview reloads");
        assert!(state.previews.contains_key(&rs[2].handle), "and is kept until it has");
        assert!(!state.wants_preview(&rs[0].handle));
        let fresh = SessionPreview {
            first_prompt: Some("new".to_owned()),
            ..SessionPreview::default()
        };
        state.apply_preview(rs[2].handle.clone(), fresh.clone());
        assert!(!state.wants_preview(&rs[2].handle));
        assert_eq!(state.previews[&rs[2].handle], fresh);
        // The list reorders; the selection follows its session.
        rs.swap(0, 1);
        state.apply_results(g, mode, rs.clone());
        assert_eq!(state.list.selected, 0);
        assert_eq!(state.selected().unwrap().handle, rs[0].handle);

        // A new query starts at the top again.
        state.list.selected = 2;
        state.input = Cursor::from("x".to_owned());
        let (g, mode, _) = state.next_search().unwrap();
        state.apply_results(g, mode, rs);
        assert_eq!(state.list.selected, 0);
    }

    #[rstest]
    fn a_pinned_session_stays_first_until_the_query_changes() {
        let mut state = State::new(&settings(), fake::context(), "s7a1b2c");
        let pinned = fake::row(HarnessKind::ClaudeCode, "s7a1b2c", "t");
        state.pin(pinned.clone(), NotResumable::NotInstalled("claude".to_owned()));
        assert_eq!(state.selected(), Some(&pinned));
        assert!(state.status.as_ref().is_some_and(|(s, _)| s.contains("isn't installed here")));

        // The id matches no text, so the workspace would widen; the pinned row keeps it.
        let (generation, mode, _) = state.next_search().unwrap();
        state.apply_results(generation, mode, Vec::new());
        assert_eq!(state.results, vec![pinned.clone()]);
        assert_eq!(state.widened, None);
        // A refresh that finds it too still shows it once, first.
        state.apply_results(generation, mode, vec![rows(2)[1].clone(), pinned.clone()]);
        assert_eq!(state.results[0], pinned);
        assert_eq!(state.results.len(), 2);

        state.input = Cursor::from("other".to_owned());
        let (generation, mode, _) = state.next_search().unwrap();
        state.apply_results(generation, mode, rows(2));
        assert!(!state.results.contains(&pinned));
        assert_eq!(state.status, None);
    }

    /// An id several sessions have opens on all of them, the first selected, ahead of anything
    /// the search finds, until the query changes.
    #[rstest]
    fn the_sessions_an_id_names_stay_first_until_the_query_changes() {
        let mut state = State::new(&settings(), fake::context(), "7aaabc31");
        let claude = fake::row(HarnessKind::ClaudeCode, "7aaabc31-1631", "a");
        let pi = fake::row(HarnessKind::Pi, "7aaabc31-1631", "b");
        state.pin_matches(vec![claude.clone(), pi.clone()]);
        assert_eq!(state.results, vec![claude.clone(), pi.clone()]);
        assert_eq!(state.selected(), Some(&claude));
        assert!(
            state.status.as_ref().is_some_and(|(s, _)| s == "7aaabc31 names 2 sessions: pick one")
        );
        // Nothing is planned for them up front: either may be resumable.
        assert!(state.plans.is_empty());

        let (generation, mode, _) = state.next_search().unwrap();
        let other = rows(2).remove(1);
        state.apply_results(generation, mode, vec![pi.clone(), other.clone()]);
        assert_eq!(state.results, vec![claude.clone(), pi, other]);
        assert_eq!(state.widened, None);

        state.input = Cursor::from("other".to_owned());
        let (generation, mode, _) = state.next_search().unwrap();
        state.apply_results(generation, mode, rows(2));
        assert!(!state.results.contains(&claude));
        assert_eq!(state.status, None);
    }

    /// Moving to a session not read yet holds the last preview shown, from the first frame drawn
    /// without it, until the new one is read or [`HOLD`] passes; never an empty preview between.
    #[rstest]
    fn the_last_preview_is_held_until_the_next_is_read() {
        let mut state = state_in(fake::context());
        state.results = rows(3);
        let [a, b, c] = [0, 1, 2].map(|i| state.results[i].handle.clone());
        let t0 = Instant::now();
        state.apply_preview(a.clone(), SessionPreview::default());
        state.preview_drawn(t0);

        state.list.selected = 1;
        // Long after the last frame drawn with it selected: the hold starts with the move.
        let t1 = t0 + 10 * HOLD;
        assert_eq!(state.preview_row_at(t1).unwrap().handle, a);
        state.preview_drawn(t1);
        state.list.selected = 2;
        state.preview_drawn(t1 + HOLD / 2);
        assert_eq!(state.preview_row_at(t1 + HOLD / 2).unwrap().handle, a);
        assert_eq!(state.preview_row_at(t1 + HOLD).unwrap().handle, c, "then the selected");

        state.apply_preview(b.clone(), SessionPreview::default());
        state.list.selected = 1;
        assert_eq!(state.preview_row_at(t1 + HOLD).unwrap().handle, b);
        state.preview_drawn(t1 + HOLD);
        state.list.selected = 2;
        assert_eq!(state.preview_row_at(t1 + 2 * HOLD).unwrap().handle, b);
    }

    /// The wheel scrolls within the text drawn: not above its top, nor past its end once all of
    /// it is rendered; while more is, past what was rendered, for the next frame to render it.
    #[rstest]
    fn panes_scroll_within_their_text() {
        let mut scroll = PaneScroll {
            session: Some(rows(1)[0].handle.clone()),
            height: 4,
            len: 10,
            ..PaneScroll::default()
        };
        scroll.scroll(-3);
        assert_eq!(scroll.offset, 0);
        scroll.scroll(100);
        assert_eq!(scroll.offset, 6);
        scroll.more = true;
        scroll.scroll(100);
        assert_eq!(scroll.offset, 10);
        assert_eq!(scroll.offset_for(&rows(2)[1].handle), 0, "another session's is the top");
    }

    #[rstest]
    fn forgetting_unanswered_requests_keeps_the_selected_sessions() {
        let mut state = state_in(fake::context());
        state.results = rows(3);
        for row in rows(3) {
            state.requested.insert((row.handle, PREVIEW));
        }
        state.list.selected = 1;
        state.forget_unanswered();
        let kept: Vec<_> = state.requested.iter().map(|(h, _)| h.clone()).collect();
        assert_eq!(kept, vec![rows(3)[1].handle.clone()]);

        // And the session an action waits on.
        let fork = rows(3)[2].handle.clone();
        state.requested.insert((fork.clone(), PLAN));
        state.pending = Some((fork.clone(), Pending::Resume));
        state.forget_unanswered();
        assert!(state.requested.contains(&(fork, PLAN)));
        assert_eq!(state.requested.len(), 2);
    }

    #[rstest]
    fn unchanged_filter_does_not_search_again() {
        let mut state = state_in(fake::context());
        assert!(state.next_search().is_some());
        assert!(state.next_search().is_none());
        state.input.insert(' ');
        // Whitespace doesn't change the parsed filter.
        assert!(state.next_search().is_none());
    }

    #[rstest]
    fn alt_a_rewrites_the_input() {
        let mut state = state_in(fake::context());
        let s = settings();
        state.input = Cursor::from("flaky".to_owned());
        let _ = state.handle_input(&s, &key(KeyCode::Char('a'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "flaky agent:claude-code");
        let _ = state.handle_input(&s, &key(KeyCode::Char('a'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "flaky agent:codex");
        // The short form is cycled too, and comes back as the long one.
        state.input = Cursor::from("a:opencode flaky".to_owned());
        let _ = state.handle_input(&s, &key(KeyCode::Char('a'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "agent:pi flaky");
        // alt-h is unbound: it neither cycles nor types.
        let _ = state.handle_input(&s, &key(KeyCode::Char('h'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "agent:pi flaky");
    }

    #[rstest]
    fn keys_map_to_outcomes() {
        let mut s = settings();
        s.enter_accept = true;
        let mut state = State::new(&s, fake::context(), "");
        state.results = rows(3);
        state.list.selected = 1;

        let press = |state: &mut State, code, m| state.handle_input(&s, &key(code, m));
        assert_eq!(press(&mut state, KeyCode::Enter, KeyModifiers::NONE), InputAction::Resume);
        assert_eq!(press(&mut state, KeyCode::Tab, KeyModifiers::NONE), InputAction::ReturnCommand);
        assert_eq!(press(&mut state, KeyCode::Char('y'), KeyModifiers::CONTROL), InputAction::Copy);
        assert_eq!(state.target(), Some(&state.results[1]));
        assert_eq!(
            press(&mut state, KeyCode::Char('c'), KeyModifiers::CONTROL),
            InputAction::ReturnOriginal
        );
        assert_eq!(
            press(&mut state, KeyCode::Char('g'), KeyModifiers::CONTROL),
            InputAction::ReturnOriginal
        );
        assert_eq!(press(&mut state, KeyCode::Esc, KeyModifiers::NONE), InputAction::Exit);

        // ctrl-o opens Inspect; esc there goes back instead of exiting. Neither clears the
        // terminal (a blank frame, then the whole screen drawn again).
        assert_eq!(
            press(&mut state, KeyCode::Char('o'), KeyModifiers::CONTROL),
            InputAction::Continue
        );
        assert_eq!(state.tab_index, 1);
        assert_eq!(press(&mut state, KeyCode::Esc, KeyModifiers::NONE), InputAction::Continue);
        assert_eq!(state.tab_index, 0);
        assert_eq!(
            press(&mut state, KeyCode::Char('l'), KeyModifiers::CONTROL),
            InputAction::Redraw
        );
    }

    #[rstest]
    fn typing_inserts_and_navigation_respects_invert() {
        let mut s = settings();
        let mut state = State::new(&s, fake::context(), "");
        state.results = rows(5);
        for c in "bug".chars() {
            let _ = state.handle_input(&s, &key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(state.input.as_str(), "bug");

        // Not inverted: the best match is at the bottom, so up moves away from it.
        let _ = state.handle_input(&s, &key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.list.selected, 1);
        let _ = state.handle_input(&s, &key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.list.selected, 0);

        s.invert = true;
        let _ = state.handle_input(&s, &key(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.list.selected, 1);
    }

    #[rstest]
    fn vim_modes() {
        let mut s = settings();
        s.keymap_mode = KeymapMode::VimInsert;
        let mut state = State::new(&s, fake::context(), "");
        state.results = rows(5);
        let _ = state.handle_input(&s, &key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(state.input.as_str(), "k");
        let _ = state.handle_input(&s, &key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(state.keymap_mode, KeymapMode::VimNormal);
        let _ = state.handle_input(&s, &key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(state.list.selected, 1);
        let _ = state.handle_input(&s, &key(KeyCode::Char('g'), KeyModifiers::NONE));
        let _ = state.handle_input(&s, &key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(state.list.selected, 4, "gg jumps to the visual top");
        let _ = state.handle_input(&s, &key(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(state.keymap_mode, KeymapMode::VimInsert);
    }
}
