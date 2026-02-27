use std::{
    io::{Write, stdout},
    time::Duration,
};

use atuin_common::{shell::Shell, utils::Escapable as _};
use eyre::Result;
use futures_util::FutureExt;
use semver::Version;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use super::{
    cursor::Cursor,
    engines::{SearchEngine, SearchState},
    history_list::{HistoryList, ListState},
};
use atuin_client::{
    database::{Context, Database, current_context},
    history::{History, HistoryId, HistoryStats, store::HistoryStore},
    settings::{
        CursorStyle, ExitMode, FilterMode, KeymapMode, PreviewStrategy, SearchMode, Settings,
        UiColumn,
    },
};

use crate::command::client::search::history_list::HistoryHighlighter;
use crate::command::client::search::keybindings::KeymapSet;
use crate::command::client::theme::{Meaning, Theme};
use crate::{VERSION, command::client::search::engines};

use ratatui::{
    Frame, Terminal, TerminalOptions, Viewport,
    backend::{CrosstermBackend, FromCrossterm},
    crossterm::{
        cursor::SetCursorStyle,
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEvent, MouseEvent},
        execute, terminal,
    },
    layout::{Alignment, Constraint, Direction, Layout},
    prelude::*,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Padding, Paragraph, Tabs},
};

#[cfg(not(target_os = "windows"))]
use ratatui::crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};

const TAB_TITLES: [&str; 2] = ["Search", "Inspect"];

pub enum InputAction {
    Accept(usize),
    AcceptInspecting,
    Copy(usize),
    Delete(usize),
    ReturnOriginal,
    ReturnQuery,
    Continue,
    Redraw,
    SwitchContext(Option<usize>),
}

#[derive(Clone)]
pub struct InspectingState {
    current: Option<HistoryId>,
    next: Option<HistoryId>,
    previous: Option<HistoryId>,
}

impl InspectingState {
    pub fn move_to_previous(&mut self) {
        let previous = self.previous.clone();
        self.reset();
        self.current = previous;
    }

    pub fn move_to_next(&mut self) {
        let next = self.next.clone();
        self.reset();
        self.current = next;
    }

