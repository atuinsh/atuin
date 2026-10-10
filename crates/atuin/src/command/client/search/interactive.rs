#[cfg(unix)]
use std::io::Read as _;
use std::io::{IsTerminal, Write, stdout};
use std::time::{Duration, Instant};

use atuin_client::database::{Context, Sqlite, current_context};
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{History, HistoryId};
use atuin_client::settings::{
    CursorStyle, ExitMode, FilterMode, KeymapMode, PreviewStrategy, RequestedSearchMode,
    SearchMode, Settings, UiColumn,
};
use atuin_common::shell::Shell;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use easy_cast::Conv;
use eyre::Result;
use futures_util::FutureExt;
use ratatui::backend::{CrosstermBackend, FromCrossterm};
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyEvent, MouseEvent};
#[cfg(not(target_os = "windows"))]
use ratatui::crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, queue, terminal};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::prelude::*;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Tabs};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use semver::Version;
use time::{OffsetDateTime, UtcOffset};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
#[cfg(windows)]
use windows_sys::Win32::System::Console::{GetConsoleOutputCP, SetConsoleOutputCP};

use super::cursor::Cursor;
use super::engines::{AnySearchEngine, SearchEngine, SearchState};
use super::history_list::{HistoryList, ListState};
use super::inspector::Stats as InspectorStats;
use super::inspector::bindings::Bindings;
use super::inspector::browser::{Browser, View as InspectorView};
use crate::VERSION;
use crate::command::client::search::engines;
use crate::command::client::search::history_list::HistoryHighlighter;
use crate::command::client::search::keybindings::KeymapSet;
use crate::command::client::theme::{Meaning, Theme};

const TAB_TITLES: [&str; 2] = ["Search", "Inspect"];

/// Tells the shell integration to execute the returned line instead of inserting it.
const ACCEPT_PREFIX: &str = "__atuin_accept__:";

#[derive(Debug, PartialEq, Eq)]
pub enum InputAction {
    Accept(usize),
    AcceptInspecting,
    Copy(usize),
    Delete(usize),
    DeleteInspecting,
    DeleteAllMatching(usize),
    ReturnOriginal,
    ReturnQuery,
    Continue,
    Redraw,
    SwitchContext(Option<usize>),
}

#[derive(Default)]
pub struct InspectingState {
    current: Option<HistoryId>,
    next: Option<HistoryId>,
    previous: Option<HistoryId>,
    browser: Browser,
    bindings: Bindings,
}

impl InspectingState {
    pub fn move_to_previous(&mut self) {
        if self.browser.view == InspectorView::Output {
            self.browser.scroll_output(false, 1);
        } else if let Some(previous) = self.previous {
            self.current = Some(previous);
        }
    }

