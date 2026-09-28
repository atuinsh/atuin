//! The picker's state: input, filter mode, results, and key handling.
//!
//! Mirrors the history search's `State` (resolve a key to an action, then execute it), minus the
//! parts that only make sense for history (search modes, contexts, deletion).

use std::collections::{HashMap, HashSet};

use atuin_client::ai_session::{HarnessKind, HarnessSession};
use atuin_client::settings::{AiSessionFilterMode as FilterMode, KeymapMode, Settings};
use atuin_client::theme::Meaning;
use atuin_client::tui::{Cursor, EvalContext, KeyCodeValue, KeyInput, SingleKey};
use atuin_common::harnesstools::continuation::Flattened;
use atuin_common::time::OffsetDateTimeExt as _;
use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use super::ResumeContext;
use super::chooser::{Chooser, ListAnchor};
use super::keymap::{Action, Keymap, KeymapSet};
use super::query::{self, ParsedQuery};
use super::resumer::{NotResumable, Resume};
use super::source::{SessionFilter, SessionPreview, SessionRow};

pub const TAB_TITLES: [&str; 2] = ["Search", "Inspect"];

/// Sessions updated this recently are live: a dot in the row, and refreshed while open.
pub const LIVE_SECS: u64 = 120;

/// What [`State::requested`] tracks per session.
pub const PREVIEW: u8 = 0;
pub const CHILDREN: u8 = 1;
pub const PLAN: u8 = 2;
/// Restoring the session's transcript from sync, once an action is waiting on it.
pub const RESTORE: u8 = 3;
/// Reading what continuing the session in another harness would flatten, for the chooser.
pub const FLATTEN: u8 = 4;

/// How many rows a search asks for.
pub const SEARCH_LIMIT: usize = 500;

/// What the event loop should do after an input event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Continue,
    Redraw,
    /// Resume the session at this index now.
    Resume(usize),
    /// Put the session's resume command on the command line.
    ReturnCommand(usize),
    /// Copy the session's resume command, and stay open.
    Copy(usize),
    /// A line of the chooser: resume the session in its own harness (`None`), or continue it in
    /// this one; then do what the key asked.
    Pick(Option<HarnessKind>, Pending),
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