    pub fn reset(&mut self) {
        self.current = None;
        self.next = None;
        self.previous = None;
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

#[allow(clippy::struct_field_names)]
#[allow(clippy::struct_excessive_bools)]
pub struct State {
    history_count: i64,
    update_needed: Option<Version>,
    results_state: ListState,
    switched_search_mode: bool,
    search_mode: SearchMode,
    results_len: usize,
    accept: bool,
    keymap_mode: KeymapMode,
    prefix: bool,
    current_cursor: Option<CursorStyle>,
    tab_index: usize,
    pending_vim_key: Option<char>,
    original_input_empty: bool,

    pub inspecting_state: InspectingState,

    keymaps: KeymapSet,
    search: SearchState,
    engine: Box<dyn SearchEngine>,
    now: Box<dyn Fn() -> OffsetDateTime + Send>,
}

#[derive(Clone, Copy)]
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
    async fn query_results(
        &mut self,
        db: &mut dyn Database,
        smart_sort: bool,
    ) -> Result<Vec<History>> {
        let results = self.engine.query(&self.search, db).await?;

        self.inspecting_state = InspectingState {
            current: None,
            next: None,
            previous: None,
        };
        self.results_state.select(0);
        self.results_len = results.len();

        if smart_sort {
            Ok(atuin_history::sort::sort(
                self.search.input.as_str(),
                results,
            ))
        } else {
            Ok(results)
        }
    }

    fn handle_input<W>(
        &mut self,
        settings: &Settings,
        input: &Event,
        w: &mut W,
    ) -> Result<InputAction>
    where
        W: Write,
    {
        execute!(w, EnableMouseCapture)?;
        let r = match input {
            Event::Key(k) => self.handle_key_input(settings, k),
            Event::Mouse(m) => self.handle_mouse_input(*m),
            Event::Paste(d) => self.handle_paste_input(d),
            _ => InputAction::Continue,
        };
        execute!(w, DisableMouseCapture)?;
        Ok(r)
    }

    fn handle_mouse_input(&mut self, input: MouseEvent) -> InputAction {
        match input.kind {
            event::MouseEventKind::ScrollDown => {
                self.scroll_down(1);
            }
            event::MouseEventKind::ScrollUp => {
                self.scroll_up(1);
            }
            _ => {}
        }
        InputAction::Continue
    }

    fn handle_paste_input(&mut self, input: &str) -> InputAction {
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
    fn is_insert_mode(&self) -> bool {
        matches!(
            self.keymap_mode,
            KeymapMode::Emacs | KeymapMode::Auto | KeymapMode::VimInsert
        )
    }

    fn handle_key_input(&mut self, settings: &Settings, input: &KeyEvent) -> InputAction {
        use super::keybindings::Action;
        use super::keybindings::EvalContext;
        use super::keybindings::key::{KeyCodeValue, KeyInput, SingleKey};

        // Skip release events
        if input.kind == event::KeyEventKind::Release {
            return InputAction::Continue;
        }

        // Reset switched_search_mode at start of each key event
        self.switched_search_mode = false;

        // Build evaluation context from current state
        let ctx = EvalContext {
            cursor_position: self.search.input.position(),
            input_width: UnicodeWidthStr::width(self.search.input.as_str()),
            input_byte_len: self.search.input.as_str().len(),
            selected_index: self.results_state.selected(),
            results_len: self.results_len,
            original_input_empty: self.original_input_empty,
            has_context: self.search.custom_context.is_some(),
        };

        // Convert KeyEvent to SingleKey
        let Some(single) = SingleKey::from_event(input) else {
            return InputAction::Continue;
        };

        // --- Phase 1: Resolve (take pending key first, then immutable borrows) ---

        // Take pending key before any immutable borrows of self
        let pending = self.pending_vim_key.take();

        // If in prefix mode, try prefix keymap first (single keys only)
        let prefix_action = if self.prefix {
            let ki = KeyInput::Single(single.clone());
            self.keymaps.prefix.resolve(&ki, &ctx)
        } else {
            None
        };

        // The if-let/else-if chain here is clearer than map_or_else with nested closures.
        #[allow(clippy::option_if_let_else)]
        let (action, new_pending) = if prefix_action.is_some() {
            (prefix_action, None)
        } else {
            // Use mode keymap (handles both single and multi-key sequences)
            let keymap = self.mode_keymap();

            if let Some(pending_char) = pending {
                // We have a pending key from a previous press (e.g., first 'g' of 'gg')
                let pending_single = SingleKey {
                    code: KeyCodeValue::Char(pending_char),
                    ctrl: false,
                    alt: false,
                    shift: false,
                    super_key: false,
                };
                let seq = KeyInput::Sequence(vec![pending_single, single.clone()]);
                let action = keymap
                    .resolve(&seq, &ctx)
                    .or_else(|| keymap.resolve(&KeyInput::Single(single.clone()), &ctx));
                (action, None)
            } else if keymap.has_sequence_starting_with(&single)
                && matches!(single.code, KeyCodeValue::Char(_))
                && !single.ctrl
                && !single.alt
            {
                // This key starts a multi-key sequence; wait for next key
                let KeyCodeValue::Char(c) = single.code else {
                    unreachable!()
                };
                (Some(Action::Noop), Some(c))
            } else {
                (
                    keymap.resolve(&KeyInput::Single(single.clone()), &ctx),
                    None,
                )
            }
        };

        // --- Phase 2: Apply mutations ---
        self.pending_vim_key = new_pending;

        // Reset prefix (before execute, so EnterPrefixMode can re-set it)
        self.prefix = false;

        if let Some(action) = action {
            self.execute_action(&action, settings)
        } else {
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
    }

    fn scroll_down(&mut self, scroll_len: usize) {
        let i = self.results_state.selected().saturating_sub(scroll_len);
        self.inspecting_state.reset();
        self.results_state.select(i);
    }

    fn scroll_up(&mut self, scroll_len: usize) {
        let i = self.results_state.selected() + scroll_len;
        self.results_state
            .select(i.min(self.results_len.saturating_sub(1)));
        self.inspecting_state.reset();
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
    pub(crate) fn execute_action(
        &mut self,
        action: &super::keybindings::Action,
        settings: &Settings,
    ) -> InputAction {
        use crate::command::client::search::keybindings::Action;

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
                self.search
                    .input
                    .prev_word(&settings.word_chars, settings.word_jump_mode);
                InputAction::Continue
            }
            Action::CursorWordRight => {
                self.search
                    .input
                    .next_word(&settings.word_chars, settings.word_jump_mode);
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
                self.search
                    .input
                    .remove_prev_word(&settings.word_chars, settings.word_jump_mode);
                InputAction::Continue
            }
            Action::DeleteWordAfter => {
                self.search
                    .input
                    .remove_next_word(&settings.word_chars, settings.word_jump_mode);
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
                let scroll_len = self
                    .results_state
                    .max_entries()
                    .saturating_sub(settings.scroll_context_lines)
                    / 2;
                if settings.invert {
                    self.scroll_down(scroll_len);
                } else {
                    self.scroll_up(scroll_len);
                }
                InputAction::Continue
            }
            Action::ScrollHalfPageDown => {
                let scroll_len = self
                    .results_state
                    .max_entries()
                    .saturating_sub(settings.scroll_context_lines)
                    / 2;
                if settings.invert {
                    self.scroll_up(scroll_len);
                } else {
                    self.scroll_down(scroll_len);
                }
                InputAction::Continue
            }
            Action::ScrollPageUp => {
                let scroll_len = self
                    .results_state
                    .max_entries()
                    .saturating_sub(settings.scroll_context_lines);
                if settings.invert {
                    self.scroll_down(scroll_len);
                } else {
                    self.scroll_up(scroll_len);
                }
                InputAction::Continue
            }
            Action::ScrollPageDown => {
                let scroll_len = self
                    .results_state
                    .max_entries()
                    .saturating_sub(settings.scroll_context_lines);
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
                self.results_state
                    .select(bottom.min(self.results_len.saturating_sub(1)));
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToScreenMiddle => {
                // M — jump to middle of visible screen
                let top = self.results_state.offset();
                let visible = self.results_state.max_entries().min(self.results_len);
                let middle = top + visible / 2;
                self.results_state
                    .select(middle.min(self.results_len.saturating_sub(1)));
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
                if self.tab_index == 1 {
                    return InputAction::AcceptInspecting;
                }
                self.accept = true;
                InputAction::Accept(self.results_state.selected())
            }
            Action::AcceptNth(n) => {
                self.accept = true;
                InputAction::Accept(self.results_state.selected() + *n as usize)
            }
            Action::ReturnSelection => {
                if self.tab_index == 1 {
                    return InputAction::AcceptInspecting;
                }
                InputAction::Accept(self.results_state.selected())
            }
            Action::ReturnSelectionNth(n) => {
                InputAction::Accept(self.results_state.selected() + *n as usize)
            }
            Action::Copy => InputAction::Copy(self.results_state.selected()),
            Action::Delete => InputAction::Delete(self.results_state.selected()),
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
                self.search_mode = self.search_mode.next(settings);
                self.engine = engines::engine(self.search_mode, settings);
                InputAction::Continue
            }
            Action::SwitchContext => {
                InputAction::SwitchContext(Some(self.results_state.selected()))
            }
            Action::ClearContext => InputAction::SwitchContext(None),
            Action::ToggleTab => {
                self.tab_index = (self.tab_index + 1) % TAB_TITLES.len();
                InputAction::Continue
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

            // -- Special --
            Action::Noop => InputAction::Continue,
        }
    }

    #[allow(clippy::cast_possible_truncation)]
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
            let length_current_cmd = results[selected].command.len() as u16;
            // calculate the number of newlines in the command
            let num_newlines = results[selected]
                .command
                .chars()
                .filter(|&c| c == '\n')
                .count() as u16;
            if num_newlines > 0 {
                std::cmp::min(
                    settings.max_preview_height,
                    results[selected]
                        .command
                        .split('\n')
                        .map(|line| {
                            (line.len() as u16 + preview_width - 1 - border_size)
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
            let longest_command = results
                .iter()
                .max_by(|h1, h2| h1.command.len().cmp(&h2.command.len()));
            longest_command.map_or(0, |v| {
                std::cmp::min(
                    settings.max_preview_height,
                    v.command
                        .split('\n')
                        .map(|line| {
                            (line.len() as u16 + preview_width - 1 - border_size)
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
    fn draw(
        &mut self,
        f: &mut Frame,
        results: &[History],
        stats: Option<HistoryStats>,
        inspecting: Option<&History>,
        settings: &Settings,
        theme: &Theme,
    ) {
        let compactness = to_compactness(f, settings);
        let invert = settings.invert;
        let border_size = match compactness {
            Compactness::Full => 1,
            _ => 0,
        };
        let preview_width = f.area().width - 2;
        let preview_height = Self::calc_preview_height(
            settings,
            results,
            self.results_state.selected(),
            self.tab_index,
            compactness,
            border_size,
            preview_width,
        );
        let show_help =
            settings.show_help && (matches!(compactness, Compactness::Full) || f.area().height > 1);
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
                        Constraint::Length(1 + border_size),               // input
                        Constraint::Min(1),                                // results list
                        Constraint::Length(preview_height),                // preview
                        Constraint::Length(if show_tabs { 1 } else { 0 }), // tabs
                        Constraint::Length(if show_help { 1 } else { 0 }), // header (sic)
                    ]
                } else {
                    match compactness {
                        Compactness::Ultracompact => [
                            Constraint::Length(if show_help { 1 } else { 0 }), // header
                            Constraint::Length(0),                             // tabs
                            Constraint::Min(1),                                // results list
                            Constraint::Length(0),
                            Constraint::Length(0),
                        ],
                        _ => [
                            Constraint::Length(if show_help { 1 } else { 0 }), // header
                            Constraint::Length(if show_tabs { 1 } else { 0 }), // tabs
                            Constraint::Min(1),                                // results list
                            Constraint::Length(1 + border_size),               // input
                            Constraint::Length(preview_height),                // preview
                        ],
                    }
                }
                .as_ref(),
            )
            .split(f.area());

        let input_chunk = if invert { chunks[0] } else { chunks[3] };
        let results_list_chunk = if invert { chunks[1] } else { chunks[2] };
        let preview_chunk = if invert { chunks[2] } else { chunks[4] };
        let tabs_chunk = if invert { chunks[3] } else { chunks[1] };
        let header_chunk = if invert { chunks[4] } else { chunks[0] };

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
                [
                    Constraint::Ratio(1, 5),
                    Constraint::Ratio(3, 5),
                    Constraint::Ratio(1, 5),
                ]
                .as_ref(),
            )
            .split(header_chunk);

        let title = self.build_title(theme);
        f.render_widget(title, header_chunks[0]);

        let help = self.build_help(settings, theme);
        f.render_widget(help, header_chunks[1]);

        let stats_tab = self.build_stats(theme);
        f.render_widget(stats_tab, header_chunks[2]);

        let indicator: String = match compactness {
            Compactness::Ultracompact => {
                if self.switched_search_mode {
                    format!("S{}>", self.search_mode.as_str().chars().next().unwrap())
                } else if self.search.custom_context.is_some() {
                    format!(
                        "C{}>",
                        self.search.filter_mode.as_str().chars().next().unwrap()
                    )
                } else {
                    format!(
                        "{}> ",
                        self.search.filter_mode.as_str().chars().next().unwrap()
                    )
                }
            }
            _ => " > ".to_string(),
        };

        match self.tab_index {
            0 => {
                let history_highlighter = HistoryHighlighter {
                    engine: self.engine.as_ref(),
                    search_input: self.search.input.as_str(),
                };
                let results_list = Self::build_results_list(
                    style,
                    results,
                    self.keymap_mode,
                    &self.now,
                    indicator.as_str(),
                    theme,
                    history_highlighter,
                    settings.show_numeric_shortcuts,
                    &settings.ui.columns,
                );
                f.render_stateful_widget(results_list, results_list_chunk, &mut self.results_state);
            }

            1 => {
                if results.is_empty() {
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
                    let inspecting = match inspecting {
                        Some(inspecting) => inspecting,
                        None => &results[self.results_state.selected()],
                    };
                    super::inspector::draw(
                        f,
                        results_list_chunk,
                        inspecting,
                        &stats.expect("Drawing inspector, but no stats"),
                        settings,
                        theme,
                        settings.timezone,
                    );
                }

                // HACK: I'm following up with abstracting this into the UI container, with a
                // sub-widget for search + for inspector
                let feedback = Paragraph::new(
                    "The inspector is new - please give feedback (good, or bad) at https://forum.atuin.sh",
                );
                f.render_widget(feedback, input_chunk);

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
            #[allow(clippy::cast_possible_truncation)]
            let prefix_width = settings
                .ui
                .columns
                .iter()
                .take_while(|col| !col.expand)
                .map(|col| col.width + 1)
                .sum::<u16>()
                + " > ".len() as u16;
            #[allow(clippy::cast_possible_truncation)]
            let min_prefix_width = "[ SRCH: FULLTXT ] ".len() as u16;
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

    #[allow(clippy::cast_possible_truncation, clippy::too_many_arguments)]
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
            input_chunk.x + extra_width as u16 + prefix_width + cursor_offset,
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

            1 => Paragraph::new(Text::from(Line::from(vec![
                Span::styled("<esc>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": exit"),
                Span::raw(", "),
                Span::styled("<ctrl-o>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": search"),
                Span::raw(", "),
                Span::styled("<ctrl-d>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": delete"),
            ]))),

            _ => unreachable!("invalid tab index"),
        }
        .style(Style::from_crossterm(theme.as_style(Meaning::Annotation)))
        .alignment(Alignment::Center)
    }

    fn build_stats(&self, theme: &Theme) -> Paragraph<'_> {
        Paragraph::new(Text::from(Span::raw(format!(
            "history count: {}",
            self.history_count,
        ))))
        .style(Style::from_crossterm(theme.as_style(Meaning::Annotation)))
        .alignment(Alignment::Right)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_results_list<'a>(
        style: StyleState,
        results: &'a [History],
        keymap_mode: KeymapMode,
        now: &'a dyn Fn() -> OffsetDateTime,
        indicator: &'a str,
        theme: &'a Theme,
        history_highlighter: HistoryHighlighter<'a>,
        show_numeric_shortcuts: bool,
        columns: &'a [UiColumn],
    ) -> HistoryList<'a> {
        let results_list = HistoryList::new(
            results,
            style.invert,
            keymap_mode == KeymapMode::VimNormal,
            now,
            indicator,
            theme,
            history_highlighter,
            show_numeric_shortcuts,
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
        let (pref, mode) = if self.switched_search_mode {
            (" SRCH:", self.search_mode.as_str())
        } else if self.search.custom_context.is_some() {
            (" CTX:", self.search.filter_mode.as_str())
        } else {
            ("", self.search.filter_mode.as_str())
        };
        // 3: surrounding "[" "] "
        let mode_width = usize::from(prefix_width) - pref.len() - 3;
        // sanity check to ensure we don't exceed the layout limits
        debug_assert!(mode_width >= mode.len(), "mode name '{mode}' is too long!");
        let input = format!("[{pref}{mode:^mode_width$}] {}", self.search.input.as_str(),);
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
            use itertools::Itertools as _;
            let s = &results[selected].command;
            s.split('\n')
                .flat_map(|line| {
                    line.char_indices()
                        .step_by(preview_width.into())
                        .map(|(i, _)| i)
                        .chain(Some(line.len()))
                        .tuple_windows()
                        .map(|(a, b)| (&line[a..b]).escape_control().to_string())
                })
                .join("\n")
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

struct Stdout {
    stdout: std::io::Stdout,
    inline_mode: bool,
}

impl Stdout {
    pub fn new(inline_mode: bool) -> std::io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = stdout();

        if !inline_mode {
            execute!(stdout, terminal::EnterAlternateScreen)?;
        }

        execute!(
            stdout,
            event::EnableMouseCapture,
            event::EnableBracketedPaste,
        )?;

        #[cfg(not(target_os = "windows"))]
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            ),
        )?;

        Ok(Self {
            stdout,
            inline_mode,
        })
    }
}

impl Drop for Stdout {
    fn drop(&mut self) {
        #[cfg(not(target_os = "windows"))]
        execute!(self.stdout, PopKeyboardEnhancementFlags).unwrap();

        if !self.inline_mode {
            execute!(self.stdout, terminal::LeaveAlternateScreen).unwrap();
        }
        execute!(
            self.stdout,
            event::DisableMouseCapture,
            event::DisableBracketedPaste,
        )
        .unwrap();

        terminal::disable_raw_mode().unwrap();
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stdout.flush()
    }
}

// this is a big blob of horrible! clean it up!
// for now, it works. But it'd be great if it were more easily readable, and
// modular. I'd like to add some more stats and stuff at some point
#[allow(
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::cognitive_complexity
)]
pub async fn history(
    query: &[String],
    settings: &Settings,
    mut db: impl Database,
    history_store: &HistoryStore,
    theme: &Theme,
) -> Result<String> {
    let inline_height = if settings.shell_up_key_binding {
        settings
            .inline_height_shell_up_key_binding
            .unwrap_or(settings.inline_height)
    } else {
        settings.inline_height
    };

    // Use fullscreen mode if the inline height doesn't fit in the terminal,
    // this will preserve the scroll position upon exit
    let inline_height = if let Ok(size) = terminal::size()
        && inline_height >= size.1
    {
        0
    } else {
        inline_height
    };

    let stdout = Stdout::new(inline_height > 0)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: if inline_height > 0 {
                Viewport::Inline(inline_height)
            } else {
                Viewport::Fullscreen
            },
        },
    )?;

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

    let initial_context = current_context().await?;

    let history_count = db.history_count(false).await?;
    let search_mode = if settings.shell_up_key_binding {
        settings
            .search_mode_shell_up_key_binding
            .unwrap_or(settings.search_mode)
    } else {
        settings.search_mode
    };
    let default_filter_mode = settings
        .filter_mode_shell_up_key_binding
        .filter(|_| settings.shell_up_key_binding)
        .unwrap_or_else(|| settings.default_filter_mode(initial_context.git_root.is_some()));
    let mut app = State {
        history_count,
        results_state: ListState::default(),
        update_needed: None,
        switched_search_mode: false,
        search_mode,
        tab_index: 0,
        inspecting_state: InspectingState {
            current: None,
            next: None,
            previous: None,
        },
        keymaps: KeymapSet::from_settings(settings),
        search: SearchState {
            input,
            filter_mode: default_filter_mode,
            context: initial_context.clone(),
            custom_context: None,
        },
        engine: engines::engine(search_mode, settings),
        results_len: 0,
        accept: false,
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
        original_input_empty: original_query.is_empty(),
    };

    app.initialize_keymap_cursor(settings);

    let mut results = app.query_results(&mut db, settings.smart_sort).await?;

    if inline_height > 0 {
        terminal.clear()?;
    }

    let mut stats: Option<HistoryStats> = None;
    let mut inspecting: Option<History> = None;
    let accept;
    let result = 'render: loop {
        terminal.draw(|f| {
            app.draw(
                f,
                &results,
                stats.clone(),
                inspecting.as_ref(),
                settings,
                theme,
            );
        })?;

        let initial_input = app.search.input.as_str().to_owned();
        let initial_filter_mode = app.search.filter_mode;
        let initial_search_mode = app.search_mode;
        let initial_custom_context = app.search.custom_context.clone();

        let event_ready = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(250)));

        tokio::select! {
            event_ready = event_ready => {
                if event_ready?? {
                    loop {
                        match app.handle_input(settings, &event::read()?, &mut std::io::stdout())? {
                            InputAction::Continue => {},
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

                                if settings.sync.records {
                                    let (id, _) = history_store.delete(entry.id).await?;
                                    history_store.incremental_build(&db, &[id]).await?;
                                } else {
                                    db.delete(entry.clone()).await?;
                                }

                                app.tab_index  = 0;
                            },
                            InputAction::SwitchContext(index) => {
                                if let Some(index) = index && let Some(entry) = results.get(index) {
                                    app.search.custom_context = Some(entry.id.clone());
                                    app.search.context = Context::from_history(entry);
                                    app.search.filter_mode = FilterMode::Session;
                                    app.search.input = Cursor::from(String::new());
                                    app.results_state = ListState::default();
                                } else {
                                    app.search.custom_context = None;
                                    app.search.context = initial_context.clone();
                                    app.search.filter_mode = default_filter_mode;
                                }
                            },
                            InputAction::Redraw => {
                                terminal.clear()?;
                                terminal.draw(|f| app.draw(f, &results, stats.clone(), inspecting.as_ref(), settings, theme))?;
                            },
                            r => {
                                accept = app.accept;
                                break 'render r;
                            },
                        }
                        if !event::poll(Duration::ZERO)? {
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
        }

        if initial_input != app.search.input.as_str()
            || initial_filter_mode != app.search.filter_mode
            || initial_search_mode != app.search_mode
            || initial_custom_context != app.search.custom_context
        {
            results = app.query_results(&mut db, settings.smart_sort).await?;
        }

        // In custom context mode, when no filter is applied, highlight the entry which was used
        // to enter the context when changing modes. This helps to find your way around.
        if app.search.custom_context.is_some()
            && app.search.input.as_str().is_empty()
            && (initial_custom_context != app.search.custom_context
                || initial_filter_mode != app.search.filter_mode)
            && let Some(history_id) = app.search.custom_context.clone()
            && let Some(pos) = results.iter().position(|entry| entry.id == history_id)
        {
            app.results_state.select(pos);
        }

        let inspecting_id = app.inspecting_state.clone().current;
        // If inspecting ID is not the current inspecting History, update it.
        match inspecting_id {
            Some(inspecting_id) => {
                if inspecting.is_none() || inspecting_id != inspecting.clone().unwrap().id {
                    inspecting = db.load(inspecting_id.0.as_str()).await?;
                }
            }
            _ => {
                inspecting = None;
            }
        }

        stats = if app.tab_index == 0 {
            None
        } else if !results.is_empty() {
            // If we have stats, then we can indicate next available IDs. This avoids passing
            // around a database object, or a full stats object.
            let selected = match inspecting.clone() {
                Some(insp) => insp,
                None => results[app.results_state.selected()].clone(),
            };
            let stats = db.stats(&selected).await?;
            app.inspecting_state.current = Some(selected.id);
            app.inspecting_state.previous = match stats.previous.clone() {
                Some(p) => Some(p.id),
                _ => None,
            };
            app.inspecting_state.next = match stats.next.clone() {
                Some(p) => Some(p.id),
                _ => None,
            };
            Some(stats)
        } else {
            None
        };
    };

    app.finalize_keymap_cursor(settings);

    if inline_height > 0 {
        terminal.clear()?;
    }

    let accept = accept
        && matches!(
            Shell::from_env(),
            Shell::Zsh | Shell::Fish | Shell::Bash | Shell::Xonsh | Shell::Nu | Shell::Powershell
        );

    let accept_prefix = "__atuin_accept__:";

    match result {
        InputAction::AcceptInspecting => {
            match inspecting {
                Some(result) => {
                    let mut command = result.command;

                    if accept {
                        command = String::from(accept_prefix) + &command;
                    }

                    // index is in bounds so we return that entry
                    Ok(command)
                }
                None => Ok(String::new()),
            }
        }
        InputAction::Accept(index) if index < results.len() => {
            let mut command = results.swap_remove(index).command;

            if is_command_chaining {
                command = format!("{} {}", original_query.trim_end(), command);
            } else if accept {
                command = String::from(accept_prefix) + &command;
            }

            // index is in bounds so we return that entry
            Ok(command)
        }
        InputAction::ReturnOriginal => Ok(String::new()),
        InputAction::Copy(index) => {
            let cmd = results.swap_remove(index).command;
            set_clipboard(cmd);
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
        | InputAction::SwitchContext(_) => {
            unreachable!("should have been handled!")
        }
    }
}

// cli-clipboard only works on Windows, Mac, and Linux.

#[cfg(all(
    feature = "clipboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
))]
fn set_clipboard(s: String) {
    let mut ctx = arboard::Clipboard::new().unwrap();
    ctx.set_text(s).unwrap();
    // Use the clipboard context to make sure it is saved
    ctx.get_text().unwrap();
}

#[cfg(not(all(
    feature = "clipboard",
    any(target_os = "windows", target_os = "macos", target_os = "linux")
)))]
fn set_clipboard(_s: String) {}