    pub fn move_to_next(&mut self) {
        if self.browser.view == InspectorView::Output {
            self.browser.scroll_output(true, 1);
        } else if let Some(next) = self.next {
            self.current = Some(next);
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub fn to_compactness(f: &Frame, settings: &Settings) -> Compactness {
    if match settings.style {
        atuin_client::settings::Style::Auto => f.area().height < 14,
        atuin_client::settings::Style::Compact => true,
        atuin_client::settings::Style::Full => false,
    } {
        if settings.auto_hide_height != 0 && f.area().height <= settings.auto_hide_height {
            Compactness::Ultracompact
        } else {
            Compactness::Compact
        }
    } else {
        Compactness::Full
    }
}

struct SearchModeState {
    mode: SearchMode,
    pub daemon_failed: bool,
}

impl SearchModeState {
    pub fn new(settings: &Settings) -> Self {
        Self {
            mode: settings.active_search_mode(),
            daemon_failed: !cfg!(feature = "daemon"),
        }
    }

    /// Return the current search mode.
    ///
    /// If [`Self::daemon_failed`] is true and the requested mode is [`SearchMode::DaemonFuzzy`],
    /// this method will return [`SearchMode::Fuzzy`] instead.
    pub fn mode(&self) -> SearchMode {
        if self.is_failed_daemon_fuzzy() {
            SearchMode::Fuzzy
        } else {
            self.mode
        }
    }

    /// Return the raw mode, without correcting for unavailable modes.
    pub fn raw_mode(&self) -> SearchMode {
        self.mode
    }

    pub fn advance_to_next_mode(&mut self, settings: &Settings) {
        self.mode = self.mode.next(settings);
    }

    pub fn is_failed_daemon_fuzzy(&self) -> bool {
        self.mode == SearchMode::DaemonFuzzy && self.daemon_failed
    }
}

#[allow(clippy::struct_field_names)]
#[allow(clippy::struct_excessive_bools)]
pub struct State {
    /// Total history count; None until the background count query finishes.
    history_count: Option<i64>,
    update_needed: Option<Version>,
    results_state: ListState,
    switched_search_mode: bool,
    search_mode_state: SearchModeState,
    results_len: usize,
    accept: bool,
    /// Return a command changing to the selected entry's directory instead of its command.
    cd: bool,
    keymap_mode: KeymapMode,
    prefix: bool,
    current_cursor: Option<CursorStyle>,
    tab_index: usize,
    pending_vim_key: Option<char>,
    /// When `pending_vim_key` was set, for `keymap_sequence_timeout_ms`.
    pending_vim_key_since: Option<Instant>,
    /// A key that arrived while a sequence was pending and has to wait for the
    /// event loop, because the pending key's own action did (a redraw, say).
    queued_key: Option<super::keybindings::key::SingleKey>,
    original_input_empty: bool,

    pub inspecting_state: InspectingState,

    keymaps: KeymapSet,
    search: SearchState,
    engine: AnySearchEngine,
    now: Box<dyn Fn() -> OffsetDateTime + Send>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Compactness {
    Ultracompact,
    Compact,
    Full,
}

#[derive(Clone, Copy)]
struct StyleState {
    compactness: Compactness,
    invert: bool,
    inner_width: usize,
}

impl State {
    fn search_mode(&self) -> SearchMode {
        self.search_mode_state.mode()
    }

    async fn query_results(
        &mut self,
        db: &mut Sqlite,
        settings: &Settings,
    ) -> Result<Vec<History>> {
        #[cfg(feature = "daemon")]
        use atuin_daemon::client::{DaemonClientErrorKind, classify_error};

        let results = match self.engine.query(&self.search, db).await {
            Ok(results) => results,
            #[cfg(feature = "daemon")]
            Err(error)
                if self.search_mode() == SearchMode::DaemonFuzzy
                    && classify_error(&error) != DaemonClientErrorKind::NonGrpc =>
            {
                tracing::warn!("daemon-fuzzy search failed: {error:#}");
                self.search_mode_state.daemon_failed = true;
                self.engine = engines::engine(self.search_mode(), settings);
                self.engine.query(&self.search, db).await?
            }
            Err(error) => return Err(error),
        };

        // Search results are deduplicated; the inspected occurrence need not be among them.
        // Context/filter changes while inspecting must not discard that independent selection.
        if self.tab_index == 0 {
            self.inspecting_state.reset();
        }
        self.results_state.select(0);
        self.results_len = results.len();

        if settings.smart_sort {
            Ok(atuin_history::sort::sort(self.search.input.as_str(), results))
        } else {
            Ok(results)
        }
    }

    #[must_use]
    fn handle_input(&mut self, settings: &Settings, input: &Event) -> InputAction {
        match input {
            Event::Key(k) => self.handle_key_input(settings, k),
            Event::Mouse(m) => self.handle_mouse_input(*m, settings),
            Event::Paste(d) => self.handle_paste_input(d),
            _ => InputAction::Continue,
        }
    }

    fn handle_mouse_input(&mut self, input: MouseEvent, settings: &Settings) -> InputAction {
        use super::keybindings::Action;
        let action = match input.kind {
            event::MouseEventKind::ScrollDown => Action::SelectNext,
            event::MouseEventKind::ScrollUp => Action::SelectPrevious,
            _ => return InputAction::Continue,
        };
        self.execute_action(&action, settings)
    }

    fn handle_paste_input(&mut self, input: &str) -> InputAction {
        if self.tab_index == 1 {
            return InputAction::Continue;
        }
        for i in input.chars() {
            self.search.input.insert(i);
        }
        InputAction::Continue
    }

    fn cast_cursor_style(style: CursorStyle) -> SetCursorStyle {
        match style {
            CursorStyle::DefaultUserShape => SetCursorStyle::DefaultUserShape,
            CursorStyle::BlinkingBlock => SetCursorStyle::BlinkingBlock,
            CursorStyle::SteadyBlock => SetCursorStyle::SteadyBlock,
            CursorStyle::BlinkingUnderScore => SetCursorStyle::BlinkingUnderScore,
            CursorStyle::SteadyUnderScore => SetCursorStyle::SteadyUnderScore,
            CursorStyle::BlinkingBar => SetCursorStyle::BlinkingBar,
            CursorStyle::SteadyBar => SetCursorStyle::SteadyBar,
        }
    }

    fn set_keymap_cursor(&mut self, settings: &Settings, keymap_name: &str) {
        let cursor_style = if keymap_name == "__clear__" {
            None
        } else {
            settings.keymap_cursor.get(keymap_name).copied()
        }
        .or_else(|| self.current_cursor.map(|_| CursorStyle::DefaultUserShape));

        if cursor_style != self.current_cursor
            && let Some(style) = cursor_style
        {
            self.current_cursor = cursor_style;
            let _ = execute!(stdout(), Self::cast_cursor_style(style));
        }
    }

    pub fn initialize_keymap_cursor(&mut self, settings: &Settings) {
        match self.keymap_mode {
            KeymapMode::Emacs => self.set_keymap_cursor(settings, "emacs"),
            KeymapMode::VimNormal => self.set_keymap_cursor(settings, "vim_normal"),
            KeymapMode::VimInsert => self.set_keymap_cursor(settings, "vim_insert"),
            KeymapMode::Auto => {}
        }
    }

    pub fn finalize_keymap_cursor(&mut self, settings: &Settings) {
        match settings.keymap_mode_shell {
            KeymapMode::Emacs => self.set_keymap_cursor(settings, "emacs"),
            KeymapMode::VimNormal => self.set_keymap_cursor(settings, "vim_normal"),
            KeymapMode::VimInsert => self.set_keymap_cursor(settings, "vim_insert"),
            KeymapMode::Auto => self.set_keymap_cursor(settings, "__clear__"),
        }
    }

    fn handle_key_exit(settings: &Settings) -> InputAction {
        match settings.exit_mode {
            ExitMode::ReturnOriginal => InputAction::ReturnOriginal,
            ExitMode::ReturnQuery => InputAction::ReturnQuery,
        }
    }

    /// Select the keymap for the current mode (ignoring prefix).
    fn mode_keymap(&self) -> &super::keybindings::Keymap {
        if self.tab_index == 1 {
            &self.keymaps.inspector
        } else {
            match self.keymap_mode {
                KeymapMode::Emacs | KeymapMode::Auto => &self.keymaps.emacs,
                KeymapMode::VimNormal => &self.keymaps.vim_normal,
                KeymapMode::VimInsert => &self.keymaps.vim_insert,
            }
        }
    }

    /// Whether the current mode supports character insertion on unmatched keys.
    /// The inspector tab has no text input, so unmatched keys are dropped there
    /// rather than leaking into the (hidden) search input.
    fn is_insert_mode(&self) -> bool {
        self.tab_index == 0
            && matches!(
                self.keymap_mode,
                KeymapMode::Emacs | KeymapMode::Auto | KeymapMode::VimInsert
            )
    }

    fn eval_context(&self) -> super::keybindings::EvalContext {
        let (selected_index, results_len) = if self.tab_index == 1 {
            if self.inspecting_state.current.is_some() {
                self.inspecting_state.browser.list_position()
            } else {
                (0, 0)
            }
        } else {
            (self.results_state.selected(), self.results_len)
        };
        super::keybindings::EvalContext {
            cursor_position: self.search.input.position(),
            input_width: UnicodeWidthStr::width(self.search.input.as_str()),
            input_byte_len: self.search.input.as_str().len(),
            selected_index,
            results_len,
            original_input_empty: self.original_input_empty,
            has_context: self.search.custom_context.is_some(),
        }
    }

    #[must_use]
    fn handle_key_input(&mut self, settings: &Settings, input: &KeyEvent) -> InputAction {
        use super::keybindings::key::SingleKey;

        // Skip release events
        if input.kind == event::KeyEventKind::Release {
            return InputAction::Continue;
        }

        // Reset switched_search_mode at start of each key event
        self.switched_search_mode = false;

        // Convert KeyEvent to SingleKey
        let Some(single) = SingleKey::from_event(input) else {
            return InputAction::Continue;
        };

        self.handle_key(single, settings)
    }

    /// Resolve and run one key: complete or start a multi-key sequence, use the
    /// prefix keymap in prefix mode, or handle the key on its own.
    #[must_use]
    fn handle_key(
        &mut self,
        single: super::keybindings::key::SingleKey,
        settings: &Settings,
    ) -> InputAction {
        use super::keybindings::Action;
        use super::keybindings::key::{KeyCodeValue, KeyInput};

        if let Some(pending_char) = self.take_pending_key() {
            // We have a pending key from a previous press (e.g., first 'g' of 'gg')
            let pending_single = Self::plain_char_key(pending_char);
            let seq = KeyInput::Sequence(vec![pending_single.clone(), single.clone()]);
            let ctx = self.eval_context();
            if let Some(action) = self.mode_keymap().resolve(&seq, &ctx) {
                return self.execute_action(&action, settings);
            }

            // Not a sequence: handle the pending key on its own, then this key from
            // the start, so a prefix mode or sequence the pending key began applies
            // to it. If the pending key's action has to go back to the event loop
            // first (a redraw, say), this key is queued and handled next.
            let result = self.handle_single_key(&pending_single, settings);
            if !matches!(result, InputAction::Continue) {
                self.queued_key = Some(single);
                return result;
            }
            return self.handle_key(single, settings);
        }

        // If in prefix mode, try prefix keymap first (single keys only)
        if self.prefix {
            let ki = KeyInput::Single(single.clone());
            let ctx = self.eval_context();
            if let Some(action) = self.keymaps.prefix.resolve(&ki, &ctx) {
                // Reset prefix (before execute, so EnterPrefixMode can re-set it)
                self.prefix = false;
                return self.execute_action(&action, settings);
            }
        }
        self.prefix = false;

        if self.mode_keymap().has_sequence_starting_with(&single)
            && matches!(single.code, KeyCodeValue::Char(_))
            && !single.ctrl
            && !single.alt
        {
            // This key starts a multi-key sequence; wait for next key
            let KeyCodeValue::Char(c) = single.code else {
                unreachable!()
            };
            self.pending_vim_key = Some(c);
            self.pending_vim_key_since = Some(Instant::now());
            return self.execute_action(&Action::Noop, settings);
        }

        self.handle_single_key(&single, settings)
    }

    fn take_pending_key(&mut self) -> Option<char> {
        self.pending_vim_key_since = None;
        self.pending_vim_key.take()
    }

    fn has_queued_key(&self) -> bool {
        self.queued_key.is_some()
    }

    /// Handle the key queued behind a pending key's action, if any.
    #[must_use]
    fn handle_queued_key(&mut self, settings: &Settings) -> InputAction {
        self.switched_search_mode = false;
        self.queued_key
            .take()
            .map_or(InputAction::Continue, |single| self.handle_key(single, settings))
    }

    fn plain_char_key(c: char) -> super::keybindings::key::SingleKey {
        super::keybindings::key::SingleKey {
            code: super::keybindings::key::KeyCodeValue::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
        }
    }

    /// Handle one key on its own, ignoring multi-key sequences: run its action in
    /// the current mode, or, if it has none, insert it in insert-capable modes.
    #[must_use]
    fn handle_single_key(
        &mut self,
        single: &super::keybindings::key::SingleKey,
        settings: &Settings,
    ) -> InputAction {
        use super::keybindings::key::{KeyCodeValue, KeyInput};

        let ctx = self.eval_context();
        if let Some(action) = self.mode_keymap().resolve(&KeyInput::Single(single.clone()), &ctx) {
            return self.execute_action(&action, settings);
        }

        // No action matched. In insert-capable modes, insert the character.
        if self.is_insert_mode() && !single.ctrl && !single.alt {
            match single.code {
                KeyCodeValue::Char(c) => {
                    self.search.input.insert(c);
                }
                KeyCodeValue::Space => {
                    self.search.input.insert(' ');
                }
                _ => {}
            }
        }
        InputAction::Continue
    }

    /// How long to wait for the rest of a pending multi-key sequence before
    /// handling the pending key on its own. Only insert-capable modes time out,
    /// where the pending key is usually text being typed; vim-normal waits, so
    /// commands like `g g` are not timed. `None` when nothing should time out.
    /// The time is counted from the key press, so it is not restarted when the
    /// event loop wakes for other work.
    fn pending_key_timeout(&self, settings: &Settings) -> Option<Duration> {
        if self.pending_vim_key.is_none()
            || !self.is_insert_mode()
            || settings.keymap_sequence_timeout_ms == 0
        {
            return None;
        }
        let timeout = Duration::from_millis(settings.keymap_sequence_timeout_ms);
        let waited = self.pending_vim_key_since.map_or(Duration::ZERO, |t| t.elapsed());
        Some(timeout.saturating_sub(waited))
    }

    /// The rest of a sequence did not arrive in time: handle the pending key on
    /// its own.
    #[must_use]
    fn flush_pending_key(&mut self, settings: &Settings) -> InputAction {
        self.take_pending_key().map_or(InputAction::Continue, |c| {
            self.handle_single_key(&Self::plain_char_key(c), settings)
        })
    }

    fn scroll_down(&mut self, scroll_len: usize) {
        let i = self.results_state.selected().saturating_sub(scroll_len);
        self.inspecting_state.reset();
        self.results_state.select(i);
    }

    fn scroll_up(&mut self, scroll_len: usize) {
        let i = self.results_state.selected() + scroll_len;
        self.results_state.select(i.min(self.results_len.saturating_sub(1)));
        self.inspecting_state.reset();
    }

    /// The inspected entry in the inspector tab, otherwise the selected one.
    fn accept_selection(&self) -> InputAction {
        if self.tab_index == 1 {
            InputAction::AcceptInspecting
        } else {
            InputAction::Accept(self.results_state.selected())
        }
    }

    /// Execute a resolved action, performing all side effects and returning the
    /// appropriate `InputAction` for the event loop.
    ///
    /// This is the "do it" half of the resolve+execute pipeline. The resolver
    /// decides *what* to do (which `Action`), and this function carries it out.
    ///
    /// Invert handling: scroll actions (`SelectNext`, `ScrollPageDown`, etc.) account
    /// for `settings.invert` so that keybindings are always in "visual" terms —
    /// users never need to think about invert in their keybinding config.
    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub(crate) fn execute_action(
        &mut self,
        action: &super::keybindings::Action,
        settings: &Settings,
    ) -> InputAction {
        use crate::command::client::search::keybindings::Action;

        if self.tab_index == 1 {
            match action {
                Action::Exit => {
                    if self.inspecting_state.browser.view == InspectorView::Output {
                        self.inspecting_state.browser.back_from_output();
                    } else {
                        self.tab_index = 0;
                    }
                    return InputAction::Redraw;
                }
                Action::SelectPrevious | Action::SelectNext => {
                    if *action == Action::SelectNext {
                        self.inspecting_state.move_to_next();
                    } else {
                        self.inspecting_state.move_to_previous();
                    }
                    return InputAction::Redraw;
                }
                Action::ScrollPageUp
                | Action::ScrollPageDown
                | Action::ScrollHalfPageUp
                | Action::ScrollHalfPageDown => {
                    let next =
                        matches!(action, Action::ScrollPageDown | Action::ScrollHalfPageDown);
                    let half =
                        matches!(action, Action::ScrollHalfPageUp | Action::ScrollHalfPageDown);
                    if self.inspecting_state.browser.view == InspectorView::Output {
                        self.inspecting_state.browser.scroll_output_page(next, half);
                    } else if next {
                        self.inspecting_state.move_to_next();
                    } else {
                        self.inspecting_state.move_to_previous();
                    }
                    return InputAction::Redraw;
                }
                Action::ScrollToScreenTop
                | Action::ScrollToScreenMiddle
                | Action::ScrollToScreenBottom => {
                    // Screen-position jumps are search-only; don't move the hidden search list.
                    return InputAction::Continue;
                }
                Action::ScrollToTop | Action::ScrollToBottom => {
                    if self.inspecting_state.browser.view == InspectorView::Output {
                        self.inspecting_state
                            .browser
                            .scroll_output_edge(matches!(action, Action::ScrollToBottom));
                    }
                    return InputAction::Redraw;
                }
                _ => {}
            }
        }

        match action {
            // -- Cursor movement --
            Action::CursorLeft => {
                self.search.input.left();
                InputAction::Continue
            }
            Action::CursorRight => {
                self.search.input.right();
                InputAction::Continue
            }
            Action::CursorWordLeft => {
                self.search.input.prev_word(&settings.word_chars, settings.word_jump_mode);
                InputAction::Continue
            }
            Action::CursorWordRight => {
                self.search.input.next_word(&settings.word_chars, settings.word_jump_mode);
                InputAction::Continue
            }
            Action::CursorWordEnd => {
                self.search.input.word_end(&settings.word_chars);
                InputAction::Continue
            }
            Action::CursorStart => {
                self.search.input.start();
                InputAction::Continue
            }
            Action::CursorEnd => {
                self.search.input.end();
                InputAction::Continue
            }

            // -- Editing --
            Action::DeleteCharBefore => {
                self.search.input.back();
                InputAction::Continue
            }
            Action::DeleteCharAfter => {
                self.search.input.remove();
                InputAction::Continue
            }
            Action::DeleteWordBefore => {
                self.search.input.remove_prev_word(&settings.word_chars, settings.word_jump_mode);
                InputAction::Continue
            }
            Action::DeleteWordAfter => {
                self.search.input.remove_next_word(&settings.word_chars, settings.word_jump_mode);
                InputAction::Continue
            }
            Action::DeleteToWordBoundary => {
                // ctrl-w: remove trailing whitespace, then delete to word boundary
                while matches!(self.search.input.back(), Some(c) if c.is_whitespace()) {}
                while self.search.input.left() {
                    if self.search.input.char().unwrap().is_whitespace() {
                        self.search.input.right();
                        break;
                    }
                    self.search.input.remove();
                }
                InputAction::Continue
            }
            Action::ClearLine => {
                self.search.input.clear();
                InputAction::Continue
            }
            Action::ClearToStart => {
                self.search.input.clear_to_start();
                InputAction::Continue
            }
            Action::ClearToEnd => {
                self.search.input.clear_to_end();
                InputAction::Continue
            }

            // -- List navigation (invert-aware) --
            Action::SelectNext => {
                if settings.invert {
                    self.scroll_up(1);
                } else {
                    self.scroll_down(1);
                }
                InputAction::Continue
            }
            Action::SelectPrevious => {
                if settings.invert {
                    self.scroll_down(1);
                } else {
                    self.scroll_up(1);
                }
                InputAction::Continue
            }
            // -- Page/half-page scroll (invert-aware) --
            Action::ScrollHalfPageUp => {
                let scroll_len =
                    self.results_state.max_entries().saturating_sub(settings.scroll_context_lines)
                        / 2;
                if settings.invert {
                    self.scroll_down(scroll_len);
                } else {
                    self.scroll_up(scroll_len);
                }
                InputAction::Continue
            }
            Action::ScrollHalfPageDown => {
                let scroll_len =
                    self.results_state.max_entries().saturating_sub(settings.scroll_context_lines)
                        / 2;
                if settings.invert {
                    self.scroll_up(scroll_len);
                } else {
                    self.scroll_down(scroll_len);
                }
                InputAction::Continue
            }
            Action::ScrollPageUp => {
                let scroll_len =
                    self.results_state.max_entries().saturating_sub(settings.scroll_context_lines);
                if settings.invert {
                    self.scroll_down(scroll_len);
                } else {
                    self.scroll_up(scroll_len);
                }
                InputAction::Continue
            }
            Action::ScrollPageDown => {
                let scroll_len =
                    self.results_state.max_entries().saturating_sub(settings.scroll_context_lines);
                if settings.invert {
                    self.scroll_up(scroll_len);
                } else {
                    self.scroll_down(scroll_len);
                }
                InputAction::Continue
            }

            // -- Absolute jumps (invert-aware) --
            Action::ScrollToTop => {
                // Visual top of history
                if settings.invert {
                    self.results_state.select(0);
                } else {
                    let last_idx = self.results_len.saturating_sub(1);
                    self.results_state.select(last_idx);
                }
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToBottom => {
                // Visual bottom of history
                if settings.invert {
                    let last_idx = self.results_len.saturating_sub(1);
                    self.results_state.select(last_idx);
                } else {
                    self.results_state.select(0);
                }
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToScreenTop => {
                // H — jump to top of visible screen
                let top = self.results_state.offset();
                let visible = self.results_state.max_entries().min(self.results_len);
                let bottom = top + visible.saturating_sub(1);
                self.results_state.select(bottom.min(self.results_len.saturating_sub(1)));
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToScreenMiddle => {
                // M — jump to middle of visible screen
                let top = self.results_state.offset();
                let visible = self.results_state.max_entries().min(self.results_len);
                let middle = top + visible / 2;
                self.results_state.select(middle.min(self.results_len.saturating_sub(1)));
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToScreenBottom => {
                // L — jump to bottom of visible screen
                let top_visible = self.results_state.offset();
                self.results_state.select(top_visible);
                self.inspecting_state.reset();
                InputAction::Continue
            }

            // -- Commands --
            Action::Accept => {
                self.accept = true;
                self.accept_selection()
            }
            Action::AcceptNth(n) => {
                self.accept = true;
                InputAction::Accept(self.results_state.selected() + usize::conv(*n))
            }
            Action::ReturnSelection => self.accept_selection(),
            Action::ReturnSelectionNth(n) => {
                InputAction::Accept(self.results_state.selected() + usize::conv(*n))
            }
            Action::AcceptCd | Action::ReturnCd => {
                self.cd = true;
                self.accept = *action == Action::AcceptCd;
                self.accept_selection()
            }
            Action::Copy => InputAction::Copy(self.results_state.selected()),
            Action::Delete if self.tab_index == 1 => InputAction::DeleteInspecting,
            Action::Delete => InputAction::Delete(self.results_state.selected()),
            Action::DeleteAll => InputAction::DeleteAllMatching(self.results_state.selected()),
            Action::ReturnOriginal => InputAction::ReturnOriginal,
            Action::ReturnQuery => InputAction::ReturnQuery,
            Action::Exit => Self::handle_key_exit(settings),
            Action::Redraw => InputAction::Redraw,
            Action::CycleFilterMode => {
                self.search.rotate_filter_mode(settings, 1);
                InputAction::Continue
            }
            Action::CycleSearchMode => {
                self.switched_search_mode = true;
                self.search_mode_state.advance_to_next_mode(settings);
                self.engine = engines::engine(self.search_mode(), settings);
                InputAction::Continue
            }
            Action::SwitchContext => {
                InputAction::SwitchContext(Some(self.results_state.selected()))
            }
            Action::ClearContext => InputAction::SwitchContext(None),
            Action::ToggleTab => {
                self.tab_index = (self.tab_index + 1) % TAB_TITLES.len();
                InputAction::Redraw
            }

            // -- Mode changes --
            Action::VimEnterNormal => {
                self.set_keymap_cursor(settings, "vim_normal");
                self.keymap_mode = KeymapMode::VimNormal;
                InputAction::Continue
            }
            Action::VimEnterInsert => {
                self.set_keymap_cursor(settings, "vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAfter => {
                self.search.input.right();
                self.set_keymap_cursor(settings, "vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAtStart => {
                self.search.input.start();
                self.set_keymap_cursor(settings, "vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAtEnd => {
                self.search.input.end();
                self.set_keymap_cursor(settings, "vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimSearchInsert => {
                self.search.input.clear();
                self.set_keymap_cursor(settings, "vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimChangeToEnd => {
                self.search.input.clear_to_end();
                self.set_keymap_cursor(settings, "vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::EnterPrefixMode => {
                self.prefix = true;
                InputAction::Continue
            }

            // -- Inspector --
            Action::InspectPrevious => {
                self.inspecting_state.move_to_previous();
                InputAction::Redraw
            }
            Action::InspectNext => {
                self.inspecting_state.move_to_next();
                InputAction::Redraw
            }
            Action::InspectRuns
            | Action::InspectSession
            | Action::InspectStats
            | Action::InspectOutput => {
                let view = match action {
                    Action::InspectSession => InspectorView::Session,
                    Action::InspectStats => InspectorView::Stats,
                    Action::InspectOutput => InspectorView::Output,
                    _ => InspectorView::Runs,
                };
                self.inspecting_state.browser.select_view(view);
                InputAction::Redraw
            }

            // -- Special --
            Action::Noop => InputAction::Continue,
        }
    }

    #[allow(clippy::bool_to_int_with_if)]
    fn calc_preview_height(
        settings: &Settings,
        results: &[History],
        selected: usize,
        tab_index: usize,
        compactness: Compactness,
        border_size: u16,
        preview_width: u16,
    ) -> u16 {
        if settings.show_preview
            && settings.preview.strategy == PreviewStrategy::Auto
            && tab_index == 0
            && !results.is_empty()
        {
            let length_current_cmd = u16::conv(results[selected].command.width());
            // calculate the number of newlines in the command
            let num_newlines =
                u16::conv(results[selected].command.chars().filter(|&c| c == '\n').count());
            if num_newlines > 0 {
                std::cmp::min(
                    settings.max_preview_height,
                    results[selected]
                        .command
                        .split('\n')
                        .map(|line| {
                            (u16::conv(line.len()) + preview_width - 1 - border_size)
                                / (preview_width - border_size)
                        })
                        .sum(),
                ) + border_size * 2
            }
            // The '- 19' takes the characters before the command (duration and time) into account
            else if length_current_cmd > preview_width - 19 {
                std::cmp::min(
                    settings.max_preview_height,
                    (length_current_cmd + preview_width - 1 - border_size)
                        / (preview_width - border_size),
                ) + border_size * 2
            } else {
                1
            }
        } else if settings.show_preview
            && settings.preview.strategy == PreviewStrategy::Static
            && tab_index == 0
        {
            let longest_command =
                results.iter().max_by(|h1, h2| h1.command.len().cmp(&h2.command.len()));
            longest_command.map_or(0, |v| {
                std::cmp::min(
                    settings.max_preview_height,
                    v.command
                        .split('\n')
                        .map(|line| {
                            (u16::conv(line.len()) + preview_width - 1 - border_size)
                                / (preview_width - border_size)
                        })
                        .sum(),
                )
            }) + border_size * 2
        } else if settings.show_preview && settings.preview.strategy == PreviewStrategy::Fixed {
            settings.max_preview_height + border_size * 2
        } else if !matches!(compactness, Compactness::Full) || tab_index == 1 {
            0
        } else {
            1
        }
    }

    #[allow(clippy::bool_to_int_with_if)]
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        f: &mut Frame,
        results: &[History],
        stats: Option<&InspectorStats>,
        inspecting: Option<&History>,
        settings: &Settings,
        theme: &Theme,
        popup_mode: bool,
    ) {
        let area = f.area();
        if popup_mode {
            f.render_widget(Clear, area);
        }
        self.draw_inner(f, area, results, stats, inspecting, settings, theme);
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::bool_to_int_with_if)]
    fn draw_inner(
        &mut self,
        f: &mut Frame,
        area: Rect,
        results: &[History],
        stats: Option<&InspectorStats>,
        inspecting: Option<&History>,
        settings: &Settings,
        theme: &Theme,
    ) {
        let bindings = &self.inspecting_state.bindings;
        // Output is a focused reader, not another panel beneath the search chrome.
        if self.tab_index == 1
            && self.inspecting_state.browser.view == InspectorView::Output
            && inspecting.or_else(|| results.get(self.results_state.selected())).is_some()
        {
            self.inspecting_state.browser.draw(f, area, theme, bindings);
            return;
        }
        let compactness = to_compactness(f, settings);
        let invert = settings.invert;
        let border_size = match compactness {
            Compactness::Full => 1,
            _ => 0,
        };
        let preview_width = area.width.saturating_sub(2);
        let preview_height = Self::calc_preview_height(
            settings,
            results,
            self.results_state.selected(),
            self.tab_index,
            compactness,
            border_size,
            preview_width,
        );

        let show_help = settings.show_help && (compactness == Compactness::Full || area.height > 1);
        let warnings = self.build_warnings(settings, theme);
        let warning_height = u16::try_from(warnings.height()).unwrap_or(u16::MAX);

        // This is an OR, as it seems more likely for someone to wish to override
        // tabs unexpectedly being missed, than unexpectedly present.
        let show_tabs = settings.show_tabs && !matches!(compactness, Compactness::Ultracompact);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .horizontal_margin(1)
            .constraints::<&[Constraint]>(
                if invert {
                    [
                        Constraint::Length(1 + border_size), // input
                        Constraint::Min(1),                  // results list
                        Constraint::Length(preview_height),  // preview
                        Constraint::Length(if show_tabs {
                            1
                        } else {
                            0
                        }), // tabs
                        Constraint::Length(if show_help {
                            1
                        } else {
                            0
                        }), // header (sic)
                        Constraint::Length(warning_height),  // skim warning
                    ]
                } else {
                    match compactness {
                        Compactness::Ultracompact => [
                            Constraint::Length(if show_help {
                                1
                            } else {
                                0
                            }), // header
                            Constraint::Length(0),              // tabs
                            Constraint::Min(1),                 // results list
                            Constraint::Length(0),              // no input
                            Constraint::Length(0),              // no preview
                            Constraint::Length(warning_height), // skim warning
                        ],
                        _ => [
                            Constraint::Length(if show_help {
                                1
                            } else {
                                0
                            }), // header
                            Constraint::Length(if show_tabs {
                                1
                            } else {
                                0
                            }), // tabs
                            Constraint::Min(1),                  // results list
                            Constraint::Length(1 + border_size), // input
                            Constraint::Length(preview_height),  // preview
                            Constraint::Length(warning_height),  // skim warning
                        ],
                    }
                }
                .as_ref(),
            )
            .split(area);

        let input_chunk = if invert {
            chunks[0]
        } else {
            chunks[3]
        };
        let results_list_chunk = if invert {
            chunks[1]
        } else {
            chunks[2]
        };
        let preview_chunk = if invert {
            chunks[2]
        } else {
            chunks[4]
        };
        let tabs_chunk = if invert {
            chunks[3]
        } else {
            chunks[1]
        };
        let header_chunk = if invert {
            chunks[4]
        } else {
            chunks[0]
        };
        // Always last, so it is the bottom row whichever way the layout is stacked.
        let warning_chunk = chunks[5];

        // TODO: this should be split so that we have one interactive search container that is
        // EITHER a search box or an inspector. But I'm not doing that now, way too much atm.
        // also allocate less 🙈
        let titles: Vec<_> = TAB_TITLES.iter().copied().map(Line::from).collect();

        if show_tabs {
            let tabs = Tabs::new(titles)
                .block(Block::default().borders(Borders::NONE))
                .select(self.tab_index)
                .style(Style::default())
                .highlight_style(Style::from_crossterm(theme.as_style(Meaning::Important)));

            f.render_widget(tabs, tabs_chunk);
        }

        let style = StyleState {
            compactness,
            invert,
            inner_width: input_chunk.width.into(),
        };

        let header_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints::<&[Constraint]>(
                [Constraint::Ratio(1, 5), Constraint::Ratio(3, 5), Constraint::Ratio(1, 5)]
                    .as_ref(),
            )
            .split(header_chunk);

        let title = self.build_title(theme);
        f.render_widget(title, header_chunks[0]);

        let help = self.build_help(settings, theme);
        f.render_widget(help, header_chunks[1]);

        let stats_tab = self.build_stats(theme);
        f.render_widget(stats_tab, header_chunks[2]);

        if warning_height > 0 {
            f.render_widget(warnings, warning_chunk);
        }

        let indicator: String = match compactness {
            Compactness::Ultracompact => {
                if self.switched_search_mode {
                    format!(
                        "S{}>",
                        self.search_mode_state.raw_mode().as_str().chars().next().unwrap()
                    )
                } else if self.search.custom_context.is_some() {
                    format!("C{}>", self.search.filter_mode.as_str().chars().next().unwrap())
                } else {
                    format!("{}> ", self.search.filter_mode.as_str().chars().next().unwrap())
                }
            }
            _ => " > ".to_string(),
        };

        match self.tab_index {
            0 => {
                let history_highlighter = HistoryHighlighter {
                    engine: &self.engine,
                    search_input: self.search.input.as_str(),
                };
                let results_list = Self::build_results_list(
                    style,
                    results,
                    self.keymap_mode,
                    &self.now,
                    settings.timezone.0,
                    indicator.as_str(),
                    theme,
                    history_highlighter,
                    settings.show_numeric_shortcuts,
                    settings.ui.syntax_highlight,
                    &settings.ui.columns,
                );
                f.render_stateful_widget(results_list, results_list_chunk, &mut self.results_state);
            }

            1 => {
                if results.is_empty() && inspecting.is_none() {
                    let message = Paragraph::new("Nothing to inspect")
                        .block(
                            Block::new()
                                .title(Line::from(" Info ".to_string()))
                                .title_alignment(Alignment::Center)
                                .borders(Borders::ALL)
                                .padding(Padding::vertical(2)),
                        )
                        .alignment(Alignment::Center);
                    f.render_widget(message, results_list_chunk);
                } else {
                    let browser = &mut self.inspecting_state.browser;
                    let chunk = super::inspector::browser::draw_views(
                        f,
                        results_list_chunk,
                        browser.view,
                        theme,
                        bindings,
                    );
                    let chunk = browser.draw_command(f, chunk, theme);
                    if browser.view == InspectorView::Stats {
                        if let Some(stats) = stats {
                            super::inspector::draw(f, chunk, stats, theme);
                        }
                    } else {
                        browser.draw(f, chunk, theme, bindings);
                    }
                }

                let guide = super::inspector::browser::input_guide(
                    self.inspecting_state.browser.view,
                    input_chunk.width,
                    theme,
                    bindings,
                );
                f.render_widget(Paragraph::new(guide), input_chunk);

                return;
            }

            _ => {
                panic!("invalid tab index");
            }
        }

        if !matches!(compactness, Compactness::Ultracompact) {
            let preview_width = match compactness {
                Compactness::Full => preview_width - 2,
                _ => preview_width,
            };
            let preview = self.build_preview(
                results,
                compactness,
                preview_width,
                preview_chunk.width.into(),
                theme,
            );
            let prefix_width = settings
                .ui
                .columns
                .iter()
                .take_while(|col| !col.expand)
                .map(|col| col.width + 1)
                .sum::<u16>()
                + u16::conv(" > ".len());
            let min_prefix_width = u16::conv("[ SRCH: FULLTXT ] ".len());
            self.draw_preview(
                f,
                style,
                input_chunk,
                compactness,
                preview_chunk,
                preview,
                std::cmp::max(prefix_width, min_prefix_width),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_preview(
        &self,
        f: &mut Frame,
        style: StyleState,
        input_chunk: Rect,
        compactness: Compactness,
        preview_chunk: Rect,
        preview: Paragraph,
        prefix_width: u16,
    ) {
        let input = self.build_input(style, prefix_width);
        f.render_widget(input, input_chunk);

        f.render_widget(preview, preview_chunk);

        let extra_width = UnicodeWidthStr::width(self.search.input.substring());

        let cursor_offset = match compactness {
            Compactness::Full => 1,
            _ => 0,
        };
        f.set_cursor_position((
            // Put cursor past the end of the input text
            input_chunk.x + u16::conv(extra_width) + prefix_width + cursor_offset,
            input_chunk.y + cursor_offset,
        ));
    }

    fn build_title(&self, theme: &Theme) -> Paragraph<'_> {
        let title = if self.update_needed.is_some() {
            let error_style: Style = Style::from_crossterm(theme.get_error());
            Paragraph::new(Text::from(Span::styled(
                format!("Atuin v{VERSION} - UPDATE"),
                error_style.add_modifier(Modifier::BOLD),
            )))
        } else {
            let style: Style = Style::from_crossterm(theme.as_style(Meaning::Base));
            Paragraph::new(Text::from(Span::styled(
                format!("Atuin v{VERSION}"),
                style.add_modifier(Modifier::BOLD),
            )))
        };
        title.alignment(Alignment::Left)
    }

    #[allow(clippy::unused_self)]
    fn build_help(&self, settings: &Settings, theme: &Theme) -> Paragraph<'_> {
        match self.tab_index {
            // search
            0 => Paragraph::new(Text::from(Line::from(vec![
                Span::styled("<esc>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": exit"),
                Span::raw(", "),
                Span::styled("<tab>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": edit"),
                Span::raw(", "),
                Span::styled("<enter>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(if settings.enter_accept {
                    ": run"
                } else {
                    ": edit"
                }),
                Span::raw(", "),
                Span::styled("<ctrl-o>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": inspect"),
            ]))),

            // The inspector has its own contextual guide at the bottom.
            1 => Paragraph::default(),

            _ => unreachable!("invalid tab index"),
        }
        .style(Style::from_crossterm(theme.as_style(Meaning::Annotation)))
        .alignment(Alignment::Center)
    }

    fn build_warnings(&self, settings: &Settings, theme: &Theme) -> Text<'static> {
        let get_style = || {
            Style::from_crossterm(theme.as_style(Meaning::AlertWarn)).add_modifier(Modifier::BOLD)
        };

        if self.search_mode_state.is_failed_daemon_fuzzy() {
            let msg = if cfg!(feature = "daemon") {
                "Warning: daemon-fuzzy search failed; falling back to fuzzy"
            } else {
                "Warning: no daemon support; falling back to fuzzy search"
            };
            return Text::styled(msg, get_style());
        }

        if settings.requested_search_mode != RequestedSearchMode::Skim {
            return Text::default();
        }

        let style = get_style();
        let code_style = Style::from_crossterm(theme.as_style(Meaning::SyntaxCommand))
            .add_modifier(Modifier::BOLD);

        Text::from(vec![
            Span::styled("Warning: \"skim\" mode was removed; falling back to \"fuzzy\"", style)
                .into(),
            vec![
                Span::styled("Set ", style),
                Span::styled("search_mode = \"daemon-fuzzy\"", code_style),
                Span::styled(" for a similar experience", style),
            ]
            .into(),
        ])
        .left_aligned()
    }

    fn build_stats(&self, theme: &Theme) -> Paragraph<'_> {
        Paragraph::new(Text::from(Span::raw(
            self.history_count.map_or_else(String::new, |count| format!("history count: {count}")),
        )))
        .style(Style::from_crossterm(theme.as_style(Meaning::Annotation)))
        .alignment(Alignment::Right)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_results_list<'a>(
        style: StyleState,
        results: &'a [History],
        keymap_mode: KeymapMode,
        now: &'a dyn Fn() -> OffsetDateTime,
        tz: UtcOffset,
        indicator: &'a str,
        theme: &'a Theme,
        history_highlighter: HistoryHighlighter<'a>,
        show_numeric_shortcuts: bool,
        syntax_highlight: bool,
        columns: &'a [UiColumn],
    ) -> HistoryList<'a> {
        let results_list = HistoryList::new(
            results,
            style.invert,
            keymap_mode == KeymapMode::VimNormal,
            now,
            tz,
            indicator,
            theme,
            history_highlighter,
            show_numeric_shortcuts,
            syntax_highlight,
            columns,
        );

        match style.compactness {
            Compactness::Full => {
                if style.invert {
                    results_list.block(
                        Block::default()
                            .borders(Borders::LEFT | Borders::RIGHT)
                            .border_type(BorderType::Rounded)
                            .title(format!("{:─>width$}", "", width = style.inner_width - 2)),
                    )
                } else {
                    results_list.block(
                        Block::default()
                            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                            .border_type(BorderType::Rounded),
                    )
                }
            }
            _ => results_list,
        }
    }

    fn build_input(&self, style: StyleState, prefix_width: u16) -> Paragraph<'_> {
        let (pref, mode) = if self.prefix {
            ("", "PREFIX")
        } else if self.switched_search_mode {
            (" SRCH:", self.search_mode_state.raw_mode().as_str())
        } else if self.search.custom_context.is_some() {
            (" CTX:", self.search.filter_mode.as_str())
        } else {
            ("", self.search.filter_mode.as_str())
        };
        // 3: surrounding "[" "] "
        let mode_width = usize::from(prefix_width) - pref.len() - 3;
        // sanity check to ensure we don't exceed the layout limits
        debug_assert!(mode_width >= mode.len(), "mode name '{mode}' is too long!");
        let input = format!("[{pref}{mode:^mode_width$}] {}", self.search.input.as_str());
        let input = Paragraph::new(input);
        match style.compactness {
            Compactness::Full => {
                if style.invert {
                    input.block(
                        Block::default()
                            .borders(Borders::LEFT | Borders::RIGHT | Borders::TOP)
                            .border_type(BorderType::Rounded),
                    )
                } else {
                    input.block(
                        Block::default()
                            .borders(Borders::LEFT | Borders::RIGHT)
                            .border_type(BorderType::Rounded)
                            .title(format!("{:─>width$}", "", width = style.inner_width - 2)),
                    )
                }
            }
            _ => input,
        }
    }

    fn build_preview(
        &self,
        results: &[History],
        compactness: Compactness,
        preview_width: u16,
        chunk_width: usize,
        theme: &Theme,
    ) -> Paragraph<'_> {
        let selected = self.results_state.selected();
        let command = if results.is_empty() {
            String::new()
        } else {
            let s = &results[selected].command;
            let mut lines = Vec::new();
            for line in s.split('\n') {
                let line = line.escape_non_printable();
                let mut width = 0;
                let mut start = 0;
                for (idx, ch) in line.char_indices() {
                    let w = ch.width().unwrap_or(0); // None for control chars which should not happen
                    if width + w > preview_width.into() {
                        lines.push(line[start..idx].to_owned());
                        start = idx;
                        width = w;
                    } else {
                        width += w;
                    }
                }
                if width != 0 {
                    lines.push(line[start..].to_owned());
                }
            }
            lines.join("\n")
        };

        match compactness {
            Compactness::Full => Paragraph::new(command).block(
                Block::default()
                    .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded)
                    .title(format!("{:─>width$}", "", width = chunk_width - 2)),
            ),
            _ => Paragraph::new(command)
                .style(Style::from_crossterm(theme.as_style(Meaning::Annotation))),
        }
    }
}

/// The writer used for terminal output - either stdout or /dev/tty
enum TerminalWriter {
    Stdout(std::io::Stdout),
    #[cfg(unix)]
    Tty(std::fs::File),
    #[cfg(windows)]
    ConOut(std::io::LineWriter<std::fs::File>, u32),
}

impl TerminalWriter {
    #[cfg(windows)]
    const CP_UTF8: u32 = 65001;

    fn new() -> std::io::Result<Self> {
        let stdout = stdout();
        if stdout.is_terminal() {
            return Ok(TerminalWriter::Stdout(stdout));
        }

        // If stdout is not a terminal (e.g., captured by command substitution),
        // fall back to /dev/tty so the TUI can still render.
        // This allows usage like: VAR=$(atuin search -i)
        #[cfg(unix)]
        {
            Ok(TerminalWriter::Tty(
                std::fs::File::options().read(true).write(true).open("/dev/tty")?,
            ))
        }

        // On Windows, use CONOUT$ which is the equivalent of /dev/tty, but this
        // requires setting the current console output code page to UTF-8 for the
        // TUI to render properly. We'll set it back to its previous value upon exit.
        #[cfg(windows)]
        {
            let file = std::fs::File::options().read(true).write(true).open("CONOUT$")?;

            let initial_console_output_cp = unsafe { GetConsoleOutputCP() };
            if initial_console_output_cp != Self::CP_UTF8 {
                unsafe {
                    SetConsoleOutputCP(Self::CP_UTF8);
                }
            }

            Ok(TerminalWriter::ConOut(std::io::LineWriter::new(file), initial_console_output_cp))
        }

        #[cfg(not(any(unix, windows)))]
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Interactive mode requires a terminal",
        ))
    }
}

impl Write for TerminalWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            TerminalWriter::Stdout(stdout) => stdout.write(buf),
            #[cfg(unix)]
            TerminalWriter::Tty(file) => file.write(buf),
            #[cfg(windows)]
            TerminalWriter::ConOut(writer, _) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            TerminalWriter::Stdout(stdout) => stdout.flush(),
            #[cfg(unix)]
            TerminalWriter::Tty(file) => file.flush(),
            #[cfg(windows)]
            TerminalWriter::ConOut(writer, _) => writer.flush(),
        }
    }
}

impl Drop for TerminalWriter {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let TerminalWriter::ConOut(_, initial_console_output_cp) = self
            && *initial_console_output_cp != Self::CP_UTF8
        {
            unsafe {
                SetConsoleOutputCP(*initial_console_output_cp);
            }
        }
    }
}

/// Screen state captured from atuin pty-proxy's screen server.
#[cfg(unix)]
struct SavedScreen {
    #[allow(dead_code)]
    rows: u16,
    #[allow(dead_code)]
    cols: u16,
    cursor_row: u16,
    cursor_col: u16,
    /// Pre-formatted ANSI bytes for each screen row, ready to write to stdout.
    rows_data: Vec<Vec<u8>>,
}

/// Fetch the current screen state from the given PTY proxy socket.
///
/// The wire format is:
///
/// ```text
/// [rows: u16 BE][cols: u16 BE][cursor_row: u16 BE][cursor_col: u16 BE]
/// [row_0_len: u32 BE][row_0_bytes...]
/// [row_1_len: u32 BE][row_1_bytes...]
/// ...
/// ```
#[cfg(unix)]
fn fetch_screen_state(socket_path: &std::path::Path) -> Option<SavedScreen> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket_path).ok()?;
    // We only read from this socket, but an older version of the PTY proxy might be waiting up to
    // 100ms for us to send a magic byte we never do; shut down the write end of the socket
    // immediately to cancel the timeout.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;

    let mut data = Vec::new();
    stream.read_to_end(&mut data).ok()?;

    if data.len() < 8 {
        return None;
    }

    let rows = u16::from_be_bytes([data[0], data[1]]);
    let cols = u16::from_be_bytes([data[2], data[3]]);
    let cursor_row = u16::from_be_bytes([data[4], data[5]]);
    let cursor_col = u16::from_be_bytes([data[6], data[7]]);

    // Parse length-prefixed rows
    let mut rows_data = Vec::with_capacity(usize::conv(rows));
    let mut offset = 8;
    while offset + 4 <= data.len() {
        let row_len = usize::conv(u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]));
        offset += 4;
        if offset + row_len > data.len() {
            break;
        }
        rows_data.push(data[offset..offset + row_len].to_vec());
        offset += row_len;
    }

    Some(SavedScreen {
        rows,
        cols,
        cursor_row,
        cursor_col,
        rows_data,
    })
}