/// Inspect's list of the sessions grouped under the one inspected, once expanded (`c`): it has
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
    filters: Vec<FilterMode>,
    pub mode: FilterMode,
    pub widened: Option<Widened>,
    /// Widen workspace to global when it has no matches. Only for the default filter, and only
    /// until the user picks a mode with ctrl-r.
    auto_widen: bool,
    roots_only: bool,

    /// The generation of the newest search sent to the worker.
    pub issued: u64,
    /// The generation whose results are on screen.
    pub applied: u64,
    last_filter: Option<SessionFilter>,
    /// The generation of a refresh in flight, whose results keep the selection.
    refreshing: Option<u64>,

    pub previews: HashMap<HarnessSession, SessionPreview>,
    pub children: HashMap<HarnessSession, Vec<SessionRow>>,
    /// Inspect's children list, while it's expanded.
    pub children_view: Option<ChildrenView>,
    /// Whether the detail pane showed beside the list last time it was drawn (it says what is
    /// grouped under the selected session, so it wants the children too).
    pub pane_shown: bool,
    /// Details asked of the worker and not answered yet. The worker drops a request superseded by
    /// a newer one of its kind, so only the selected session's entries are kept (see
    /// [`Self::forget_unanswered`]).
    pub requested: HashSet<(HarnessSession, u8)>,
    /// A session named by id that can't be resumed, kept first while the query is still the id
    /// it was named by, so the picker opens on it and says why.
    pinned: Option<(SessionRow, String)>,
    /// Resume plans, fetched for the selected session only (planning may walk directories). A
    /// plan to restore the session from sync is replaced by the plain one once it is restored.
    pub plans: HashMap<HarnessSession, Result<Resume, NotResumable>>,
    /// Other hosts' names by host id, once the source has read them.
    host_names: HashMap<String, String>,
    /// An enter/tab/ctrl-y waiting for its session's plan.
    pub pending: Option<(HarnessSession, Pending)>,
    /// The "Resume in" chooser, while it's open.
    pub chooser: Option<Chooser>,
    /// Where the selected row was last drawn, for the chooser to open against.
    pub list_anchor: Option<ListAnchor>,
    /// What continuing each session elsewhere would flatten, once read (see
    /// [`Request::Flatten`](super::worker::Request::Flatten)); `Err` when it can't be read.
    pub flattened: HashMap<HarnessSession, Result<Flattened, String>>,
    /// A continuation being written, and what to do once it is.
    pub continuing: Option<(HarnessSession, HarnessKind, Pending)>,

    /// A one-line message in the status row (copied, can't resume, search failed).
    pub status: Option<(String, Meaning)>,
    pub original_input_empty: bool,
    pub accept: bool,
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
            filters: if sessions.filters.is_empty() {
                vec![FilterMode::Global]
            } else {
                sessions.filters.clone()
            },
            mode: FilterMode::Global,
            widened: None,
            auto_widen: false,
            roots_only: sessions.group_forks,
            issued: 0,
            applied: 0,
            last_filter: None,
            refreshing: None,
            previews: HashMap::new(),
            children: HashMap::new(),
            children_view: None,
            pane_shown: false,
            requested: HashSet::new(),
            pinned: None,
            host_names: HashMap::new(),
            plans: HashMap::new(),
            pending: None,
            chooser: None,
            list_anchor: None,
            flattened: HashMap::new(),
            continuing: None,
            status: None,
            original_input_empty: query.is_empty(),
            accept: false,
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

    /// The mode ctrl-r goes to next: the next available one in `[ai.sessions] filters`, wrapping
    /// around. `None` when there is no other.
    pub fn next_mode(&self) -> Option<FilterMode> {
        let len = self.filters.len();
        let at = self.filters.iter().position(|m| *m == self.mode).unwrap_or(len - 1);
        (1..=len)
            .map(|step| self.filters[(at + step) % len])
            .find(|mode| self.mode_available(*mode))
            .filter(|mode| *mode != self.mode)
    }

    /// ctrl-r: the next available mode in `[ai.sessions] filters`.
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

    /// The source filter for the current mode and query.
    pub fn filter(&self) -> SessionFilter {
        let q = self.parsed_query();
        let ctx = &self.context;
        let mut filter = SessionFilter {
            text: q.text,
            harness: q.harness,
            model: q.model,
            branch: q.branch,
            host_name: q.host,
            roots_only: self.roots_only,
            limit: SEARCH_LIMIT,
            ..SessionFilter::default()
        };
        match self.mode {
            FilterMode::Global => {}
            FilterMode::Host => filter.host = Some(ctx.host_id.clone()),
            FilterMode::Workspace => filter.workspace.clone_from(&ctx.git_root),
            FilterMode::Directory => filter.directory = Some(ctx.cwd.clone()),
            FilterMode::Branch => {
                filter.workspace.clone_from(&ctx.git_root);
                if filter.branch.is_none() {
                    filter.branch.clone_from(&ctx.branch);
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
        // A search may have listed its rows before the names were known.
        for row in &mut rows {
            self.label(row);
        }
        if let Some((row, query)) = &self.pinned {
            if self.input.as_str() == query {
                rows.retain(|r| r.handle != row.handle);
                rows.insert(0, row.clone());
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
        for handle in live {
            self.previews.remove(&handle);
            self.requested.remove(&(handle, PREVIEW));
        }
        Some(next)
    }

    /// Name other hosts' rows, which showed a short id until the names were read.
    pub fn apply_host_names(&mut self, names: HashMap<String, String>) {
        self.host_names = names;
        let rows = self.results.iter_mut().chain(self.children.values_mut().flatten());
        for row in rows.chain(self.pinned.as_mut().map(|(row, _)| row)) {
            Self::label_with(&self.host_names, &self.context.host_id, row);
        }
    }

    fn label(&self, row: &mut SessionRow) {
        Self::label_with(&self.host_names, &self.context.host_id, row);
    }

    /// This host keeps the name it has now; another takes the name it synced under, if known.
    fn label_with(names: &HashMap<String, String>, here: &str, row: &mut SessionRow) {
        if row.host_id != here
            && let Some(name) = names.get(&row.host_id)
        {
            row.hostname.clone_from(name);
        }
    }

    pub fn selected(&self) -> Option<&SessionRow> {
        self.results.get(self.list.selected)
    }

    /// Open on `row`, which the query named by id but which can't be resumed: it is selected, its
    /// plan is the reason (shown in the status row), and it stays first until the query changes.
    pub fn pin(&mut self, row: SessionRow, why: NotResumable) {
        self.status =
            Some((format!("can't resume {}: {why}", row.handle.session), Meaning::AlertError));
        self.plans.insert(row.handle.clone(), Err(why));
        self.results = vec![row.clone()];
        self.list.selected = 0;
        self.pinned = Some((row, self.input.as_str().to_owned()));
    }

    /// Forget unanswered requests for sessions other than the selected one: the worker drops
    /// those once a newer request of the same kind arrives, so they must be asked for again.
    pub fn forget_unanswered(&mut self) {
        let Some(selected) = self.selected().map(|r| r.handle.clone()) else {
            self.requested.clear();
            return;
        };
        self.requested.retain(|(handle, _)| *handle == selected);
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

    fn handle_mouse_input(&mut self, settings: &Settings, event: MouseEvent) -> InputAction {
        // The chooser is for the session it opened on.
        if self.chooser.is_some() {
            return InputAction::Continue;
        }
        let action = match event.kind {
            MouseEventKind::ScrollDown => Action::SelectNext,
            MouseEventKind::ScrollUp => Action::SelectPrevious,
            _ => return InputAction::Continue,
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

    pub fn eval_context(&self) -> EvalContext {
        EvalContext {
            cursor_position: self.input.position(),
            input_width: UnicodeWidthStr::width(self.input.as_str()),
            input_byte_len: self.input.as_str().len(),
            selected_index: self.list.selected,
            results_len: self.results.len(),
            original_input_empty: self.original_input_empty,
            has_context: false,
        }
    }

    #[must_use]
    pub fn handle_key_input(&mut self, settings: &Settings, input: &KeyEvent) -> InputAction {
        if input.kind == KeyEventKind::Release {
            return InputAction::Continue;
        }
        let Some(single) = SingleKey::from_event(input) else {
            return InputAction::Continue;
        };
        if self.chooser.is_some() {
            return self.chooser_key(&single);
        }
        let ctx = self.eval_context();
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
                .resolve(&seq, &ctx)
                .or_else(|| keymap.resolve(&KeyInput::Single(single.clone()), &ctx));
            (action, None)
        } else if let KeyCodeValue::Char(c) = single.code
            && !single.ctrl
            && !single.alt
            && keymap.has_sequence_starting_with(&single)
        {
            (Some(Action::Noop), Some(c))
        } else {
            (keymap.resolve(&KeyInput::Single(single.clone()), &ctx), None)
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
        let known = self.children.get(&row.handle).map(Vec::len);
        if row.children == 0 && known.unwrap_or(0) == 0 {
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
        let selected = self.list.selected;

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

            Action::Resume => {
                self.accept = true;
                return InputAction::Resume(selected);
            }
            Action::ReturnCommand => return InputAction::ReturnCommand(selected),
            Action::Copy => return InputAction::Copy(selected),
            Action::ReturnOriginal => return InputAction::ReturnOriginal,
            Action::Exit if self.tab_index == 1 => {
                self.tab_index = 0;
                return InputAction::Redraw;
            }
            Action::Exit => return InputAction::Exit,
            Action::Redraw => return InputAction::Redraw,
            Action::CycleFilterMode => self.cycle_filter_mode(),
            Action::CycleHarness => {
                self.input = Cursor::from(query::cycle_harness(self.input.as_str()));
                self.input.end();
            }
            Action::ToggleTab => {
                self.tab_index = (self.tab_index + 1) % TAB_TITLES.len();
                self.children_view = None;
                return InputAction::Redraw;
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
        assert_eq!(state.filter().workspace, Some(PathBuf::from(fake::REPO)));
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
        assert_eq!(state.filter().workspace, None);
    }

    #[rstest]
    fn empty_workspace_widens_to_global_once() {
        let mut state = state_in(fake::context());
        let (generation, mode, _) = state.next_search().unwrap();
        assert!(state.apply_results(generation, mode, Vec::new()), "should re-search");
        assert_eq!(state.mode, FilterMode::Global);
        assert_eq!(state.widened, Some(Widened::NoMatches));

        let (generation, mode, filter) = state.next_search().unwrap();
        assert_eq!(filter.workspace, None);
        state.apply_results(generation, mode, rows(2));
        assert_eq!(state.results.len(), 2);

        // Once widened, an empty global result is just empty.
        state.input = Cursor::from("nothing matches this".to_owned());
        let (generation, mode, _) = state.next_search().unwrap();
        state.apply_results(generation, mode, Vec::new());
        assert_eq!(state.mode, FilterMode::Global);
        assert!(state.results.is_empty());
    }

    /// Rows listed before the host names were read show short ids; the names relabel them, and
    /// rows of searches answered before the names are relabelled as they arrive.
    #[rstest]
    fn host_names_relabel_other_hosts_rows() {
        let mut state = state_in(fake::context());
        let remote = |id: &str| {
            let mut row = fake::row(HarnessKind::ClaudeCode, id, "t");
            row.host_id = "0190bbbb00007000".to_owned();
            row.hostname = "0190bbbb".to_owned();
            row
        };
        let here = fake::row(HarnessKind::ClaudeCode, "here", "t");
        let (generation, mode, _) = state.next_search().unwrap();
        state.apply_results(generation, mode, vec![remote("a"), here.clone()]);
        state.children.insert(here.handle.clone(), vec![remote("child")]);

        let names = HashMap::from([
            ("0190bbbb00007000".to_owned(), "buildbox".to_owned()),
            (fake::THIS_HOST_ID.to_owned(), "old-name".to_owned()),
        ]);
        state.apply_host_names(names);
        let hostnames =
            |rows: &[SessionRow]| rows.iter().map(|r| r.hostname.clone()).collect::<Vec<_>>();
        // This host keeps the name it has now.
        assert_eq!(hostnames(&state.results), ["buildbox", fake::THIS_HOSTNAME]);
        assert_eq!(hostnames(&state.children[&here.handle]), ["buildbox"]);

        state.input = Cursor::from("t".to_owned());
        let (generation, mode, _) = state.next_search().unwrap();
        state.apply_results(generation, mode, vec![remote("b")]);
        assert_eq!(hostnames(&state.results), ["buildbox"]);
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
        assert_eq!(state.filter().host.as_deref(), Some(fake::THIS_HOST_ID));
        state.mode = FilterMode::Directory;
        assert_eq!(state.filter().directory, Some(PathBuf::from(fake::REPO)));
        state.mode = FilterMode::Branch;
        assert_eq!(state.filter().branch.as_deref(), Some("ai-resume"));
        state.input = Cursor::from("b:main flaky h:codex".to_owned());
        let filter = state.filter();
        assert_eq!(filter.branch.as_deref(), Some("main"));
        assert_eq!(filter.text, "flaky");
        assert_eq!(filter.harness, Some(HarnessKind::Codex));
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
        assert!(!state.previews.contains_key(&rs[2].handle), "the live preview reloads");
        assert!(state.previews.contains_key(&rs[0].handle));
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
    fn alt_h_rewrites_the_input() {
        let mut state = state_in(fake::context());
        let s = settings();
        state.input = Cursor::from("flaky".to_owned());
        let _ = state.handle_input(&s, &key(KeyCode::Char('h'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "flaky h:claude");
        let _ = state.handle_input(&s, &key(KeyCode::Char('h'), KeyModifiers::ALT));
        assert_eq!(state.input.as_str(), "flaky h:codex");
    }

    #[rstest]
    fn keys_map_to_outcomes() {
        let mut s = settings();
        s.enter_accept = true;
        let mut state = State::new(&s, fake::context(), "");
        state.results = rows(3);
        state.list.selected = 1;

        let press = |state: &mut State, code, m| state.handle_input(&s, &key(code, m));
        assert_eq!(press(&mut state, KeyCode::Enter, KeyModifiers::NONE), InputAction::Resume(1));
        assert!(state.accept);
        assert_eq!(
            press(&mut state, KeyCode::Tab, KeyModifiers::NONE),
            InputAction::ReturnCommand(1)
        );
        assert_eq!(
            press(&mut state, KeyCode::Char('y'), KeyModifiers::CONTROL),
            InputAction::Copy(1)
        );
        assert_eq!(
            press(&mut state, KeyCode::Char('c'), KeyModifiers::CONTROL),
            InputAction::ReturnOriginal
        );
        assert_eq!(
            press(&mut state, KeyCode::Char('g'), KeyModifiers::CONTROL),
            InputAction::ReturnOriginal
        );
        assert_eq!(press(&mut state, KeyCode::Esc, KeyModifiers::NONE), InputAction::Exit);

        // ctrl-o opens Inspect; esc there goes back instead of exiting.
        assert_eq!(
            press(&mut state, KeyCode::Char('o'), KeyModifiers::CONTROL),
            InputAction::Redraw
        );
        assert_eq!(state.tab_index, 1);
        assert_eq!(press(&mut state, KeyCode::Esc, KeyModifiers::NONE), InputAction::Redraw);
        assert_eq!(state.tab_index, 0);
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