#[cfg(test)]
mod tests {
    use atuin_client::database::Context;
    use atuin_client::history::History;
    use atuin_client::settings::{
        FilterMode, KeymapMode, Preview, PreviewStrategy, SearchMode, Settings,
    };
    use time::OffsetDateTime;

    use crate::command::client::search::engines::{self, SearchState};
    use crate::command::client::search::history_list::ListState;

    use super::{Compactness, InspectingState, KeymapSet, State};

    #[test]
    #[allow(clippy::too_many_lines)]
    fn calc_preview_height_test() {
        let settings_preview_auto = Settings {
            preview: Preview {
                strategy: PreviewStrategy::Auto,
            },
            show_preview: true,
            ..Settings::utc()
        };

        let settings_preview_auto_h2 = Settings {
            preview: Preview {
                strategy: PreviewStrategy::Auto,
            },
            show_preview: true,
            max_preview_height: 2,
            ..Settings::utc()
        };

        let settings_preview_h4 = Settings {
            preview: Preview {
                strategy: PreviewStrategy::Static,
            },
            show_preview: true,
            max_preview_height: 4,
            ..Settings::utc()
        };

        let settings_preview_fixed = Settings {
            preview: Preview {
                strategy: PreviewStrategy::Fixed,
            },
            show_preview: true,
            max_preview_height: 15,
            ..Settings::utc()
        };

        let cmd_60: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("for i in $(seq -w 10); do echo \"item number $i - abcd\"; done")
            .cwd("/")
            .build()
            .into();

        let cmd_124: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("echo 'Aurea prima sata est aetas, quae vindice nullo, sponte sua, sine lege fidem rectumque colebat. Poena metusque aberant'")
            .cwd("/")
            .build()
            .into();

        let cmd_200: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("CREATE USER atuin WITH ENCRYPTED PASSWORD 'supersecretpassword'; CREATE DATABASE atuin WITH OWNER = atuin; \\c atuin; REVOKE ALL PRIVILEGES ON SCHEMA public FROM PUBLIC; echo 'All done. 200 characters'")
            .cwd("/")
            .build()
            .into();

        let results: Vec<History> = vec![cmd_60, cmd_124, cmd_200];

        // the selected command does not require a preview
        let no_preview = State::calc_preview_height(
            &settings_preview_auto,
            &results,
            0_usize,
            0_usize,
            Compactness::Full,
            1,
            80,
        );
        // the selected command requires 2 lines
        let preview_h2 = State::calc_preview_height(
            &settings_preview_auto,
            &results,
            1_usize,
            0_usize,
            Compactness::Full,
            1,
            80,
        );
        // the selected command requires 3 lines
        let preview_h3 = State::calc_preview_height(
            &settings_preview_auto,
            &results,
            2_usize,
            0_usize,
            Compactness::Full,
            1,
            80,
        );
        // the selected command requires a preview of 1 line (happens when the command is between preview_width-19 and preview_width)
        let preview_one_line = State::calc_preview_height(
            &settings_preview_auto,
            &results,
            0_usize,
            0_usize,
            Compactness::Full,
            1,
            66,
        );
        // the selected command requires 3 lines, but we have a max preview height limit of 2
        let preview_limit_at_2 = State::calc_preview_height(
            &settings_preview_auto_h2,
            &results,
            2_usize,
            0_usize,
            Compactness::Full,
            1,
            80,
        );
        // the longest command requires 3 lines
        let preview_static_h3 = State::calc_preview_height(
            &settings_preview_h4,
            &results,
            1_usize,
            0_usize,
            Compactness::Full,
            1,
            80,
        );
        // the longest command requires 10 lines, but we have a max preview height limit of 4
        let preview_static_limit_at_4 = State::calc_preview_height(
            &settings_preview_h4,
            &results,
            1_usize,
            0_usize,
            Compactness::Full,
            1,
            20,
        );
        // the longest command requires 10 lines, but we have a max preview height of 15 and a fixed preview strategy
        let settings_preview_fixed = State::calc_preview_height(
            &settings_preview_fixed,
            &results,
            1_usize,
            0_usize,
            Compactness::Full,
            1,
            20,
        );

        assert_eq!(no_preview, 1);
        // 1 * 2 is the space for the border
        let border_space = 2;
        assert_eq!(preview_h2, 2 + border_space);
        assert_eq!(preview_h3, 3 + border_space);
        assert_eq!(preview_one_line, 1 + border_space);
        assert_eq!(preview_limit_at_2, 2 + border_space);
        assert_eq!(preview_static_h3, 3 + border_space);
        assert_eq!(preview_static_limit_at_4, 4 + border_space);
        assert_eq!(settings_preview_fixed, 15 + border_space);
    }