/// Restore the screen area that was covered by the popup.
///
/// Writes the pre-formatted per-row ANSI bytes received from atuin pty-proxy
/// directly to stdout, which correctly handles wide characters, colors, and
/// all text attributes without needing a client-side vt100 parser.
#[cfg(unix)]
fn restore_popup_area(saved: &SavedScreen, popup_rect: Rect, scroll_offset: u16) {
    use ratatui::crossterm::cursor::MoveTo;
    use ratatui::crossterm::style::{Attribute, SetAttribute};
    use ratatui::crossterm::terminal::{Clear, ClearType};

    let mut stdout = stdout();

    for dy in 0..popup_rect.height {
        let target_row = popup_rect.y + dy;
        let source_row = usize::conv(target_row + scroll_offset);

        // The snapshot from the server spans the full width of the terminal, not just the popup
        // area, so move to the start of the line and clear the whole line before we write the row
        // contents.
        let _ = execute!(
            stdout,
            MoveTo(0, target_row),
            SetAttribute(Attribute::Reset),
            Clear(ClearType::CurrentLine),
        );

        if let Some(row_bytes) = saved.rows_data.get(source_row) {
            let _ = stdout.write_all(row_bytes);
        }
    }

    let _ =
        execute!(stdout, MoveTo(saved.cursor_col, saved.cursor_row.saturating_sub(scroll_offset)));
    let _ = stdout.flush();
}