    // Test when there's no results, scrolling up or down doesn't underflow
    #[test]
    fn state_scroll_up_underflow() {
        let settings = Settings::utc();
        let mut state = State {
            history_count: 0,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 0,
            accept: false,
            keymap_mode: KeymapMode::Auto,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Directory,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

        state.scroll_up(1);
        state.scroll_down(1);
    }

    #[test]
    fn test_accept_keybindings() {
        use atuin_client::settings::Keys;
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut settings = Settings::utc();
        settings.keys = Keys {
            scroll_exits: true,
            exit_past_line_start: false,
            accept_past_line_end: true,
            accept_past_line_start: false,
            accept_with_backspace: false,
            prefix: "a".to_string(),
        };

        let mut state = State {
            history_count: 1,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 1,
            accept: false,
            keymap_mode: KeymapMode::Emacs,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

        let tab_event = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &tab_event);
        assert!(
            matches!(result, super::InputAction::Accept(_)),
            "Tab should always accept"
        );

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

    #[test]
    fn test_vim_gg_multikey_sequence() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let settings = Settings::utc();

        let mut state = State {
            history_count: 100,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 100,
            accept: false,
            keymap_mode: KeymapMode::VimNormal,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

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

    #[test]
    fn test_vim_g_key_clears_on_other_input() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let settings = Settings::utc();

        let mut state = State {
            history_count: 100,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 100,
            accept: false,
            keymap_mode: KeymapMode::VimNormal,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

        state.results_state.select(50);

        // Press 'g' to set pending state
        let g_event = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        state.handle_key_input(&settings, &g_event);
        assert_eq!(state.pending_vim_key, Some('g'));

        // Press 'j' - should clear pending state
        let j_event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        state.handle_key_input(&settings, &j_event);
        assert_eq!(state.pending_vim_key, None);
    }

    #[test]
    fn test_vim_big_g_jump_to_bottom() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let settings = Settings::utc();

        let mut state = State {
            history_count: 100,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 100,
            accept: false,
            keymap_mode: KeymapMode::VimNormal,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

        state.results_state.select(50);

        // 'G' should jump to visual bottom (index 0 in non-inverted mode)
        let big_g_event = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &big_g_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.results_state.selected(), 0);
    }

    #[test]
    fn test_vim_ctrl_u_d_half_page_scroll() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let settings = Settings::utc();

        let mut state = State {
            history_count: 100,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 100,
            accept: false,
            keymap_mode: KeymapMode::VimNormal,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

        state.results_state.select(50);

        // Ctrl+d should return Continue and clear pending key
        // (scroll amount depends on max_entries which is 0 in tests)
        state.pending_vim_key = Some('g');
        let ctrl_d_event = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        let result = state.handle_key_input(&settings, &ctrl_d_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.pending_vim_key, None);

        // Ctrl+u should return Continue and clear pending key
        state.pending_vim_key = Some('g');
        let ctrl_u_event = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let result = state.handle_key_input(&settings, &ctrl_u_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.pending_vim_key, None);
    }

    #[test]
    fn test_vim_ctrl_f_b_full_page_scroll() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let settings = Settings::utc();

        let mut state = State {
            history_count: 100,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 100,
            accept: false,
            keymap_mode: KeymapMode::VimNormal,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

        state.results_state.select(50);

        // Ctrl+f should return Continue and clear pending key
        // (scroll amount depends on max_entries which is 0 in tests)
        state.pending_vim_key = Some('g');
        let ctrl_f_event = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        let result = state.handle_key_input(&settings, &ctrl_f_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.pending_vim_key, None);

        // Ctrl+b should return Continue and clear pending key
        state.pending_vim_key = Some('g');
        let ctrl_b_event = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        let result = state.handle_key_input(&settings, &ctrl_b_event);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.pending_vim_key, None);
    }

    // -----------------------------------------------------------------------
    // Executor tests (execute_action)
    // -----------------------------------------------------------------------

    /// Helper to build a State for executor tests.
    fn make_executor_state(results_len: usize, selected: usize) -> State {
        let settings = Settings::utc();
        let mut state = State {
            history_count: results_len as i64,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len,
            accept: false,
            keymap_mode: KeymapMode::Emacs,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::defaults(&settings),
            search: SearchState {
                input: String::new().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };
        state.results_state.select(selected);
        state
    }

    #[test]
    fn execute_select_next_no_invert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 50);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::SelectNext, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Non-inverted: SelectNext = scroll_down = selected - 1
        assert_eq!(state.results_state.selected(), 49);
    }