struct Stdout {
    writer: TerminalWriter,
    inline_mode: bool,
    no_mouse: bool,
}

impl Stdout {
    pub fn new(inline_mode: bool, no_mouse: bool) -> std::io::Result<Self> {
        terminal::enable_raw_mode()?;

        let mut writer = TerminalWriter::new()?;

        if !inline_mode {
            execute!(writer, terminal::EnterAlternateScreen)?;
        }

        if !no_mouse {
            execute!(writer, event::EnableMouseCapture)?;
        }

        execute!(writer, event::EnableBracketedPaste)?;

        #[cfg(not(target_os = "windows"))]
        execute!(
            writer,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            ),
        )?;

        Ok(Self {
            writer,
            inline_mode,
            no_mouse,
        })
    }
}

impl Drop for Stdout {
    fn drop(&mut self) {
        #[cfg(not(target_os = "windows"))]
        if let Err(e) = execute!(self.writer, PopKeyboardEnhancementFlags) {
            tracing::error!(?e, "Failed to pop keyboard enhancement flags");
        }

        if !self.inline_mode
            && let Err(e) = execute!(self.writer, terminal::LeaveAlternateScreen)
        {
            tracing::error!(?e, "Failed to leave alt screen mode");
        }

        if !self.no_mouse
            && let Err(e) = execute!(self.writer, event::DisableMouseCapture)
        {
            tracing::error!(?e, "Failed to disable mouse capture");
        }

        if let Err(e) = execute!(self.writer, event::DisableBracketedPaste) {
            tracing::error!(?e, "Failed to disable bracketed paste");
        }

        if let Err(e) = terminal::disable_raw_mode() {
            tracing::error!(?e, "Failed to disable raw mode");
        }
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

// this is a big blob of horrible! clean it up!
/// Compute the popup position and any scroll offset needed to make room.
///
/// Given the cursor row, terminal dimensions, and desired popup height,
/// returns `(popup_rect, scroll_offset)` where `scroll_offset` is the number
/// of lines the caller should scroll the terminal up before rendering.
///
/// This function performs no I/O — it is a pure computation.
#[cfg(unix)]
fn compute_popup_placement(
    cursor_row: u16,
    term_rows: u16,
    term_cols: u16,
    inline_height: u16,
) -> (Rect, u16) {
    let popup_w = term_cols;
    let popup_h = inline_height.min(term_rows);
    let space_below = term_rows.saturating_sub(cursor_row);

    let (popup_y, scroll) = if popup_h <= space_below {
        // Fits below cursor
        (cursor_row, 0u16)
    } else if cursor_row >= term_rows / 2 {
        // Bottom half — render above cursor (overlay on existing text)
        (cursor_row.saturating_sub(popup_h), 0u16)
    } else {
        // Top half, not enough space — scroll terminal to make room
        let scroll = popup_h.saturating_sub(space_below);
        let popup_y = cursor_row.saturating_sub(scroll);
        (popup_y, scroll)
    };

    (Rect::new(0, popup_y, popup_w, popup_h), scroll)
}

// for now, it works. But it'd be great if it were more easily readable, and
// modular. I'd like to add some more stats and stuff at some point
#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
pub async fn history(
    query: &[String],
    settings: &Settings,
    mut db: Sqlite,
    history_store: &HistoryStore,
    theme: &Theme,
) -> Result<String> {
    let inline_height = if settings.shell_up_key_binding {
        settings.inline_height_shell_up_key_binding.unwrap_or(settings.inline_height)
    } else {
        settings.inline_height
    };

    // Use fullscreen mode if the inline height doesn't fit in the terminal,
    // this will preserve the scroll position upon exit.
    // Also force fullscreen when stdout isn't a terminal (e.g., command substitution
    // like VAR=$(atuin search -i)). In that case, we need to use /dev/tty for the TUI and force
    // fullscreen mode (inline mode won't work as it requires cursor position queries
    // that don't work when stdout is captured).
    let inline_height = if !stdout().is_terminal() {
        0
    } else if let Ok(size) = terminal::size()
        && inline_height >= size.1
    {
        0
    } else {
        inline_height
    };

    // Popup mode: if running under atuin pty-proxy and inline mode is requested,
    // fetch the screen state and render as a centered overlay.
    #[cfg(unix)]
    let (saved_screen, popup_rect, popup_scroll_offset) = {
        #[cfg(feature = "pty-proxy")]
        let socket_path = atuin_pty_proxy::parent_socket_path();
        #[cfg(not(feature = "pty-proxy"))]
        let socket_path = None::<std::path::PathBuf>;

        if let Some(ref path) = socket_path
            && inline_height > 0
        {
            let saved = fetch_screen_state(path);
            if let Some(ref s) = saved {
                let (term_cols, term_rows) = terminal::size().unwrap_or((s.cols, s.rows));
                let (popup_rect, scroll) =
                    compute_popup_placement(s.cursor_row, term_rows, term_cols, inline_height);

                // Scroll terminal content up to make room if needed
                if scroll > 0 {
                    use ratatui::crossterm::cursor::MoveTo;
                    let mut stdout = stdout();
                    let _ = execute!(stdout, MoveTo(0, term_rows - 1));
                    for _ in 0..scroll {
                        let _ = writeln!(stdout);
                    }
                    let _ = stdout.flush();
                }

                (saved, popup_rect, scroll)
            } else {
                (None, Rect::default(), 0u16)
            }
        } else {
            (None, Rect::default(), 0u16)
        }
    };

    #[cfg(not(unix))]
    let (saved_screen, popup_rect, _popup_scroll_offset): (Option<()>, Rect, u16) =
        (None, Rect::default(), 0);

    let popup_mode = saved_screen.is_some();

    let stdout = Stdout::new(inline_height > 0, settings.no_mouse)?;

    // In popup mode, clear the popup region on the physical terminal before
    // ratatui takes over. Ratatui's diff-based rendering compares against an
    // initially-empty buffer, so cells that remain "empty" (spaces with default
    // style) won't be written — leaving underlying terminal text visible.
    // By pre-clearing with spaces, those cells are already correct on screen.
    if popup_mode {
        use ratatui::crossterm::cursor::MoveTo;
        let mut raw_stdout = std::io::stdout();
        // Queue all commands without flushing so the terminal receives them
        // as a single write — no intermediate cursor positions are visible.
        let _ = queue!(
            raw_stdout,
            ratatui::crossterm::style::SetAttribute(ratatui::crossterm::style::Attribute::Reset)
        );
        for row in popup_rect.y..popup_rect.y.saturating_add(popup_rect.height) {
            let _ = queue!(raw_stdout, MoveTo(popup_rect.x, row));
            let _ = write!(raw_stdout, "{:width$}", "", width = usize::conv(popup_rect.width));
        }
        let _ = raw_stdout.flush();
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::with_options(backend, TerminalOptions {
        viewport: if popup_mode {
            Viewport::Fixed(popup_rect)
        } else if inline_height > 0 {
            Viewport::Inline(inline_height)
        } else {
            Viewport::Fullscreen
        },
    })?;

    let original_query = query.join(" ");

    // Check if this is a command chaining scenario
    let is_command_chaining = if settings.command_chaining {
        let trimmed = original_query.trim_end();
        trimmed.ends_with("&&") || trimmed.ends_with('|')
    } else {
        false
    };

    // For command chaining, start with empty input to allow searching for new commands
    let search_input = if is_command_chaining {
        String::new()
    } else {
        original_query.clone()
    };

    let mut input = Cursor::from(search_input);
    // Put the cursor at the end of the query by default
    input.end();

    let settings2 = settings.clone();
    let update_needed = tokio::spawn(async move { settings2.needs_update().await }).fuse();
    tokio::pin!(update_needed);

    // Counting history is a full table scan, which can take a while on a large,
    // cold database - don't hold up the first frame for it.
    let count_db = db.clone();
    let history_count = tokio::spawn(async move { count_db.history_count(false).await }).fuse();
    tokio::pin!(history_count);

    let initial_context = current_context().await?;
    let search_mode_state = SearchModeState::new(settings);
    let default_filter_mode = settings
        .filter_mode_shell_up_key_binding
        .filter(|_| settings.shell_up_key_binding)
        .unwrap_or_else(|| settings.default_filter_mode(initial_context.git_root.is_some()));

    let mut app = State {
        history_count: None,
        results_state: ListState::default(),
        update_needed: None,
        switched_search_mode: false,
        tab_index: 0,
        inspecting_state: InspectingState::default(),
        keymaps: KeymapSet::from_settings(settings),
        search: SearchState {
            input,
            filter_mode: default_filter_mode,
            context: initial_context.clone(),
            custom_context: None,
            shells: settings.search.shells.clone(),
        },
        engine: engines::engine(search_mode_state.mode(), settings),
        search_mode_state,
        results_len: 0,
        accept: false,
        cd: false,
        keymap_mode: match settings.keymap_mode {
            KeymapMode::Auto => KeymapMode::Emacs,
            value => value,
        },
        current_cursor: None,
        now: if settings.prefers_reduced_motion {
            let now = OffsetDateTime::now_utc();
            Box::new(move || now)
        } else {
            Box::new(OffsetDateTime::now_utc)
        },
        prefix: false,
        pending_vim_key: None,
        pending_vim_key_since: None,
        queued_key: None,
        original_input_empty: original_query.is_empty(),
    };

    app.initialize_keymap_cursor(settings);

    if inline_height > 0 && !popup_mode {
        terminal.clear()?;
    }

    // Paint the UI before running the first search: on a cold start the query can
    // block for a while (cold database pages, sleeping daemon), and the user should
    // see the search UI immediately rather than a frozen terminal.
    terminal.draw(|f| {
        app.draw(f, &[], None, None, settings, theme, popup_mode);
    })?;

    let mut results = app.query_results(&mut db, settings).await?;

    let mut stats: Option<InspectorStats> = None;
    // Aggregates depend on the command, not the selected occurrence.
    let mut stats_for: Option<String> = None;
    let mut inspecting: Option<History> = None;
    let accept;
    let result = 'render: loop {
        if app.tab_index == 1 {
            let context = app.eval_context();
            app.inspecting_state.bindings.update(&app.keymaps.inspector, &context);
            if let Some(selected) =
                inspecting.as_ref().or_else(|| results.get(app.results_state.selected()))
            {
                app.inspecting_state.browser.prepare(selected, settings, theme);
            }
        }

        terminal.draw(|f| {
            app.draw(f, &results, stats.as_ref(), inspecting.as_ref(), settings, theme, popup_mode);
        })?;

        let initial_input = app.search.input.as_str().to_owned();
        let initial_filter_mode = app.search.filter_mode;
        let initial_search_mode = app.search_mode();
        let initial_custom_context = app.search.custom_context;

        let pending_key_timeout = app.pending_key_timeout(settings);
        let poll_for = if app.has_queued_key() {
            Duration::ZERO
        } else {
            pending_key_timeout.unwrap_or(Duration::from_millis(250))
        };
        let event_ready = tokio::task::spawn_blocking(move || event::poll(poll_for));

        tokio::select! {
            event_ready = event_ready => {
                let event_ready = event_ready??;
                let timed_out =
                    !event_ready && !app.has_queued_key() && pending_key_timeout.is_some();
                if event_ready || timed_out || app.has_queued_key() {
                    loop {
                        let input_action = if app.has_queued_key() {
                            app.handle_queued_key(settings)
                        } else if timed_out {
                            app.flush_pending_key(settings)
                        } else {
                            app.handle_input(settings, &event::read()?)
                        };
                        match input_action {
                            InputAction::Continue => {},
                            InputAction::DeleteInspecting => {
                                if let Some(id) = app.inspecting_state.current {
                                    if let Some(entry) = db.load(id).await? {
                                        crate::command::client::history::delete_history_entries(
                                            settings, history_store, &db, [entry],
                                        ).await?;
                                    }
                                    app.tab_index = 0;
                                    results = app.query_results(&mut db, settings).await?;
                                    break;
                                }
                            },
                            InputAction::Delete(index) => {
                                if results.is_empty() {
                                    break;
                                }
                                app.results_len -= 1;
                                let selected = app.results_state.selected();
                                if selected == app.results_len {
                                    app.inspecting_state.reset();
                                    app.results_state.select(selected - 1);
                                }

                                let entry = results.remove(index);

                                crate::command::client::history::delete_history_entries(
                                    settings,
                                    history_store,
                                    &db,
                                    [entry],
                                )
                                .await?;

                                app.tab_index  = 0;
                            },
                            InputAction::DeleteAllMatching(index) => {
                                if results.is_empty() {
                                    break;
                                }

                                let command = if app.tab_index == 1 {
                                    let Some(entry) = inspecting.as_ref() else { break; };
                                    entry.command.clone()
                                } else {
                                    results[index].command.clone()
                                };

                                // Remove matching entries from the visible results
                                results.retain(|e| e.command != command);

                                // Query the DB for ALL entries with this command and delete them
                                let all_matching = db.query_history(
                                    &format!(
                                        "select {} from history where command = '{}' and deleted_at is null",
                                        atuin_client::database::HISTORY_COLUMNS,
                                        command.replace('\'', "''")
                                    )
                                ).await?;

                                crate::command::client::history::delete_history_entries(
                                    settings,
                                    history_store,
                                    &db,
                                    all_matching,
                                )
                                .await?;

                                app.results_len = results.len();
                                app.results_state = ListState::default();
                                app.inspecting_state.reset();
                                app.tab_index = 0;
                            },
                            InputAction::SwitchContext(index) => {
                                let entry = index.and_then(|index| {
                                    if app.tab_index == 1 { inspecting.as_ref() } else { results.get(index) }
                                });
                                if let Some(entry) = entry {
                                    app.search.custom_context = Some(entry.id);
                                    app.search.context = Context::from_history(entry);
                                    app.search.filter_mode = FilterMode::Session;
                                    app.search.input = Cursor::from(String::new());
                                    app.results_state = ListState::default();
                                } else {
                                    app.search.custom_context = None;
                                    app.search.context = initial_context.clone();
                                    app.search.filter_mode = default_filter_mode;
                                }
                                // Apply the context before consuming a queued accept/navigation key.
                                break;
                            },
                            InputAction::Redraw => {
                                // Inspector navigation uses ratatui's diff; clearing on every scroll flickers.
                                if !popup_mode && app.tab_index != 1 {
                                    terminal.clear()?;
                                }
                                // Refresh the selected occurrence before drawing or accepting another key.
                                break;
                            },
                            r => {
                                accept = app.accept;
                                break 'render r;
                            },
                        }
                        if timed_out || (!app.has_queued_key() && !event::poll(Duration::ZERO)?) {
                            break;
                        }
                    }
                }
            }
            update_needed = &mut update_needed => {
                // Don't fail interactive search if update check fails
                // The update check is a nice-to-have feature, not critical
                app.update_needed = update_needed.ok().flatten();
            }
            history_count = &mut history_count => {
                app.history_count = history_count.ok().and_then(Result::ok);
            }
        }

        if initial_input != app.search.input.as_str()
            || initial_filter_mode != app.search.filter_mode
            || initial_search_mode != app.search_mode()
            || initial_custom_context != app.search.custom_context
        {
            results = app.query_results(&mut db, settings).await?;
        }

        // In custom context mode, when no filter is applied, highlight the entry which was used
        // to enter the context when changing modes. This helps to find your way around.
        if app.search.custom_context.is_some()
            && app.search.input.as_str().is_empty()
            && (initial_custom_context != app.search.custom_context
                || initial_filter_mode != app.search.filter_mode)
            && let Some(history_id) = app.search.custom_context
            && let Some(pos) = results.iter().position(|entry| entry.id == history_id)
        {
            app.results_state.select(pos);
        }

        let inspecting_id = app.inspecting_state.current;
        // If inspecting ID is not the current inspecting History, update it.
        match inspecting_id {
            Some(inspecting_id) => {
                if inspecting.as_ref().is_none_or(|entry| inspecting_id != entry.id) {
                    inspecting = db.load(inspecting_id).await?;
                }
            }
            _ => {
                inspecting = None;
            }
        }

        if app.tab_index == 1 && inspecting.is_none() {
            inspecting = results.get(app.results_state.selected()).cloned();
        }
        stats = if app.tab_index == 0 {
            stats_for = None;
            None
        } else if let Some(selected) = inspecting.as_ref() {
            app.inspecting_state.current = Some(selected.id);
            if app.inspecting_state.browser.view == InspectorView::Stats {
                app.inspecting_state.previous = None;
                app.inspecting_state.next = None;
                if stats_for.as_deref() == Some(selected.command.as_str()) {
                    stats
                } else {
                    stats_for = Some(selected.command.clone());
                    Some(db.stats(selected).await?.into())
                }
            } else {
                (app.inspecting_state.previous, app.inspecting_state.next) =
                    app.inspecting_state.browser.refresh(&db, selected, settings).await?;
                stats_for = None;
                None
            }
        } else {
            stats_for = None;
            None
        };
    };

    app.finalize_keymap_cursor(settings);

    if popup_mode {
        // In popup mode, restore the screen area that was covered by the popup.
        // This must happen before Stdout is dropped (which disables raw mode).
        #[cfg(unix)]
        if let Some(ref saved) = saved_screen {
            restore_popup_area(saved, popup_rect, popup_scroll_offset);
        }
    } else if inline_height > 0 {
        terminal.clear()?;
        // ratatui-core v0.1.1 changed the behavior of `Terminal::clear` so it no longer moves the
        // cursor to the viewport origin; do that manually here.
        let origin = terminal.get_frame().area().as_position();
        terminal.set_cursor_position(origin)?;
    }

    let shell = Shell::from_env();
    let accept = accept
        && matches!(
            shell,
            Shell::Zsh | Shell::Fish | Shell::Bash | Shell::Xonsh | Shell::Nu | Shell::Powershell
        );

    let chain = is_command_chaining.then_some(original_query.as_str());

    match result {
        InputAction::AcceptInspecting => Ok(inspecting
            .map(|entry| selection_output(entry, app.cd, accept, None, &shell))
            .unwrap_or_default()),
        InputAction::Accept(index) if index < results.len() => {
            Ok(selection_output(results.swap_remove(index), app.cd, accept, chain, &shell))
        }
        InputAction::ReturnOriginal => Ok(String::new()),
        InputAction::Copy(index) => {
            let cmd = if app.tab_index == 1 {
                inspecting.map(|entry| entry.command).unwrap_or_default()
            } else {
                results.swap_remove(index).command
            };
            if let Err(e) = set_clipboard(cmd) {
                tracing::warn!(?e, "failed to copy to clipboard");
            }
            Ok(String::new())
        }
        InputAction::ReturnQuery | InputAction::Accept(_) => {
            // Either:
            // * index == RETURN_QUERY, in which case we should return the input
            // * out of bounds -> usually implies no selected entry so we return the input
            Ok(app.search.input.into_inner())
        }
        InputAction::Continue
        | InputAction::Redraw
        | InputAction::Delete(_)
        | InputAction::DeleteInspecting
        | InputAction::DeleteAllMatching(_)
        | InputAction::SwitchContext(_) => {
            unreachable!("should have been handled!")
        }
    }
}

/// The line returned to the shell for the selected `entry`; empty returns the original command line.
fn selection_output(
    entry: History,
    cd: bool,
    accept: bool,
    chain: Option<&str>,
    shell: &Shell,
) -> String {
    let command = if cd {
        match cd_command(&entry.cwd, shell) {
            Some(command) => command,
            None => return String::new(),
        }
    } else {
        entry.command
    };
    match chain {
        Some(query) => format!("{} {command}", query.trim_end()),
        None if accept => format!("{ACCEPT_PREFIX}{command}"),
        None => command,
    }
}

/// Build a command that changes into `cwd` in `shell`, or `None` when no safe one exists: `cwd` is
/// not absolute (imported entries store `unknown`), holds a control or line-separator character, or
/// the shell is unknown so its quoting is too.
fn cd_command(cwd: &str, shell: &Shell) -> Option<String> {
    if !std::path::Path::new(cwd).is_absolute()
        || cwd.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return None;
    }
    Some(match shell {
        // Single quotes only: double quotes still allow `!` history expansion in bash and zsh.
        Shell::Sh | Shell::Bash | Shell::Zsh => format!("cd -- '{}'", cwd.replace('\'', r"'\''")),
        Shell::Fish => format!("cd -- {}", backslash_quote(cwd)),
        // Raw strings have no escapes; use one `#` more than any run following a `'`.
        Shell::Nu => {
            let longest = cwd
                .split('\'')
                .skip(1)
                .map(|s| s.len() - s.trim_start_matches('#').len())
                .max()
                .unwrap_or(0);
            let hashes = "#".repeat(longest + 1);
            format!("cd r{hashes}'{cwd}'{hashes}")
        }
        // xonsh expands `$VAR` in quoted subprocess args; `@()` evaluates a plain Python string.
        // Its `cd` also rejects `--`.
        Shell::Xonsh => format!("cd @({})", backslash_quote(cwd)),
        // Outside Windows, PowerShell treats `\` as a separator even in `-LiteralPath`.
        Shell::Powershell if cfg!(not(windows)) && cwd.contains('\\') => return None,
        Shell::Powershell => {
            // PowerShell also treats curly single quotes as string delimiters.
            let mut quoted = String::with_capacity(cwd.len() + 2);
            for c in cwd.chars() {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
                    quoted.push(c);
                }
                quoted.push(c);
            }
            format!("Set-Location -LiteralPath '{quoted}'")
        }
        Shell::Unknown => return None,
    })
}

/// Single-quote `s` for fish and Python: doubling every `\` and escaping `'` is enough in both.
fn backslash_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
}

// cli-clipboard only works on Windows, Mac, and Linux.

#[cfg(all(
    feature = "clipboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
fn set_clipboard(s: String) -> Result<(), arboard::Error> {
    let mut ctx = arboard::Clipboard::new()?;
    ctx.set_text(s)?;
    // Use the clipboard context to make sure it is saved
    ctx.get_text()?;
    Ok(())
}

#[cfg(not(all(
    feature = "clipboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
)))]
fn set_clipboard(_s: String) -> Result<(), std::convert::Infallible> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use atuin_client::database::Context;
    use atuin_client::history::History;
    #[cfg(unix)]
    use atuin_client::settings::RequestedSearchMode;
    use atuin_client::settings::{
        FilterMode, KeymapMode, Preview, PreviewStrategy, SearchMode, Settings, Shells,
    };
    use atuin_common::shell::Shell;
    use rstest::{fixture, rstest};
    use time::OffsetDateTime;

    use super::{
        Compactness, InputAction, InspectingState, InspectorView, KeymapSet, SearchModeState,
        State, cd_command, selection_output,
    };
    use crate::command::client::search::engines::{self, SearchState};
    use crate::command::client::search::history_list::ListState;
    use crate::command::client::search::keybindings::Action;

    #[fixture]
    fn settings() -> Settings {
        Settings::utc()
    }

    /// Build a full `State` for tests. Override the leading params via `#[with(..)]`
    /// (positional/left-anchored: `keymap_mode`, `results_len`, `selected`, `filter_mode`, `input`).
    #[fixture]
    fn state(
        #[default(KeymapMode::Emacs)] keymap_mode: KeymapMode,
        #[default(100usize)] results_len: usize,
        #[default(0usize)] selected: usize,
        #[default(FilterMode::Global)] filter_mode: FilterMode,
        #[default("")] input: &str,
    ) -> State {
        let mut state = State {
            history_count: Some(i64::try_from(results_len).unwrap()),
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode_state: SearchModeState {
                mode: SearchMode::DaemonFuzzy,
                daemon_failed: false,
            },
            results_len,
            accept: false,
            cd: false,
            keymap_mode,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            pending_vim_key_since: None,
            queued_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState::default(),
            keymaps: KeymapSet::defaults(&Settings::utc()),
            search: SearchState {
                input: input.to_string().into(),
                filter_mode,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    cmd_origin: atuin_domain::record::CmdOrigin::default(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
                shells: Shells::all(),
            },
            engine: engines::engine(SearchMode::Fuzzy, &Settings::utc()),
            now: Box::new(OffsetDateTime::now_utc),
        };
        state.results_state.select(selected);
        state
    }

    /// Build a read-only history corpus (60, 124 and 200 character commands) for
    /// the preview-height cases. Shared across all cases via `#[once]`.
    #[fixture]
    #[once]
    fn preview_corpus() -> Vec<History> {
        let cmd_60: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("for i in $(seq -w 10); do echo \"item number $i - abcd\"; done")
            .cwd("/")
            .build()
            .into();

        let cmd_124: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command(
                "echo 'Aurea prima sata est aetas, quae vindice nullo, sponte sua, sine lege \
                 fidem rectumque colebat. Poena metusque aberant'",
            )
            .cwd("/")
            .build()
            .into();

        let cmd_200: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command(
                "CREATE USER atuin WITH ENCRYPTED PASSWORD 'supersecretpassword'; CREATE DATABASE \
                 atuin WITH OWNER = atuin; \\c atuin; REVOKE ALL PRIVILEGES ON SCHEMA public FROM \
                 PUBLIC; echo 'All done. 200 characters'",
            )
            .cwd("/")
            .build()
            .into();

        vec![cmd_60, cmd_124, cmd_200]
    }

    /// Build `Settings` for a preview strategy, optionally overriding the max height.
    fn preview_settings(strategy: PreviewStrategy, max: Option<u16>) -> Settings {
        let mut s = Settings::utc();
        s.show_preview = true;
        s.preview = Preview { strategy };
        if let Some(m) = max {
            s.max_preview_height = m;
        }
        s
    }

    // The border space (`border_size * 2`) is 2 in every case below.
    #[rstest]
    #[case::no_preview(PreviewStrategy::Auto, None, 0, 80, 1)]
    #[case::auto_h2(PreviewStrategy::Auto, None, 1, 80, 4)]
    #[case::auto_h3(PreviewStrategy::Auto, None, 2, 80, 5)]
    #[case::auto_one_line(PreviewStrategy::Auto, None, 0, 66, 3)]
    #[case::auto_limit_2(PreviewStrategy::Auto, Some(2), 2, 80, 4)]
    #[case::static_h3(PreviewStrategy::Static, Some(4), 1, 80, 5)]
    #[case::static_limit_4(PreviewStrategy::Static, Some(4), 1, 20, 6)]
    #[case::fixed(PreviewStrategy::Fixed, Some(15), 1, 20, 17)]
    fn calc_preview_height_cases(
        #[from(preview_corpus)] results: &[History],
        #[case] strategy: PreviewStrategy,
        #[case] max: Option<u16>,
        #[case] selected: usize,
        #[case] preview_width: u16,
        #[case] expected: u16,
    ) {
        assert_eq!(
            State::calc_preview_height(
                &preview_settings(strategy, max),
                results,
                selected,
                0,
                Compactness::Full,
                1,
                preview_width,
            ),
            expected
        );
    }

    // Test when there's no results, scrolling up or down doesn't underflow
    #[rstest]
    fn state_scroll_up_underflow(
        #[with(KeymapMode::Auto, 0, 0, FilterMode::Directory)] mut state: State,
    ) {
        state.scroll_up(1);
        state.scroll_down(1);
    }

    #[allow(clippy::too_many_lines)]
    #[rstest]
    fn test_accept_keybindings(
        #[with(KeymapMode::Emacs, 1)] mut state: State,
        mut settings: Settings,
    ) {
        use atuin_client::settings::Keys;
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        settings.keys = Keys {
            scroll_exits: true,
            exit_past_line_start: false,
            accept_past_line_end: true,
            accept_past_line_start: false,
            accept_with_backspace: false,
            prefix: "a".to_string(),
        };
        state.keymaps = KeymapSet::defaults(&settings);

        let tab_event = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &tab_event);
        assert!(matches!(result, super::InputAction::Accept(_)), "Tab should always accept");

        // Test left arrow with accept_past_line_start disabled (should continue)
        let left_event = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &left_event);
        assert!(
            matches!(result, super::InputAction::Continue),
            "Left arrow should continue when disabled"
        );

        // Test left arrow with accept_past_line_start enabled (should accept at start of line)
        settings.keys.accept_past_line_start = true;
        state.keymaps = KeymapSet::defaults(&settings);
        let result = state.handle_key_input(&settings, &left_event);
        assert!(
            matches!(result, super::InputAction::Accept(_)),
            "Left arrow should accept at start of line when enabled"
        );
        settings.keys.accept_past_line_start = false;
        state.keymaps = KeymapSet::defaults(&settings);

        let backspace_event = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &backspace_event);
        assert!(
            matches!(result, super::InputAction::Continue),
            "Backspace should continue when disabled"
        );

        settings.keys.accept_with_backspace = true;
        state.keymaps = KeymapSet::defaults(&settings);
        let result = state.handle_key_input(&settings, &backspace_event);
        assert!(
            matches!(result, super::InputAction::Accept(_)),
            "Backspace should accept at start of line when enabled"
        );

        state.search.input.insert('t');
        state.search.input.insert('e');
        state.search.input.insert('s');
        state.search.input.insert('t');
        state.search.input.end();

        let right_event = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &right_event);
        assert!(
            matches!(result, super::InputAction::Accept(_)),
            "Right arrow should accept at end of line when enabled"
        );

        settings.keys.accept_past_line_start = true;
        state.keymaps = KeymapSet::defaults(&settings);
        let left_event = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &left_event);
        assert!(
            matches!(result, super::InputAction::Continue),
            "Left arrow should continue and end of line, even when enabled"
        );
        settings.keys.accept_past_line_start = false;
        state.keymaps = KeymapSet::defaults(&settings);

        settings.keys.accept_with_backspace = true;
        state.keymaps = KeymapSet::defaults(&settings);
        let backspace_event = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &backspace_event);
        assert!(
            matches!(result, super::InputAction::Continue),
            "Backspace should continue at end of line, even when enabled"
        );
        settings.keys.accept_with_backspace = false;
        state.keymaps = KeymapSet::defaults(&settings);
    }

    #[rstest]
    fn test_vim_gg_multikey_sequence(
        #[with(KeymapMode::VimNormal)] mut state: State,
        settings: Settings,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        // Start in the middle of the list
        state.results_state.select(50);

        // First 'g' should set pending state
        let g_event = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &g_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.pending_vim_key, Some('g'));
        assert_eq!(state.results_state.selected(), 50); // Position unchanged

        // Second 'g' should jump to end (visual top in non-inverted mode)
        let result = state.handle_key_input(&settings, &g_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.pending_vim_key, None);
        assert_eq!(state.results_state.selected(), 99); // Jumped to last index (visual top)
    }

    #[rstest]
    fn test_vim_g_key_clears_on_other_input(
        #[with(KeymapMode::VimNormal)] mut state: State,
        settings: Settings,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        state.results_state.select(50);

        // Press 'g' to set pending state
        let g_event = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        let _ = state.handle_key_input(&settings, &g_event);
        assert_eq!(state.pending_vim_key, Some('g'));

        // Press 'j' - should clear pending state
        let j_event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        let _ = state.handle_key_input(&settings, &j_event);
        assert_eq!(state.pending_vim_key, None);
    }

    #[rstest]
    fn test_vim_big_g_jump_to_bottom(
        #[with(KeymapMode::VimNormal)] mut state: State,
        settings: Settings,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        state.results_state.select(50);

        // 'G' should jump to visual bottom (index 0 in non-inverted mode)
        let big_g_event = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &big_g_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.results_state.selected(), 0);
    }

    // Ctrl+{d,u,f,b} in vim-normal mode should return Continue and clear any
    // pending vim key. (Scroll amount depends on max_entries, which is 0 in tests.)
    #[rstest]
    fn test_vim_ctrl_scroll_clears_pending(
        #[with(KeymapMode::VimNormal)] mut state: State,
        settings: Settings,
        #[values('d', 'u', 'f', 'b')] c: char,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        state.results_state.select(50);
        state.pending_vim_key = Some('g');
        let r = state
            .handle_key_input(&settings, &KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        assert!(matches!(r, InputAction::Continue));
        assert_eq!(state.pending_vim_key, None);
    }

    /// Vim-insert state with `j k` bound to vim-enter-normal, the usual escape
    /// arpeggio.
    fn state_with_jk_escape(mut state: State) -> State {
        use super::super::keybindings::key::KeyInput;
        state.keymaps.vim_insert.bind(KeyInput::parse("j k").unwrap(), Action::VimEnterNormal);
        state
    }

    fn type_chars(state: &mut State, settings: &Settings, chars: &str) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for c in chars.chars() {
            let event = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            assert!(matches!(state.handle_key_input(settings, &event), InputAction::Continue));
        }
    }

    #[rstest]
    fn test_insert_sequence_completes(
        #[with(KeymapMode::VimInsert)] state: State,
        settings: Settings,
    ) {
        let mut state = state_with_jk_escape(state);
        type_chars(&mut state, &settings, "jk");
        assert_eq!(state.keymap_mode, KeymapMode::VimNormal);
        assert_eq!(state.search.input.as_str(), "");
        assert_eq!(state.pending_vim_key, None);
    }

    // A pending key that doesn't start the sequence after all is typed, not lost.
    #[rstest]
    fn test_insert_sequence_mismatch_types_both_keys(
        #[with(KeymapMode::VimInsert)] state: State,
        settings: Settings,
    ) {
        let mut state = state_with_jk_escape(state);
        type_chars(&mut state, &settings, "jaj");
        assert_eq!(state.search.input.as_str(), "ja");
        assert_eq!(state.pending_vim_key, Some('j'));
        type_chars(&mut state, &settings, "jk");
        assert_eq!(state.search.input.as_str(), "jaj");
        assert_eq!(state.keymap_mode, KeymapMode::VimNormal);
    }

    #[rstest]
    fn test_insert_sequence_timeout_types_pending_key(
        #[with(KeymapMode::VimInsert)] state: State,
        settings: Settings,
    ) {
        let mut state = state_with_jk_escape(state);
        assert_eq!(state.pending_key_timeout(&settings), None);
        type_chars(&mut state, &settings, "j");
        let left = state.pending_key_timeout(&settings).unwrap();
        assert!(left <= std::time::Duration::from_millis(100), "{left:?}");

        assert!(matches!(state.flush_pending_key(&settings), InputAction::Continue));
        assert_eq!(state.search.input.as_str(), "j");
        assert_eq!(state.pending_vim_key, None);
        assert_eq!(state.keymap_mode, KeymapMode::VimInsert);
    }

    // The timeout counts from the key press, so waking for other work in between
    // doesn't restart it.
    #[rstest]
    fn test_insert_sequence_timeout_counts_from_key_press(
        #[with(KeymapMode::VimInsert)] state: State,
        settings: Settings,
    ) {
        let mut state = state_with_jk_escape(state);
        type_chars(&mut state, &settings, "j");
        state.pending_vim_key_since = Some(
            std::time::Instant::now().checked_sub(std::time::Duration::from_millis(60)).unwrap(),
        );
        let left = state.pending_key_timeout(&settings).unwrap();
        assert!(left <= std::time::Duration::from_millis(40), "{left:?}");

        state.pending_vim_key_since = Some(
            std::time::Instant::now().checked_sub(std::time::Duration::from_millis(500)).unwrap(),
        );
        assert_eq!(state.pending_key_timeout(&settings), Some(std::time::Duration::ZERO));
    }

    // When the pending key's own action needs the event loop (here a redraw), the
    // key after it is queued, not lost.
    #[rstest]
    fn test_sequence_mismatch_queues_key_after_redraw(
        #[with(KeymapMode::VimInsert)] state: State,
        settings: Settings,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use super::super::keybindings::key::KeyInput;

        let mut state = state_with_jk_escape(state);
        state.keymaps.vim_insert.bind(KeyInput::parse("j").unwrap(), Action::ToggleTab);
        type_chars(&mut state, &settings, "j");

        let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(state.handle_key_input(&settings, &a), InputAction::Redraw));
        assert_eq!(state.tab_index, 1);
        assert!(state.has_queued_key());

        let _ = state.handle_queued_key(&settings);
        assert!(!state.has_queued_key());
    }

    // A pending key that turns out to enter prefix mode sends the next key to the
    // prefix keymap: `a` there is cursor-start, not an inserted `a`.
    #[rstest]
    fn test_sequence_mismatch_into_prefix_mode(
        #[with(KeymapMode::VimInsert)] state: State,
        settings: Settings,
    ) {
        use super::super::keybindings::key::KeyInput;

        let mut state = state_with_jk_escape(state);
        state.keymaps.vim_insert.bind(KeyInput::parse("j").unwrap(), Action::EnterPrefixMode);
        type_chars(&mut state, &settings, "xyja");
        assert_eq!(state.search.input.as_str(), "xy");
        assert_eq!(state.search.input.position(), 0);
        assert!(!state.prefix);
    }

    #[rstest]
    fn test_sequence_timeout_zero_waits(
        #[with(KeymapMode::VimInsert)] state: State,
        mut settings: Settings,
    ) {
        let mut state = state_with_jk_escape(state);
        settings.keymap_sequence_timeout_ms = 0;
        type_chars(&mut state, &settings, "j");
        assert_eq!(state.pending_key_timeout(&settings), None);
    }

    // Vim-normal commands like `g g` are never timed.
    #[rstest]
    fn test_vim_normal_sequence_does_not_time_out(
        #[with(KeymapMode::VimNormal)] mut state: State,
        settings: Settings,
    ) {
        type_chars(&mut state, &settings, "g");
        assert_eq!(state.pending_vim_key, Some('g'));
        assert_eq!(state.pending_key_timeout(&settings), None);
    }

    // -----------------------------------------------------------------------
    // Executor tests (execute_action)
    // -----------------------------------------------------------------------

    // Selection/scroll actions, invert-aware. `state` starts at selected index 50
    // in a 100-result list.
    #[rstest]
    #[case::select_next_no_invert(false, Action::SelectNext, 49)]
    #[case::select_next_with_invert(true, Action::SelectNext, 51)]
    #[case::select_previous_no_invert(false, Action::SelectPrevious, 51)]
    #[case::scroll_to_top_no_invert(false, Action::ScrollToTop, 99)]
    #[case::scroll_to_top_with_invert(true, Action::ScrollToTop, 0)]
    #[case::scroll_to_bottom_no_invert(false, Action::ScrollToBottom, 0)]
    fn execute_scroll_selection(
        #[with(KeymapMode::Emacs, 100, 50)] mut state: State,
        mut settings: Settings,
        #[case] invert: bool,
        #[case] action: Action,
        #[case] expected_selected: usize,
    ) {
        settings.invert = invert;
        let r = state.execute_action(&action, &settings);
        assert!(matches!(r, InputAction::Continue));
        assert_eq!(state.results_state.selected(), expected_selected);
    }

    // Vim mode-change actions. `start` is applied in the body because `#[with]`
    // cannot receive a `#[case]` value.
    #[rstest]
    #[case::enter_normal(KeymapMode::Emacs, Action::VimEnterNormal, KeymapMode::VimNormal)]
    #[case::enter_insert(KeymapMode::VimNormal, Action::VimEnterInsert, KeymapMode::VimInsert)]
    fn execute_vim_mode_change(
        mut state: State,
        settings: Settings,
        #[case] start: KeymapMode,
        #[case] action: Action,
        #[case] expected: KeymapMode,
    ) {
        state.keymap_mode = start;
        let r = state.execute_action(&action, &settings);
        assert!(matches!(r, InputAction::Continue));
        assert_eq!(state.keymap_mode, expected);
    }

    #[rstest]
    fn execute_accept_sets_accept_flag(
        #[with(KeymapMode::Emacs, 100, 5)] mut state: State,
        mut settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        settings.enter_accept = true;
        let result = state.execute_action(&Action::Accept, &settings);
        assert!(matches!(result, super::InputAction::Accept(5)));
        assert!(state.accept);
        assert!(!state.cd);
    }

    #[rstest]
    fn execute_return_selection_does_not_set_accept(
        #[with(KeymapMode::Emacs, 100, 5)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::ReturnSelection, &settings);
        assert!(matches!(result, super::InputAction::Accept(5)));
        assert!(!state.accept);
    }

    #[rstest]
    #[case::accept_cd(Action::AcceptCd, 0, InputAction::Accept(5), true)]
    #[case::accept_cd_inspector(Action::AcceptCd, 1, InputAction::AcceptInspecting, true)]
    #[case::return_cd(Action::ReturnCd, 0, InputAction::Accept(5), false)]
    #[case::return_cd_inspector(Action::ReturnCd, 1, InputAction::AcceptInspecting, false)]
    fn execute_cd_selects_entry(
        #[with(KeymapMode::Emacs, 100, 5)] mut state: State,
        settings: Settings,
        #[case] action: Action,
        #[case] tab_index: usize,
        #[case] expected: InputAction,
        #[case] accept: bool,
    ) {
        state.tab_index = tab_index;
        assert_eq!(state.execute_action(&action, &settings), expected);
        assert!(state.cd);
        assert_eq!(state.accept, accept);
    }

    /// Makes a `/`-rooted test path absolute on the host: Windows needs a drive letter. Expected
    /// outputs get the same prefix on their first `/`, which is always the path's.
    fn on_host(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_owned()
        }
    }

    #[rstest]
    #[case::command(false, false, None, "/a b", "echo hi")]
    #[case::command_accept(false, true, None, "/a b", "__atuin_accept__:echo hi")]
    #[case::command_chained(false, true, Some("make && "), "/a b", "make && echo hi")]
    #[case::cd(true, false, None, "/a b", "cd -- '/a b'")]
    #[case::cd_accept(true, true, None, "/a b", "__atuin_accept__:cd -- '/a b'")]
    #[case::cd_chained(true, true, Some("make && "), "/a b", "make && cd -- '/a b'")]
    #[case::cd_chained_untrimmed(true, true, Some("make &&"), "/a b", "make && cd -- '/a b'")]
    #[case::cd_without_directory(true, true, None, "unknown", "")]
    #[case::cd_chained_without_directory(true, true, Some("make && "), "unknown", "")]
    fn selection_output_maps_entry(
        #[case] cd: bool,
        #[case] accept: bool,
        #[case] chain: Option<&str>,
        #[case] cwd: &str,
        #[case] expected: &str,
    ) {
        let entry: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command("echo hi")
            .cwd(on_host(cwd))
            .build()
            .into();
        assert_eq!(
            selection_output(entry, cd, accept, chain, &Shell::Bash),
            expected.replacen('/', &on_host("/"), 1)
        );
    }

    #[rstest]
    #[case::sh(Shell::Sh, r#"cd -- '/a b'\''c"d\e$HOME!'"#)]
    #[case::bash(Shell::Bash, r#"cd -- '/a b'\''c"d\e$HOME!'"#)]
    #[case::zsh(Shell::Zsh, r#"cd -- '/a b'\''c"d\e$HOME!'"#)]
    #[case::fish(Shell::Fish, r#"cd -- '/a b\'c"d\\e$HOME!'"#)]
    #[case::nu(Shell::Nu, r#"cd r#'/a b'c"d\e$HOME!'#"#)]
    #[case::xonsh(Shell::Xonsh, r#"cd @('/a b\'c"d\\e$HOME!')"#)]
    fn cd_command_quotes_per_shell(#[case] shell: Shell, #[case] expected: &str) {
        let expected = expected.replacen('/', &on_host("/"), 1);
        assert_eq!(cd_command(&on_host(r#"/a b'c"d\e$HOME!"#), &shell), Some(expected));
    }

    #[rstest]
    #[case::nu_raw_terminator(Shell::Nu, "/x'#y'##z", "cd r###'/x'#y'##z'###")]
    #[case::powershell_quotes(
        Shell::Powershell,
        "/a b'c\"d$HOME!\u{2018}\u{2019}\u{201a}\u{201b}",
        "Set-Location -LiteralPath '/a \
         b''c\"d$HOME!\u{2018}\u{2018}\u{2019}\u{2019}\u{201a}\u{201a}\u{201b}\u{201b}'"
    )]
    fn cd_command_escapes_shell_specific_delimiters(
        #[case] shell: Shell,
        #[case] cwd: &str,
        #[case] expected: &str,
    ) {
        let expected = expected.replacen('/', &on_host("/"), 1);
        assert_eq!(cd_command(&on_host(cwd), &shell), Some(expected));
    }

    #[rstest]
    #[case::unknown("unknown")]
    #[case::empty("")]
    #[case::relative("some/dir")]
    #[case::newline("/a\nb")]
    #[case::carriage_return("/a\rb")]
    #[case::escape("/a\x1b[2Jb")]
    #[case::nul("/a\0b")]
    #[case::next_line("/a\u{85}b")]
    #[case::line_separator("/a\u{2028}b")]
    #[case::paragraph_separator("/a\u{2029}b")]
    fn cd_command_rejects_unusable_cwd(
        #[case] cwd: &str,
        #[values(
            Shell::Sh,
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::Nu,
            Shell::Xonsh,
            Shell::Powershell
        )]
        shell: Shell,
    ) {
        assert_eq!(cd_command(&on_host(cwd), &shell), None);
    }

    #[rstest]
    fn cd_command_rejects_unknown_shell() {
        assert_eq!(cd_command(&on_host("/tmp"), &Shell::Unknown), None);
    }

    #[cfg(not(windows))]
    #[rstest]
    fn cd_command_rejects_backslash_in_powershell() {
        assert_eq!(cd_command(r"/a\b", &Shell::Powershell), None);
    }

    proptest::proptest! {
        #[rstest]
        fn cd_command_posix_round_trips(path in "/[^\\p{Cc}\\x{2028}\\x{2029}]*") {
            let path = on_host(&path);
            let command = cd_command(&path, &Shell::Bash).unwrap();
            proptest::prop_assert_eq!(shlex::split(&command), Some(vec!["cd".into(), "--".into(), path]));
        }
    }

    /// Runs every generated `cd` in the real shell and checks where it lands. CI installs bash, zsh
    /// and fish; the other shells are skipped when missing.
    #[cfg(unix)]
    #[rstest]
    #[case::sh(Shell::Sh, "sh", &["-c"], "pwd")]
    #[case::bash(Shell::Bash, "bash", &["--norc", "-c"], "pwd")]
    #[case::zsh(Shell::Zsh, "zsh", &["-f", "-c"], "pwd")]
    #[case::fish(Shell::Fish, "fish", &["--no-config", "-c"], "pwd")]
    #[case::nu(Shell::Nu, "nu", &["-n", "-c"], "print $env.PWD")]
    #[case::xonsh(Shell::Xonsh, "xonsh", &["--no-rc", "-c"], "print($PWD)")]
    #[case::powershell(Shell::Powershell, "pwsh", &["-NoProfile", "-Command"], "(Get-Location).ProviderPath")]
    fn cd_command_lands_in_directory_in_real_shell(
        #[case] shell: Shell,
        #[case] program: &str,
        #[case] args: &[&str],
        #[case] print_cwd: &str,
    ) {
        use std::fmt::Write as _;

        const NAMES: &[&str] = &[
            r#"a b'c"d\e!$HOME"#,
            "q'#x'##y",
            "x'# y",
            "ends'#",
            "`x` $(y) @(z) {a,b} *? [1]",
            "s \u{2018}x\u{2019} \u{201a}y\u{201b}",
            "-lead",
            "trail ",
            "semi;amp&pipe|lt<gt>",
            "~ tilde",
            "back\\",
            "\u{e9} \u{4e2d}",
        ];
        let required = std::env::var_os("ATUIN_E2E_REQUIRE_SHELLS").is_some()
            && matches!(shell, Shell::Bash | Shell::Zsh | Shell::Fish);
        let root = tempfile::tempdir().unwrap();
        let mut script = String::new();
        let mut expected = Vec::new();
        for name in NAMES {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            let Some(cd) = cd_command(dir.to_str().unwrap(), &shell) else {
                continue;
            };
            writeln!(script, "{cd}\n{print_cwd}").unwrap();
            expected.push(dir.canonicalize().unwrap());
        }
        let skipped = NAMES.iter().filter(|n| shell == Shell::Powershell && n.contains('\\'));
        assert_eq!(expected.len(), NAMES.len() - skipped.count(), "unexpected skips");
        // Same override as the e2e shell setups, e.g. Homebrew bash on macOS.
        let program = std::env::var(format!("ATUIN_E2E_{}", program.to_uppercase()))
            .unwrap_or_else(|_| program.to_owned());
        let output = match std::process::Command::new(&program)
            .args(args)
            .arg(&script)
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(output) => output,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => {
                eprintln!("skipping: {program} not installed");
                return;
            }
            Err(e) => panic!("{program}: {e}"),
        };
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            output.status.success(),
            "{program} failed:\n{script}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let landed: Vec<_> = stdout.lines().map(|l| std::fs::canonicalize(l).unwrap()).collect();
        assert_eq!(landed, expected, "{program} script:\n{script}");
    }

    #[rstest]
    fn execute_accept_nth(#[with(KeymapMode::Emacs, 100, 5)] mut state: State, settings: Settings) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::AcceptNth(3), &settings);
        assert!(matches!(result, super::InputAction::Accept(8)));
    }

    #[rstest]
    fn execute_toggle_tab(#[with(KeymapMode::Emacs, 100, 0)] mut state: State, settings: Settings) {
        use crate::command::client::search::keybindings::Action;

        assert_eq!(state.tab_index, 0);
        let _ = state.execute_action(&Action::ToggleTab, &settings);
        assert_eq!(state.tab_index, 1);
        let _ = state.execute_action(&Action::ToggleTab, &settings);
        assert_eq!(state.tab_index, 0);
    }

    #[rstest]
    fn execute_enter_prefix_mode(
        #[with(KeymapMode::Emacs, 100, 0)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        assert!(!state.prefix);
        let _ = state.execute_action(&Action::EnterPrefixMode, &settings);
        assert!(state.prefix);
    }

    #[rstest]
    fn prefix_chord_ctrl_a_c_switches_context(
        #[with(KeymapMode::Emacs, 100, 7)] mut state: State,
        settings: Settings,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let result = state.handle_key_input(&settings, &ctrl_a);
        assert!(matches!(result, super::InputAction::Continue));
        assert!(state.prefix, "ctrl-a should enter prefix mode");

        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &c);
        assert!(
            matches!(result, super::InputAction::SwitchContext(Some(7))),
            "prefix + c should switch context"
        );
        assert_eq!(state.search.input.as_str(), "", "c should not be inserted");
    }

    #[rstest]
    fn inspector_prefix_chord_switches_context(
        #[with(KeymapMode::Emacs, 100, 7)] mut state: State,
        settings: Settings,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        state.tab_index = 1;

        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        let _ = state.handle_key_input(&settings, &ctrl_a);
        assert!(state.prefix, "ctrl-a should enter prefix mode in inspector");

        let c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &c);
        assert!(
            matches!(result, super::InputAction::SwitchContext(Some(7))),
            "prefix + c should switch context in inspector"
        );
    }

    #[rstest]
    fn inspector_unmatched_key_does_not_edit_search_input(
        #[with(KeymapMode::Emacs, 100, 7)] mut state: State,
        settings: Settings,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        state.tab_index = 1;

        let x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &x);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(
            state.search.input.as_str(),
            "",
            "unmatched keys in the inspector must not leak into the search input"
        );
    }

    #[rstest]
    fn execute_exit_returns_based_on_exit_mode(
        #[with(KeymapMode::Emacs, 100, 0)] mut state: State,
        mut settings: Settings,
    ) {
        use atuin_client::settings::ExitMode;

        use crate::command::client::search::keybindings::Action;

        settings.exit_mode = ExitMode::ReturnOriginal;
        let result = state.execute_action(&Action::Exit, &settings);
        assert!(matches!(result, super::InputAction::ReturnOriginal));

        settings.exit_mode = ExitMode::ReturnQuery;
        let result = state.execute_action(&Action::Exit, &settings);
        assert!(matches!(result, super::InputAction::ReturnQuery));
    }

    #[rstest]
    fn execute_return_original(
        #[with(KeymapMode::Emacs, 100, 0)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::ReturnOriginal, &settings);
        assert!(matches!(result, super::InputAction::ReturnOriginal));
    }

    #[rstest]
    fn execute_copy(#[with(KeymapMode::Emacs, 100, 7)] mut state: State, settings: Settings) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::Copy, &settings);
        assert!(matches!(result, super::InputAction::Copy(7)));
    }

    #[rstest]
    fn execute_delete(#[with(KeymapMode::Emacs, 100, 7)] mut state: State, settings: Settings) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::Delete, &settings);
        assert!(matches!(result, super::InputAction::Delete(7)));
    }

    #[rstest]
    fn execute_switch_context(
        #[with(KeymapMode::Emacs, 100, 7)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::SwitchContext, &settings);
        assert!(matches!(result, super::InputAction::SwitchContext(Some(7))));
    }

    #[rstest]
    fn execute_clear_context(
        #[with(KeymapMode::Emacs, 100, 7)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::ClearContext, &settings);
        assert!(matches!(result, super::InputAction::SwitchContext(None)));
    }

    #[rstest]
    fn execute_noop(#[with(KeymapMode::Emacs, 100, 50)] mut state: State, settings: Settings) {
        use crate::command::client::search::keybindings::Action;

        let result = state.execute_action(&Action::Noop, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.results_state.selected(), 50);
    }

    #[rstest]
    fn execute_accept_in_inspector_tab(
        #[with(KeymapMode::Emacs, 100, 5)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        state.tab_index = 1;
        let result = state.execute_action(&Action::Accept, &settings);
        assert!(matches!(result, super::InputAction::AcceptInspecting));
        assert!(state.accept);
    }

    #[rstest]
    fn inspector_enter_opens_output_and_escape_goes_back(
        #[with(KeymapMode::Emacs, 100, 5)] mut state: State,
        mut settings: Settings,
    ) {
        use ratatui::crossterm::event;

        use crate::command::client::search::keybindings::Action;
        settings.enter_accept = true;
        state.keymaps = KeymapSet::from_settings(&settings);
        state.tab_index = 1;
        state.inspecting_state.browser.select_view(super::InspectorView::Session);
        let event = event::Event::Key(event::KeyEvent::new(
            event::KeyCode::Enter,
            event::KeyModifiers::NONE,
        ));
        assert!(matches!(state.handle_input(&settings, &event), InputAction::Redraw));
        assert_eq!(state.inspecting_state.browser.view, super::InspectorView::Output);
        assert!(!state.accept, "Enter in inspector must not execute the command");
        let _ = state.execute_action(&Action::ScrollPageDown, &settings);
        assert_eq!(state.results_state.selected(), 5);
        let _ = state.execute_action(&Action::Exit, &settings);
        assert_eq!(state.inspecting_state.browser.view, super::InspectorView::Session);
        assert_eq!(state.tab_index, 1);
        let _ = state.execute_action(&Action::Exit, &settings);
        assert_eq!(state.tab_index, 0);
    }

    #[fixture]
    async fn inspector_corpus(
        #[default(3usize)] count: usize,
        #[default(false)] tied: bool,
    ) -> (atuin_client::database::Sqlite, Vec<History>) {
        let db = atuin_client::database::Sqlite::in_memory(std::time::Duration::from_secs(2))
            .await
            .unwrap();
        let mut entries = Vec::new();
        for i in 0..count {
            let mut entry: History = History::capture()
                .timestamp(
                    OffsetDateTime::UNIX_EPOCH
                        + time::Duration::seconds(if tied {
                            0
                        } else {
                            i64::try_from(i).unwrap()
                        }),
                )
                .command("echo FIRST")
                .cwd("/tmp")
                .build()
                .into();
            entry.session = "inspector-test".into();
            entry.cwd = format!("/tmp/{i}");
            db.save(&entry).await.unwrap();
            entries.push(entry);
        }
        entries.sort_by_key(|entry| (entry.timestamp, entry.id.to_string()));
        (db, entries)
    }

    async fn refresh_inspector(
        state: &mut State,
        db: &atuin_client::database::Sqlite,
        settings: &Settings,
    ) {
        if matches!(
            state.inspecting_state.browser.view,
            InspectorView::Output | InspectorView::Stats
        ) {
            state.inspecting_state.previous = None;
            state.inspecting_state.next = None;
            return;
        }
        let selected = db.load(state.inspecting_state.current.unwrap()).await.unwrap().unwrap();
        (state.inspecting_state.previous, state.inspecting_state.next) =
            state.inspecting_state.browser.refresh(db, &selected, settings).await.unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn context_refresh_preserves_an_occurrence_missing_from_search(
        mut state: State,
        settings: Settings,
        #[future] inspector_corpus: (atuin_client::database::Sqlite, Vec<History>),
    ) {
        let (mut db, entries) = inspector_corpus.await;
        let mut third = entries[2].clone();
        third.id = atuin_client::history::HistoryId::new(atuin_common::utils::uuid_v7());
        third.command = "echo THIRD".into();
        third.timestamp += time::Duration::seconds(1);
        db.save(&third).await.unwrap();
        state.tab_index = 1;
        state.inspecting_state.current = Some(entries[0].id);
        state.inspecting_state.browser.select_view(super::InspectorView::Session);
        state.search.custom_context = Some(entries[0].id);
        state.search.context = Context::from_history(&entries[0]);
        state.search.filter_mode = FilterMode::Session;
        let results = state.query_results(&mut db, &settings).await.unwrap();
        assert!(
            !results.iter().any(|entry| entry.id == entries[0].id),
            "older occurrence should be deduplicated"
        );
        refresh_inspector(&mut state, &db, &settings).await;
        assert_eq!(state.inspecting_state.current, Some(entries[0].id));
        assert_eq!(state.inspecting_state.browser.view, super::InspectorView::Session);
        assert!(matches!(
            state.execute_action(&Action::ReturnSelection, &settings),
            InputAction::AcceptInspecting
        ));
        // Even a filter with no search results must not replace the inspected occurrence.
        state.search.input = "not in this history".to_owned().into();
        assert!(state.query_results(&mut db, &settings).await.unwrap().is_empty());
        assert_eq!(state.inspecting_state.current, Some(entries[0].id));
        // Ordinary searches still reset the inspector.
        state.tab_index = 0;
        state.query_results(&mut db, &settings).await.unwrap();
        assert!(state.inspecting_state.current.is_none());
    }

    #[rstest]
    #[case(InspectorView::Runs)]
    #[case(InspectorView::Session)]
    #[tokio::test]
    async fn conditional_navigation_crosses_windows_in_both_directions(
        #[with(KeymapMode::Emacs, 1, 0)] mut state: State,
        settings: Settings,
        #[case] view: super::InspectorView,
        #[with(250, true)]
        #[future]
        inspector_corpus: (atuin_client::database::Sqlite, Vec<History>),
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::command::client::search::keybindings::{ConditionExpr, KeyInput, KeyRule};
        let (db, entries) = inspector_corpus.await;
        state.tab_index = 1;
        state.inspecting_state.browser.select_view(view);
        state.inspecting_state.current = Some(entries.last().unwrap().id);
        for (key, condition, action) in [
            ("down", "has-results && !list-at-end", Action::SelectNext),
            ("up", "!list-at-start", Action::SelectPrevious),
        ] {
            state.keymaps.inspector.bind_conditional(KeyInput::parse(key).unwrap(), vec![
                KeyRule::when(ConditionExpr::parse(condition).unwrap(), action),
            ]);
        }
        refresh_inspector(&mut state, &db, &settings).await;
        for (key, indices) in [
            (KeyCode::Down, (0..entries.len()).rev().collect::<Vec<_>>()),
            (KeyCode::Up, (0..entries.len()).collect()),
        ] {
            for index in indices {
                assert_eq!(state.inspecting_state.current, Some(entries[index].id));
                let _ = state.handle_key_input(&settings, &KeyEvent::new(key, KeyModifiers::NONE));
                refresh_inspector(&mut state, &db, &settings).await;
            }
        }
        assert_eq!(state.inspecting_state.current, Some(entries.last().unwrap().id));
        assert_eq!(state.results_state.selected(), 0);
        assert_eq!(state.inspecting_state.browser.view, view);
    }

    #[rstest]
    #[case(InspectorView::Runs)]
    #[case(InspectorView::Session)]
    #[case(InspectorView::Output)]
    #[case(InspectorView::Stats)]
    #[tokio::test]
    async fn mouse_navigation_stays_in_the_inspector(
        #[with(KeymapMode::Emacs, 100, 5)] mut state: State,
        mut settings: Settings,
        #[case] view: super::InspectorView,
        #[values(false, true)] invert: bool,
        #[future] inspector_corpus: (atuin_client::database::Sqlite, Vec<History>),
    ) {
        use ratatui::crossterm::event::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let (db, entries) = inspector_corpus.await;
        settings.invert = invert;
        state.tab_index = 1;
        state.inspecting_state.current = Some(entries[1].id);
        state.inspecting_state.browser.select_view(view);
        refresh_inspector(&mut state, &db, &settings).await;
        for (kind, expected) in
            [(MouseEventKind::ScrollDown, entries[0].id), (MouseEventKind::ScrollUp, entries[1].id)]
        {
            let event = Event::Mouse(MouseEvent {
                kind,
                column: 5,
                row: 10,
                modifiers: KeyModifiers::NONE,
            });
            assert!(matches!(state.handle_input(&settings, &event), InputAction::Redraw));
            refresh_inspector(&mut state, &db, &settings).await;
            let expected = if matches!(view, InspectorView::Output | InspectorView::Stats) {
                entries[1].id
            } else {
                expected
            };
            assert_eq!(state.inspecting_state.current, Some(expected));
            assert_eq!(state.results_state.selected(), 5);
            assert_eq!(state.inspecting_state.browser.view, view);
        }
        let _ = state.execute_action(&Action::ScrollToScreenTop, &settings);
        assert_eq!(state.inspecting_state.current, Some(entries[1].id));
        assert_eq!(state.results_state.selected(), 5);
        let input = state.search.input.as_str().to_owned();
        let _ = state.handle_input(&settings, &Event::Paste("unwanted query".into()));
        assert_eq!(state.search.input.as_str(), input);
    }

    #[rstest]
    fn inspector_delete_targets_the_inspected_occurrence(
        #[with(KeymapMode::Emacs, 100, 5)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;
        state.tab_index = 1;
        assert!(matches!(
            state.execute_action(&Action::Delete, &settings),
            super::InputAction::DeleteInspecting
        ));
    }

    #[rstest]
    fn execute_cycle_search_mode(
        #[with(KeymapMode::Emacs, 100, 0)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        let original_mode = state.search_mode();
        let result = state.execute_action(&Action::CycleSearchMode, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        assert!(state.switched_search_mode);
        assert_ne!(state.search_mode(), original_mode);
    }

    #[cfg(all(feature = "daemon", unix))]
    #[rstest]
    #[tokio::test]
    async fn unavailable_daemon_fuzzy_retries_with_local_fuzzy() {
        use atuin_client::database::Sqlite;

        let temp = tempfile::tempdir().unwrap();
        let mut settings = Settings::utc();
        settings.requested_search_mode = RequestedSearchMode::DaemonFuzzy;
        settings.daemon.enabled = true;
        settings.daemon.autostart = true;
        settings.daemon.systemd_socket = true;
        settings.daemon.socket_path = Some(temp.path().join("missing.sock"));

        let mut state = state(KeymapMode::Emacs, 0, 0, FilterMode::Global, "query");
        state.search_mode_state = SearchModeState::new(&settings);
        assert_eq!(state.search_mode(), SearchMode::DaemonFuzzy);
        state.engine = engines::engine(SearchMode::DaemonFuzzy, &settings);
        let mut db = Sqlite::in_memory(std::time::Duration::from_secs(2)).await.unwrap();
        let history: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command("echo query match")
            .cwd("/tmp")
            .build()
            .into();
        db.save(&history).await.unwrap();

        let results = state.query_results(&mut db, &settings).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].command, "echo query match");
        assert_eq!(state.search_mode(), SearchMode::Fuzzy);
        assert_eq!(state.search_mode_state.raw_mode(), SearchMode::DaemonFuzzy);
        assert!(state.search_mode_state.daemon_failed);
        assert!(state.search_mode_state.is_failed_daemon_fuzzy());

        state.search_mode_state.mode = SearchMode::FullText;
        let _ = state.execute_action(&Action::CycleSearchMode, &settings);
        assert_eq!(state.search_mode_state.raw_mode(), SearchMode::DaemonFuzzy);
        assert_eq!(state.search_mode(), SearchMode::Fuzzy);
    }

    #[rstest]
    fn execute_vim_search_insert(
        #[with(KeymapMode::Emacs, 100, 0)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        state.search.input.insert('h');
        state.search.input.insert('i');
        state.keymap_mode = KeymapMode::VimNormal;
        let result = state.execute_action(&Action::VimSearchInsert, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Should clear input and switch to insert mode
        assert_eq!(state.search.input.as_str(), "");
        assert_eq!(state.keymap_mode, KeymapMode::VimInsert);
    }

    #[rstest]
    fn execute_cursor_movement(
        #[with(KeymapMode::Emacs, 100, 0)] mut state: State,
        settings: Settings,
    ) {
        use crate::command::client::search::keybindings::Action;

        // Insert some text
        state.search.input.insert('h');
        state.search.input.insert('e');
        state.search.input.insert('l');
        state.search.input.insert('l');
        state.search.input.insert('o');
        // cursor is at end (position 5)

        // CursorLeft
        let _ = state.execute_action(&Action::CursorLeft, &settings);
        assert_eq!(state.search.input.position(), 4);

        // CursorStart
        let _ = state.execute_action(&Action::CursorStart, &settings);
        assert_eq!(state.search.input.position(), 0);

        // CursorEnd
        let _ = state.execute_action(&Action::CursorEnd, &settings);
        assert_eq!(state.search.input.position(), 5);

        // CursorRight at end does nothing
        let _ = state.execute_action(&Action::CursorRight, &settings);
        assert_eq!(state.search.input.position(), 5);
    }

    #[rstest]
    fn execute_editing(#[with(KeymapMode::Emacs, 100, 0)] mut state: State, settings: Settings) {
        use crate::command::client::search::keybindings::Action;

        // Insert "hello"
        state.search.input.insert('h');
        state.search.input.insert('e');
        state.search.input.insert('l');
        state.search.input.insert('l');
        state.search.input.insert('o');

        // DeleteCharBefore (backspace)
        let _ = state.execute_action(&Action::DeleteCharBefore, &settings);
        assert_eq!(state.search.input.as_str(), "hell");

        // ClearLine
        let _ = state.execute_action(&Action::ClearLine, &settings);
        assert_eq!(state.search.input.as_str(), "");
    }

    #[rstest]
    fn keymap_config_return_query(
        #[with(KeymapMode::Emacs, 100, 0, FilterMode::Global, "test query")] mut state: State,
        mut settings: Settings,
    ) {
        use std::collections::HashMap;

        use atuin_client::settings::KeyBindingConfig;
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        // Configure tab to return-query
        settings.keymap.emacs = HashMap::from([(
            "tab".to_string(),
            KeyBindingConfig::Simple("return-query".to_string()),
        )]);
        state.keymaps = KeymapSet::from_settings(&settings);

        let tab_event = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &tab_event);
        assert!(
            matches!(result, super::InputAction::ReturnQuery),
            "Tab configured as return-query should return InputAction::ReturnQuery"
        );
    }
}