    #[test]
    fn execute_select_next_with_invert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 50);
        let mut settings = Settings::utc();
        settings.invert = true;
        let result = state.execute_action(&Action::SelectNext, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Inverted: SelectNext = scroll_up = selected + 1
        assert_eq!(state.results_state.selected(), 51);
    }

    #[test]
    fn execute_select_previous_no_invert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 50);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::SelectPrevious, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Non-inverted: SelectPrevious = scroll_up = selected + 1
        assert_eq!(state.results_state.selected(), 51);
    }

    #[test]
    fn execute_vim_enter_normal() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::VimEnterNormal, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.keymap_mode, KeymapMode::VimNormal);
    }

    #[test]
    fn execute_vim_enter_insert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        state.keymap_mode = KeymapMode::VimNormal;
        let settings = Settings::utc();
        let result = state.execute_action(&Action::VimEnterInsert, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.keymap_mode, KeymapMode::VimInsert);
    }

    #[test]
    fn execute_accept_sets_accept_flag() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 5);
        let mut settings = Settings::utc();
        settings.enter_accept = true;
        let result = state.execute_action(&Action::Accept, &settings);
        assert!(matches!(result, super::InputAction::Accept(5)));
        assert!(state.accept);
    }

    #[test]
    fn execute_return_selection_does_not_set_accept() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 5);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::ReturnSelection, &settings);
        assert!(matches!(result, super::InputAction::Accept(5)));
        assert!(!state.accept);
    }

    #[test]
    fn execute_accept_nth() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 5);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::AcceptNth(3), &settings);
        assert!(matches!(result, super::InputAction::Accept(8)));
    }

    #[test]
    fn execute_scroll_to_top_no_invert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 50);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::ScrollToTop, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Non-inverted: visual top = highest index
        assert_eq!(state.results_state.selected(), 99);
    }

    #[test]
    fn execute_scroll_to_top_with_invert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 50);
        let mut settings = Settings::utc();
        settings.invert = true;
        let result = state.execute_action(&Action::ScrollToTop, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Inverted: visual top = index 0
        assert_eq!(state.results_state.selected(), 0);
    }

    #[test]
    fn execute_scroll_to_bottom_no_invert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 50);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::ScrollToBottom, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Non-inverted: visual bottom = index 0
        assert_eq!(state.results_state.selected(), 0);
    }

    #[test]
    fn execute_toggle_tab() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        let settings = Settings::utc();
        assert_eq!(state.tab_index, 0);
        state.execute_action(&Action::ToggleTab, &settings);
        assert_eq!(state.tab_index, 1);
        state.execute_action(&Action::ToggleTab, &settings);
        assert_eq!(state.tab_index, 0);
    }

    #[test]
    fn execute_enter_prefix_mode() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        let settings = Settings::utc();
        assert!(!state.prefix);
        state.execute_action(&Action::EnterPrefixMode, &settings);
        assert!(state.prefix);
    }

    #[test]
    fn execute_exit_returns_based_on_exit_mode() {
        use crate::command::client::search::keybindings::Action;
        use atuin_client::settings::ExitMode;

        let mut state = make_executor_state(100, 0);
        let mut settings = Settings::utc();

        settings.exit_mode = ExitMode::ReturnOriginal;
        let result = state.execute_action(&Action::Exit, &settings);
        assert!(matches!(result, super::InputAction::ReturnOriginal));

        settings.exit_mode = ExitMode::ReturnQuery;
        let result = state.execute_action(&Action::Exit, &settings);
        assert!(matches!(result, super::InputAction::ReturnQuery));
    }

    #[test]
    fn execute_return_original() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::ReturnOriginal, &settings);
        assert!(matches!(result, super::InputAction::ReturnOriginal));
    }

    #[test]
    fn execute_copy() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 7);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::Copy, &settings);
        assert!(matches!(result, super::InputAction::Copy(7)));
    }

    #[test]
    fn execute_delete() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 7);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::Delete, &settings);
        assert!(matches!(result, super::InputAction::Delete(7)));
    }

    #[test]
    fn execute_switch_context() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 7);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::SwitchContext, &settings);
        assert!(matches!(result, super::InputAction::SwitchContext(Some(7))));
    }

    #[test]
    fn execute_clear_context() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 7);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::ClearContext, &settings);
        assert!(matches!(result, super::InputAction::SwitchContext(None)));
    }

    #[test]
    fn execute_noop() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 50);
        let settings = Settings::utc();
        let result = state.execute_action(&Action::Noop, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        assert_eq!(state.results_state.selected(), 50);
    }

    #[test]
    fn execute_accept_in_inspector_tab() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 5);
        state.tab_index = 1;
        let settings = Settings::utc();
        let result = state.execute_action(&Action::Accept, &settings);
        assert!(matches!(result, super::InputAction::AcceptInspecting));
    }

    #[test]
    fn execute_cycle_search_mode() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        let settings = Settings::utc();
        let original_mode = state.search_mode;
        let result = state.execute_action(&Action::CycleSearchMode, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        assert!(state.switched_search_mode);
        assert_ne!(state.search_mode, original_mode);
    }

    #[test]
    fn execute_vim_search_insert() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        state.search.input.insert('h');
        state.search.input.insert('i');
        state.keymap_mode = KeymapMode::VimNormal;
        let settings = Settings::utc();
        let result = state.execute_action(&Action::VimSearchInsert, &settings);
        assert!(matches!(result, super::InputAction::Continue));
        // Should clear input and switch to insert mode
        assert_eq!(state.search.input.as_str(), "");
        assert_eq!(state.keymap_mode, KeymapMode::VimInsert);
    }

    #[test]
    fn execute_cursor_movement() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        let settings = Settings::utc();

        // Insert some text
        state.search.input.insert('h');
        state.search.input.insert('e');
        state.search.input.insert('l');
        state.search.input.insert('l');
        state.search.input.insert('o');
        // cursor is at end (position 5)

        // CursorLeft
        state.execute_action(&Action::CursorLeft, &settings);
        assert_eq!(state.search.input.position(), 4);

        // CursorStart
        state.execute_action(&Action::CursorStart, &settings);
        assert_eq!(state.search.input.position(), 0);

        // CursorEnd
        state.execute_action(&Action::CursorEnd, &settings);
        assert_eq!(state.search.input.position(), 5);

        // CursorRight at end does nothing
        state.execute_action(&Action::CursorRight, &settings);
        assert_eq!(state.search.input.position(), 5);
    }

    #[test]
    fn execute_editing() {
        use crate::command::client::search::keybindings::Action;

        let mut state = make_executor_state(100, 0);
        let settings = Settings::utc();

        // Insert "hello"
        state.search.input.insert('h');
        state.search.input.insert('e');
        state.search.input.insert('l');
        state.search.input.insert('l');
        state.search.input.insert('o');

        // DeleteCharBefore (backspace)
        state.execute_action(&Action::DeleteCharBefore, &settings);
        assert_eq!(state.search.input.as_str(), "hell");

        // ClearLine
        state.execute_action(&Action::ClearLine, &settings);
        assert_eq!(state.search.input.as_str(), "");
    }

    #[test]
    fn keymap_config_return_query() {
        use atuin_client::settings::KeyBindingConfig;
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use std::collections::HashMap;

        let mut settings = Settings::utc();
        // Configure tab to return-query
        settings.keymap.emacs = HashMap::from([(
            "tab".to_string(),
            KeyBindingConfig::Simple("return-query".to_string()),
        )]);

        let mut state = State {
            history_count: 100,
            update_needed: None,
            results_state: ListState::default(),
            switched_search_mode: false,
            search_mode: SearchMode::Fuzzy,
            results_len: 100,
            accept: false,
            keymap_mode: KeymapMode::Emacs,
            prefix: false,
            current_cursor: None,
            tab_index: 0,
            pending_vim_key: None,
            original_input_empty: false,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            keymaps: KeymapSet::from_settings(&settings),
            search: SearchState {
                input: "test query".to_string().into(),
                filter_mode: FilterMode::Global,
                context: Context {
                    session: String::new(),
                    cwd: String::new(),
                    hostname: String::new(),
                    host_id: String::new(),
                    git_root: None,
                },
                custom_context: None,
            },
            engine: engines::engine(SearchMode::Fuzzy, &settings),
            now: Box::new(OffsetDateTime::now_utc),
        };

        let tab_event = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let result = state.handle_key_input(&settings, &tab_event);
        assert!(
            matches!(result, super::InputAction::ReturnQuery),
            "Tab configured as return-query should return InputAction::ReturnQuery"
        );
    }
}
