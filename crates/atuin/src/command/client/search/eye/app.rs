//! The Elm-shaped search application.
//!
//! Input flows through the same resolve+execute pipeline as the ratatui
//! path: every key event reaches `handle_key_input` via keymap fallthrough,
//! resolves against the user-configurable `KeymapSet` (conditions, vim
//! pending keys, prefix chords), and `execute_action` carries it out. Both
//! functions are ported nearly verbatim from `interactive.rs` so behavior
//! stays identical by construction; `InputAction` is reused as the interface
//! between them, with the old event loop's arms living in
//! `apply_input_action` (async arms become detached effects).
//!
use std::cell::RefCell;
use std::sync::Arc;

use atuin_client::database::{Context, Database};
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{History, HistoryId, HistoryStats};
use atuin_client::settings::{
    CursorStyle as CfgCursorStyle, ExitMode, FilterMode, KeymapMode, SearchMode, Settings,
};
use atuin_client::theme::Theme;
use crossterm::event::{KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use eye_declare::{
    App, Ctx, CursorStyle, Element, ElementExt, Focus, FocusHandle, InputEvent, Keymap, Task,
    empty, keymap,
};
use semver::Version;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use unicode_width::UnicodeWidthStr;

use super::super::cursor::Cursor;
use super::super::engines::{self, AnySearchEngine, SearchEngine, SearchState};
use super::super::history_list::ListState;
use super::super::interactive::{InputAction, InspectingState};
use super::super::keybindings::key::{KeyCodeValue, KeyInput, SingleKey};
use super::super::keybindings::{Action, EvalContext, KeymapSet};
use super::view::SearchFrame;

/// What the run loop hands back to `eye::history` for resolution into the
/// final shell-facing string. `Default` (stdin closing, driver teardown)
/// deliberately maps to "keep the original command line".
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum Output {
    #[default]
    ReturnOriginal,
    /// Return the current search input to the command line.
    ReturnQuery(String),
    /// A selected history entry; `execute` distinguishes accept-and-run
    /// (the `Accept` action) from return-to-command-line (`ReturnSelection`).
    Selection { command: String, execute: bool },
}

#[derive(Clone)]
pub(super) enum Msg {
    Raw(InputEvent),
    Results {
        generation: u64,
        results: Vec<History>,
    },
    /// The inspector fetch effect finished: the inspected entry and its
    /// stats, boxed to keep the message small.
    Inspected {
        entry: Box<History>,
        stats: Box<HistoryStats>,
    },
    HistoryCount(i64),
    UpdateNeeded(Option<Version>),
    Resize {
        height: u16,
    },
    /// A detached background operation (delete, rebuild) finished; carries
    /// nothing — the model was already updated optimistically.
    OpDone,
}

/// Engine + database behind one lock so query effects serialize; the
/// generation counter drops results that a newer keystroke superseded.
struct QueryBackend {
    engine: AnySearchEngine,
    db: Box<dyn Database>,
}

#[allow(clippy::struct_excessive_bools)]
pub(super) struct SearchApp<'a> {
    pub(super) settings: &'a Settings,
    pub(super) theme: &'a Theme,
    pub(super) search: SearchState,
    pub(super) results: Vec<History>,
    pub(super) results_state: RefCell<ListState>,
    /// A second engine instance used only for match highlighting at render
    /// time — the query engine lives behind an async lock the synchronous
    /// render can't take. Engine construction is cheap (the daemon variant
    /// connects lazily) and `get_highlight_indices` is pure local compute.
    pub(super) highlight_engine: AnySearchEngine,
    pub(super) now: Box<dyn Fn() -> OffsetDateTime + Send>,
    /// The frame's height: the configured inline height, or the tracked
    /// terminal height in fullscreen mode.
    pub(super) frame_height: u16,
    fullscreen: bool,
    pub(super) keymap_mode: KeymapMode,
    pub(super) search_mode: SearchMode,
    pub(super) prefix: bool,
    pub(super) switched_search_mode: bool,
    pub(super) tab_index: usize,
    pub(super) inspecting: Option<History>,
    pub(super) stats: Option<HistoryStats>,
    pub(super) history_count: Option<i64>,
    pub(super) update_needed: Option<Version>,
    inspecting_state: InspectingState,
    /// The entry id `stats` was computed for, so the inspector only hits
    /// the database when the inspected entry actually changes.
    stats_for: Option<HistoryId>,
    inspector_task: Option<Task>,
    /// Set when entering/leaving a custom context with an empty input: the
    /// next results delivery re-selects the context's anchor entry.
    highlight_context_anchor: bool,
    history_store: HistoryStore,
    keymaps: KeymapSet,
    pending_vim_key: Option<char>,
    original_input_empty: bool,
    /// Set by `Action::Accept`/`AcceptNth`; distinguishes accept-and-run
    /// from return-to-command-line when the exit resolves.
    accept: bool,
    /// The keymap-driven cursor shape currently in effect; `None` until a
    /// configured shape first applies (so apps with no `keymap_cursor`
    /// config never touch the terminal's cursor shape).
    current_cursor: Option<CfgCursorStyle>,
    initial_context: Context,
    default_filter_mode: FilterMode,
    /// Swapped in by `CycleSearchMode`; the next query installs it inside
    /// the backend lock (the engine can't be replaced synchronously).
    pending_engine: Option<AnySearchEngine>,
    generation: u64,
    backend: Arc<Mutex<QueryBackend>>,
    query_task: Option<Task>,
    exiting: bool,
    _focus: Focus,
    input_focus: FocusHandle,
}

impl<'a> SearchApp<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search_input: String,
        settings: &'a Settings,
        theme: &'a Theme,
        db: Box<dyn Database>,
        engine: AnySearchEngine,
        highlight_engine: AnySearchEngine,
        history_store: HistoryStore,
        context: Context,
        filter_mode: FilterMode,
        search_mode: SearchMode,
        initial_height: u16,
        fullscreen: bool,
    ) -> Self {
        let original_input_empty = search_input.is_empty();
        let mut input = Cursor::from(search_input);
        input.end();

        let focus = Focus::new();
        let input_focus = focus.handle();
        input_focus.focus();

        let now: Box<dyn Fn() -> OffsetDateTime + Send> = if settings.prefers_reduced_motion {
            let now = OffsetDateTime::now_utc();
            Box::new(move || now)
        } else {
            Box::new(OffsetDateTime::now_utc)
        };

        let mut app = Self {
            settings,
            theme,
            search: SearchState {
                input,
                filter_mode,
                context: context.clone(),
                custom_context: None,
                shells: settings.search.shells.clone(),
            },
            results: Vec::new(),
            results_state: RefCell::new(ListState::default()),
            highlight_engine,
            now,
            frame_height: initial_height,
            fullscreen,
            keymap_mode: match settings.keymap_mode {
                KeymapMode::Auto => KeymapMode::Emacs,
                value => value,
            },
            search_mode,
            prefix: false,
            switched_search_mode: false,
            tab_index: 0,
            inspecting: None,
            stats: None,
            history_count: None,
            update_needed: None,
            inspecting_state: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            stats_for: None,
            inspector_task: None,
            highlight_context_anchor: false,
            history_store,
            keymaps: KeymapSet::from_settings(settings),
            pending_vim_key: None,
            original_input_empty,
            accept: false,
            current_cursor: None,
            initial_context: context,
            default_filter_mode: filter_mode,
            pending_engine: None,
            generation: 0,
            backend: Arc::new(Mutex::new(QueryBackend { engine, db })),
            query_task: None,
            exiting: false,
            _focus: focus,
            input_focus,
        };
        app.initialize_keymap_cursor();
        app
    }

    fn set_keymap_cursor(&mut self, keymap_name: &str) {
        let cursor_style = if keymap_name == "__clear__" {
            None
        } else {
            self.settings.keymap_cursor.get(keymap_name).copied()
        }
        .or_else(|| {
            self.current_cursor
                .map(|_| CfgCursorStyle::DefaultUserShape)
        });

        if cursor_style != self.current_cursor && cursor_style.is_some() {
            self.current_cursor = cursor_style;
        }
    }

    fn initialize_keymap_cursor(&mut self) {
        match self.keymap_mode {
            KeymapMode::Emacs => self.set_keymap_cursor("emacs"),
            KeymapMode::VimNormal => self.set_keymap_cursor("vim_normal"),
            KeymapMode::VimInsert => self.set_keymap_cursor("vim_insert"),
            KeymapMode::Auto => {}
        }
    }

    /// The shell gets the shape configured for its keymap mode; the final
    /// (exit) present emits it.
    fn finalize_keymap_cursor(&mut self) {
        match self.settings.keymap_mode_shell {
            KeymapMode::Emacs => self.set_keymap_cursor("emacs"),
            KeymapMode::VimNormal => self.set_keymap_cursor("vim_normal"),
            KeymapMode::VimInsert => self.set_keymap_cursor("vim_insert"),
            KeymapMode::Auto => self.set_keymap_cursor("__clear__"),
        }
    }

    // The lock is deliberately held across the query await: it serializes
    // engine+db access so a stale query can't interleave with a fresh one.
    #[allow(clippy::significant_drop_tightening)]
    fn spawn_query(&mut self, ctx: &mut Ctx<'_, Self>) {
        self.generation += 1;
        let generation = self.generation;
        let backend = Arc::clone(&self.backend);
        let smart_sort = self.settings.smart_sort;
        let new_engine = self.pending_engine.take();
        // SearchState isn't Clone (Cursor); snapshot the fields the engine reads.
        let state = SearchState {
            input: Cursor::from(self.search.input.as_str().to_owned()),
            filter_mode: self.search.filter_mode,
            context: self.search.context.clone(),
            custom_context: self.search.custom_context.clone(),
            shells: self.search.shells.clone(),
        };
        // Replacing the task drops (cancels) the previous query.
        self.query_task = Some(ctx.perform(async move {
            let results = {
                let mut backend = backend.lock().await;
                if let Some(engine) = new_engine {
                    backend.engine = engine;
                }
                let QueryBackend { engine, db } = &mut *backend;
                engine.query(&state, db.as_mut()).await
            };
            let results = match results {
                Ok(results) => results,
                Err(e) => {
                    tracing::error!(?e, "search query failed");
                    Vec::new()
                }
            };
            let results = if smart_sort {
                atuin_history::sort::sort(state.input.as_str(), results)
            } else {
                results
            };
            Msg::Results {
                generation,
                results,
            }
        }));
    }

    fn finish(&mut self, output: Output, ctx: &mut Ctx<'_, Self>) {
        // Emptying the tail in the same update as the exit makes the final
        // present vacate the region; finalize then reclaims the rows, so the
        // prompt returns to where the search UI appeared. The same present
        // carries the shell's cursor shape.
        self.finalize_keymap_cursor();
        self.exiting = true;
        ctx.exit(output);
    }

    /// Move the selection toward older entries (visually up when the list
    /// is bottom-anchored).
    fn scroll_up(&mut self, scroll_len: usize) {
        let len = self.results.len();
        let state = self.results_state.get_mut();
        let i = state.selected() + scroll_len;
        state.select(i.min(len.saturating_sub(1)));
        self.inspecting_state.reset();
    }

    /// Move the selection toward newer entries.
    fn scroll_down(&mut self, scroll_len: usize) {
        let state = self.results_state.get_mut();
        let i = state.selected().saturating_sub(scroll_len);
        state.select(i);
        self.inspecting_state.reset();
    }

    /// Wheel scrolling in visual terms, matching the ratatui path's
    /// `handle_mouse_input` (wheel only; there is no click-select).
    fn handle_mouse_input(&mut self, input: MouseEvent) {
        match (input.kind, self.settings.invert) {
            (MouseEventKind::ScrollDown, false) | (MouseEventKind::ScrollUp, true) => {
                self.scroll_down(1);
            }
            (MouseEventKind::ScrollDown, true) | (MouseEventKind::ScrollUp, false) => {
                self.scroll_up(1);
            }
            _ => {}
        }
    }

    fn handle_key_exit(settings: &Settings) -> InputAction {
        match settings.exit_mode {
            ExitMode::ReturnOriginal => InputAction::ReturnOriginal,
            ExitMode::ReturnQuery => InputAction::ReturnQuery,
        }
    }

    /// Select the keymap for the current mode (ignoring prefix).
    fn mode_keymap(&self) -> &super::super::keybindings::Keymap {
        if self.tab_index == 1 {
            return &self.keymaps.inspector;
        }
        match self.keymap_mode {
            KeymapMode::Emacs | KeymapMode::Auto => &self.keymaps.emacs,
            KeymapMode::VimNormal => &self.keymaps.vim_normal,
            KeymapMode::VimInsert => &self.keymaps.vim_insert,
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

    fn handle_key_input(&mut self, input: &KeyEvent) -> InputAction {
        // Skip release events
        if input.kind == KeyEventKind::Release {
            return InputAction::Continue;
        }

        // Reset switched_search_mode at start of each key event
        self.switched_search_mode = false;

        // Build evaluation context from current state
        let ctx = EvalContext {
            cursor_position: self.search.input.position(),
            input_width: UnicodeWidthStr::width(self.search.input.as_str()),
            input_byte_len: self.search.input.as_str().len(),
            selected_index: self.results_state.get_mut().selected(),
            results_len: self.results.len(),
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
            self.execute_action(&action)
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

    /// Execute a resolved action, performing all synchronous side effects and
    /// returning the `InputAction` for `apply_input_action` to dispatch.
    ///
    /// Invert handling: scroll actions account for `settings.invert` so that
    /// keybindings are always in "visual" terms — users never need to think
    /// about invert in their keybinding config.
    #[allow(clippy::too_many_lines)]
    fn execute_action(&mut self, action: &Action) -> InputAction {
        let settings = self.settings;
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
                    .get_mut()
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
                    .get_mut()
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
                    .get_mut()
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
                    .get_mut()
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
                    self.results_state.get_mut().select(0);
                } else {
                    let last_idx = self.results.len().saturating_sub(1);
                    self.results_state.get_mut().select(last_idx);
                }
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToBottom => {
                // Visual bottom of history
                if settings.invert {
                    let last_idx = self.results.len().saturating_sub(1);
                    self.results_state.get_mut().select(last_idx);
                } else {
                    self.results_state.get_mut().select(0);
                }
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToScreenTop => {
                // H — jump to top of visible screen
                let results_len = self.results.len();
                let state = self.results_state.get_mut();
                let top = state.offset();
                let visible = state.max_entries().min(results_len);
                let bottom = top + visible.saturating_sub(1);
                state.select(bottom.min(results_len.saturating_sub(1)));
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToScreenMiddle => {
                // M — jump to middle of visible screen
                let results_len = self.results.len();
                let state = self.results_state.get_mut();
                let top = state.offset();
                let visible = state.max_entries().min(results_len);
                let middle = top + visible / 2;
                state.select(middle.min(results_len.saturating_sub(1)));
                self.inspecting_state.reset();
                InputAction::Continue
            }
            Action::ScrollToScreenBottom => {
                // L — jump to bottom of visible screen
                let state = self.results_state.get_mut();
                let top_visible = state.offset();
                state.select(top_visible);
                self.inspecting_state.reset();
                InputAction::Continue
            }

            // -- Commands --
            Action::Accept => {
                if self.tab_index == 1 {
                    return InputAction::AcceptInspecting;
                }
                self.accept = true;
                InputAction::Accept(self.results_state.get_mut().selected())
            }
            Action::AcceptNth(n) => {
                self.accept = true;
                InputAction::Accept(self.results_state.get_mut().selected() + *n as usize)
            }
            Action::ReturnSelection => {
                if self.tab_index == 1 {
                    return InputAction::AcceptInspecting;
                }
                InputAction::Accept(self.results_state.get_mut().selected())
            }
            Action::ReturnSelectionNth(n) => {
                InputAction::Accept(self.results_state.get_mut().selected() + *n as usize)
            }
            Action::Copy => InputAction::Copy(self.results_state.get_mut().selected()),
            Action::Delete => InputAction::Delete(self.results_state.get_mut().selected()),
            Action::DeleteAll => {
                InputAction::DeleteAllMatching(self.results_state.get_mut().selected())
            }
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
                self.pending_engine = Some(engines::engine(self.search_mode, settings));
                self.highlight_engine = engines::engine(self.search_mode, settings);
                InputAction::Continue
            }
            Action::SwitchContext => {
                InputAction::SwitchContext(Some(self.results_state.get_mut().selected()))
            }
            Action::ClearContext => InputAction::SwitchContext(None),
            Action::ToggleTab => {
                self.tab_index = (self.tab_index + 1) % 2;
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

            // -- Mode changes --
            // Cursor-shape changes (set_keymap_cursor in the ratatui path)
            // need an eye_declare capability that lands in a later phase.
            Action::VimEnterNormal => {
                self.set_keymap_cursor("vim_normal");
                self.keymap_mode = KeymapMode::VimNormal;
                InputAction::Continue
            }
            Action::VimEnterInsert => {
                self.set_keymap_cursor("vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAfter => {
                self.search.input.right();
                self.set_keymap_cursor("vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAtStart => {
                self.search.input.start();
                self.set_keymap_cursor("vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAtEnd => {
                self.search.input.end();
                self.set_keymap_cursor("vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimSearchInsert => {
                self.search.input.clear();
                self.set_keymap_cursor("vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimChangeToEnd => {
                self.search.input.clear_to_end();
                self.set_keymap_cursor("vim_insert");
                self.keymap_mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::EnterPrefixMode => {
                self.prefix = true;
                InputAction::Continue
            }

            // -- Special --
            Action::Noop => InputAction::Continue,
        }
    }

    /// The old event loop's match arms: dispatch the `InputAction` that
    /// `handle_key_input` resolved, turning exits into `Output`s and async
    /// work into detached effects.
    fn apply_input_action(&mut self, action: InputAction, ctx: &mut Ctx<'_, Self>) {
        match action {
            InputAction::Continue | InputAction::Redraw => {}
            InputAction::AcceptInspecting => {
                let output = match &self.inspecting {
                    Some(entry) => Output::Selection {
                        command: entry.command.clone(),
                        execute: self.accept,
                    },
                    None => Output::ReturnOriginal,
                };
                self.finish(output, ctx);
            }
            InputAction::Accept(index) => {
                let execute = self.accept;
                let output = match self.results.get(index) {
                    Some(entry) => Output::Selection {
                        command: entry.command.clone(),
                        execute,
                    },
                    // Out of bounds usually implies no selected entry, so
                    // return the input (matching the ratatui path).
                    None => Output::ReturnQuery(self.search.input.as_str().to_owned()),
                };
                self.finish(output, ctx);
            }
            InputAction::ReturnOriginal => self.finish(Output::ReturnOriginal, ctx),
            InputAction::ReturnQuery => {
                let input = self.search.input.as_str().to_owned();
                self.finish(Output::ReturnQuery(input), ctx);
            }
            InputAction::Copy(index) => {
                if let Some(entry) = self.results.get(index)
                    && let Err(e) = super::super::interactive::set_clipboard(entry.command.clone())
                {
                    tracing::warn!(?e, "failed to copy to clipboard");
                }
                self.finish(Output::ReturnOriginal, ctx);
            }
            InputAction::Delete(index) => self.delete_single(index, ctx),
            InputAction::DeleteAllMatching(index) => self.delete_all_matching(index, ctx),
            InputAction::SwitchContext(selected) => self.switch_context(selected),
        }
    }

    // The lock guard spans the rebuild await on purpose: it keeps the delete
    // from interleaving with an in-flight query on the shared db handle.
    #[allow(clippy::significant_drop_tightening)]
    fn delete_single(&mut self, index: usize, ctx: &mut Ctx<'_, Self>) {
        if self.results.is_empty() || index >= self.results.len() {
            return;
        }
        let state = self.results_state.get_mut();
        let selected = state.selected();
        if selected == self.results.len() - 1 {
            state.select(selected.saturating_sub(1));
        }
        let entry = self.results.remove(index);
        self.inspecting_state.reset();
        self.tab_index = 0;

        let store = self.history_store.clone();
        let backend = Arc::clone(&self.backend);
        ctx.perform(async move {
            let ids = match store.delete_entries([entry]).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!(?e, "failed to delete history entry");
                    return Msg::OpDone;
                }
            };
            let backend = backend.lock().await;
            if let Err(e) = store.build_all(&*backend.db, &ids).await {
                tracing::error!(?e, "failed to rebuild history after delete");
            }
            Msg::OpDone
        })
        .detach();
    }

    // See delete_single for why the guard spans the awaits.
    #[allow(clippy::significant_drop_tightening)]
    fn delete_all_matching(&mut self, index: usize, ctx: &mut Ctx<'_, Self>) {
        if self.results.is_empty() || index >= self.results.len() {
            return;
        }
        let command = self.results[index].command.clone();

        // Remove matching entries from the visible results
        self.results.retain(|e| e.command != command);
        *self.results_state.get_mut() = ListState::default();
        self.inspecting_state.reset();
        self.tab_index = 0;

        // Query the DB for ALL entries with this command and delete them
        let store = self.history_store.clone();
        let backend = Arc::clone(&self.backend);
        ctx.perform(async move {
            let backend = backend.lock().await;
            let all_matching = match backend
                .db
                .query_history(&format!(
                    "select * from history where command = '{}' and deleted_at is null",
                    command.replace('\'', "''")
                ))
                .await
            {
                Ok(all) => all,
                Err(e) => {
                    tracing::error!(?e, "failed to query history for delete-all");
                    return Msg::OpDone;
                }
            };
            let ids = match store.delete_entries(all_matching).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!(?e, "failed to delete history entries");
                    return Msg::OpDone;
                }
            };
            if let Err(e) = store.build_all(&*backend.db, &ids).await {
                tracing::error!(?e, "failed to rebuild history after delete-all");
            }
            Msg::OpDone
        })
        .detach();
    }

    /// Keep the inspector's entry + stats in sync with the model: when the
    /// inspector tab is showing, fetch whenever the target entry (the
    /// explicitly inspected id, else the selection) differs from what
    /// `stats` was computed for. Runs after every update; `Msg::Inspected`
    /// re-enters it, so a target that moved mid-fetch converges on the next
    /// pass. Mirrors the per-iteration stats block of the ratatui loop.
    fn sync_inspector(&mut self, ctx: &mut Ctx<'_, Self>) {
        if self.tab_index != 1 || self.results.is_empty() {
            self.stats = None;
            self.stats_for = None;
            return;
        }
        let selected = self
            .results_state
            .get_mut()
            .selected()
            .min(self.results.len() - 1);
        let fallback = self.results[selected].clone();
        let inspected = self.inspecting_state.current.clone();
        let target = inspected.clone().unwrap_or_else(|| fallback.id.clone());
        if self.stats_for.as_ref() == Some(&target) {
            return;
        }
        let backend = Arc::clone(&self.backend);
        // Replacing the task cancels a fetch for a stale target.
        self.inspector_task = Some(ctx.perform(async move {
            let backend = backend.lock().await;
            let entry = match inspected {
                Some(id) => match backend.db.load(id.0.as_str()).await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => fallback,
                    Err(e) => {
                        tracing::error!(?e, "failed to load inspected entry");
                        return Msg::OpDone;
                    }
                },
                None => fallback,
            };
            match backend.db.stats(&entry).await {
                Ok(stats) => Msg::Inspected {
                    entry: Box::new(entry),
                    stats: Box::new(stats),
                },
                Err(e) => {
                    tracing::error!(?e, "failed to compute history stats");
                    Msg::OpDone
                }
            }
        }));
    }

    fn switch_context(&mut self, selected: Option<usize>) {
        if let Some(index) = selected
            && let Some(entry) = self.results.get(index)
        {
            self.search.custom_context = Some(entry.id.clone());
            self.search.context = Context::from_history(entry);
            self.search.filter_mode = FilterMode::Session;
            self.search.input = Cursor::from(String::new());
            *self.results_state.get_mut() = ListState::default();
        } else {
            self.search.custom_context = None;
            self.search.context = self.initial_context.clone();
            self.search.filter_mode = self.default_filter_mode;
        }
    }
}

impl App for SearchApp<'_> {
    type Msg = Msg;
    type Output = Output;

    fn init(&mut self, ctx: &mut Ctx<'_, Self>) {
        self.spawn_query(ctx);

        // Counting history is a full table scan, which can take a while on a
        // large, cold database — don't hold up the first frame for it.
        let backend = Arc::clone(&self.backend);
        ctx.perform(async move {
            let backend = backend.lock().await;
            match backend.db.history_count(false).await {
                Ok(count) => Msg::HistoryCount(count),
                Err(e) => {
                    tracing::error!(?e, "failed to count history");
                    Msg::OpDone
                }
            }
        })
        .detach();

        let settings = self.settings.clone();
        ctx.perform(async move { Msg::UpdateNeeded(settings.needs_update().await) })
            .detach();
    }

    fn update(&mut self, msg: Msg, ctx: &mut Ctx<'_, Self>) {
        match msg {
            Msg::Results {
                generation,
                results,
            } => {
                if generation == self.generation {
                    self.results = results;
                    // New results reset the selection, matching query_results.
                    self.results_state.get_mut().select(0);
                    self.inspecting_state.reset();
                    // In custom context mode with no filter, highlight the
                    // entry that was used to enter the context.
                    if self.highlight_context_anchor {
                        self.highlight_context_anchor = false;
                        if let Some(id) = self.search.custom_context.clone()
                            && let Some(pos) = self.results.iter().position(|e| e.id == id)
                        {
                            self.results_state.get_mut().select(pos);
                        }
                    }
                }
            }
            Msg::Inspected { entry, stats } => {
                if self.tab_index == 1 {
                    self.inspecting_state.current = Some(entry.id.clone());
                    self.inspecting_state.previous = stats.previous.as_ref().map(|p| p.id.clone());
                    self.inspecting_state.next = stats.next.as_ref().map(|n| n.id.clone());
                    self.stats_for = Some(entry.id.clone());
                    self.inspecting = Some(*entry);
                    self.stats = Some(*stats);
                }
            }
            Msg::HistoryCount(count) => self.history_count = Some(count),
            Msg::Resize { height } => {
                // Inline frames are a fixed height; only fullscreen tracks
                // the terminal.
                if self.fullscreen {
                    self.frame_height = height;
                }
            }
            Msg::UpdateNeeded(version) => self.update_needed = version,
            Msg::OpDone => {}
            Msg::Raw(event) => {
                // The old event loop requeried when an input pass changed
                // anything the engine reads; mirror that by diffing the
                // query-relevant state around the key handling.
                let initial_input = self.search.input.as_str().to_owned();
                let initial_filter_mode = self.search.filter_mode;
                let initial_search_mode = self.search_mode;
                let initial_custom_context = self.search.custom_context.clone();

                match event {
                    InputEvent::Key(key) => {
                        let action = self.handle_key_input(&key);
                        self.apply_input_action(action, ctx);
                    }
                    InputEvent::Paste(text) => {
                        for c in text.chars() {
                            self.search.input.insert(c);
                        }
                    }
                    InputEvent::Mouse(mouse) => self.handle_mouse_input(mouse),
                    // InputEvent is non-exhaustive; future kinds are ignored.
                    _ => {}
                }

                if !self.exiting
                    && (initial_input != self.search.input.as_str()
                        || initial_filter_mode != self.search.filter_mode
                        || initial_search_mode != self.search_mode
                        || initial_custom_context != self.search.custom_context)
                {
                    // The anchor re-select fires when a context change (or
                    // its filter modes) delivers unfiltered results.
                    self.highlight_context_anchor = self.search.custom_context.is_some()
                        && self.search.input.as_str().is_empty()
                        && (initial_custom_context != self.search.custom_context
                            || initial_filter_mode != self.search.filter_mode);
                    self.spawn_query(ctx);
                }
            }
        }
        self.sync_inspector(ctx);
    }

    fn tail(&self) -> impl Element + '_ {
        if self.exiting {
            return empty().any();
        }
        SearchFrame { app: self }.any()
    }

    fn keymap(&self) -> Keymap<Msg> {
        keymap().fallthrough(&self.input_focus, Msg::Raw)
    }

    fn cursor_style(&self) -> CursorStyle {
        match self.current_cursor {
            None | Some(CfgCursorStyle::DefaultUserShape) => CursorStyle::DefaultUserShape,
            Some(CfgCursorStyle::BlinkingBlock) => CursorStyle::BlinkingBlock,
            Some(CfgCursorStyle::SteadyBlock) => CursorStyle::SteadyBlock,
            Some(CfgCursorStyle::BlinkingUnderScore) => CursorStyle::BlinkingUnderScore,
            Some(CfgCursorStyle::SteadyUnderScore) => CursorStyle::SteadyUnderScore,
            Some(CfgCursorStyle::BlinkingBar) => CursorStyle::BlinkingBar,
            Some(CfgCursorStyle::SteadyBar) => CursorStyle::SteadyBar,
        }
    }

    fn on_resize(&self, _width: u16, height: u16) -> Option<Msg> {
        Some(Msg::Resize { height })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use atuin_client::database::Sqlite;
    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_client::settings::{ExitMode, KeyBindingConfig, Keys};
    use atuin_client::theme::ThemeManager;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::HostId;
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::super::super::keybindings::KeymapSet;
    use super::*;

    fn test_theme() -> &'static Theme {
        let manager = Box::leak(Box::new(ThemeManager::new(Some(false), None)));
        manager.load_theme("default", None)
    }

    fn dummy_results(n: usize) -> Vec<History> {
        (0..n)
            .map(|i| {
                History::capture()
                    .timestamp(OffsetDateTime::now_utc())
                    .command(format!("command number {i}"))
                    .cwd("/")
                    .build()
                    .into()
            })
            .collect()
    }

    /// Build a `SearchApp` mirroring the ratatui suite's `state` fixture:
    /// `results_len` dummy results, `selected` index, and — matching that
    /// fixture — `original_input_empty` forced to false.
    async fn test_app(
        keymap_mode: KeymapMode,
        results_len: usize,
        selected: usize,
        input: &str,
        settings: Settings,
    ) -> SearchApp<'static> {
        let settings: &'static Settings = Box::leak(Box::new(settings));
        let db = Sqlite::new("sqlite::memory:", 30.0).await.unwrap();
        let store = SqliteStore::new(":memory:", 30.0).await.unwrap();
        let history_store = HistoryStore::new(store, HostId(uuid_v7()), [0u8; 32]);

        let mut app = SearchApp::new(
            input.to_string(),
            settings,
            test_theme(),
            Box::new(db),
            engines::engine(SearchMode::Fuzzy, settings),
            engines::engine(SearchMode::Fuzzy, settings),
            history_store,
            Context {
                session: String::new(),
                cwd: String::new(),
                hostname: String::new(),
                host_id: String::new(),
                git_root: None,
            },
            FilterMode::Global,
            SearchMode::Fuzzy,
            20,
            false,
        );
        app.keymap_mode = keymap_mode;
        app.original_input_empty = false;
        app.results = dummy_results(results_len);
        app.results_state.get_mut().select(selected);
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[tokio::test]
    async fn accept_keybindings() {
        let base_keys = Keys {
            scroll_exits: true,
            exit_past_line_start: false,
            accept_past_line_end: true,
            accept_past_line_start: false,
            accept_with_backspace: false,
            prefix: "a".to_string(),
        };
        let mut settings = Settings::utc();
        settings.keys = base_keys.clone();
        let mut app = test_app(KeymapMode::Emacs, 1, 0, "", settings).await;

        let rebind = |app: &mut SearchApp<'_>, f: &dyn Fn(&mut Keys)| {
            let mut settings = Settings::utc();
            settings.keys = base_keys.clone();
            f(&mut settings.keys);
            app.keymaps = KeymapSet::defaults(&settings);
        };

        assert!(
            matches!(
                app.handle_key_input(&key(KeyCode::Tab)),
                InputAction::Accept(_)
            ),
            "Tab should always accept"
        );

        // Left arrow with accept_past_line_start disabled → continue
        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Left)),
            InputAction::Continue
        ));

        // Left arrow with accept_past_line_start enabled → accept at line start
        rebind(&mut app, &|k| k.accept_past_line_start = true);
        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Left)),
            InputAction::Accept(_)
        ));
        rebind(&mut app, &|_| {});

        // Backspace disabled → continue
        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Backspace)),
            InputAction::Continue
        ));

        // Backspace enabled → accept at line start
        rebind(&mut app, &|k| k.accept_with_backspace = true);
        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Backspace)),
            InputAction::Accept(_)
        ));

        for c in "test".chars() {
            app.search.input.insert(c);
        }
        app.search.input.end();

        // Right arrow at end of line with accept_past_line_end → accept
        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Right)),
            InputAction::Accept(_)
        ));

        // With text present, line-start accepts no longer fire
        rebind(&mut app, &|k| k.accept_past_line_start = true);
        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Left)),
            InputAction::Continue
        ));
        rebind(&mut app, &|k| k.accept_with_backspace = true);
        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Backspace)),
            InputAction::Continue
        ));
    }

    #[tokio::test]
    async fn vim_gg_multikey_sequence() {
        let mut app = test_app(KeymapMode::VimNormal, 100, 50, "", Settings::utc()).await;

        let g = key(KeyCode::Char('g'));
        assert!(matches!(app.handle_key_input(&g), InputAction::Continue));
        assert_eq!(app.pending_vim_key, Some('g'));
        assert_eq!(app.results_state.get_mut().selected(), 50);

        assert!(matches!(app.handle_key_input(&g), InputAction::Continue));
        assert_eq!(app.pending_vim_key, None);
        assert_eq!(app.results_state.get_mut().selected(), 99);
    }

    #[tokio::test]
    async fn vim_g_key_clears_on_other_input() {
        let mut app = test_app(KeymapMode::VimNormal, 100, 50, "", Settings::utc()).await;

        app.handle_key_input(&key(KeyCode::Char('g')));
        assert_eq!(app.pending_vim_key, Some('g'));

        app.handle_key_input(&key(KeyCode::Char('j')));
        assert_eq!(app.pending_vim_key, None);
    }

    #[tokio::test]
    async fn vim_big_g_jump_to_bottom() {
        let mut app = test_app(KeymapMode::VimNormal, 100, 50, "", Settings::utc()).await;

        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Char('G'))),
            InputAction::Continue
        ));
        assert_eq!(app.results_state.get_mut().selected(), 0);
    }

    #[tokio::test]
    async fn vim_ctrl_scroll_clears_pending() {
        for c in ['d', 'u', 'f', 'b'] {
            let mut app = test_app(KeymapMode::VimNormal, 100, 50, "", Settings::utc()).await;
            app.pending_vim_key = Some('g');
            assert!(matches!(
                app.handle_key_input(&ctrl(c)),
                InputAction::Continue
            ));
            assert_eq!(app.pending_vim_key, None);
        }
    }

    #[tokio::test]
    async fn execute_scroll_selection() {
        let cases = [
            (false, Action::SelectNext, 49),
            (true, Action::SelectNext, 51),
            (false, Action::SelectPrevious, 51),
            (false, Action::ScrollToTop, 99),
            (true, Action::ScrollToTop, 0),
            (false, Action::ScrollToBottom, 0),
        ];
        for (invert, action, expected) in cases {
            let mut settings = Settings::utc();
            settings.invert = invert;
            let mut app = test_app(KeymapMode::Emacs, 100, 50, "", settings).await;
            assert!(matches!(app.execute_action(&action), InputAction::Continue));
            assert_eq!(
                app.results_state.get_mut().selected(),
                expected,
                "invert={invert} action={action:?}"
            );
        }
    }

    #[tokio::test]
    async fn execute_vim_mode_change() {
        let cases = [
            (
                KeymapMode::Emacs,
                Action::VimEnterNormal,
                KeymapMode::VimNormal,
            ),
            (
                KeymapMode::VimNormal,
                Action::VimEnterInsert,
                KeymapMode::VimInsert,
            ),
            (
                KeymapMode::VimInsert,
                Action::VimEnterNormal,
                KeymapMode::VimNormal,
            ),
        ];
        for (start, action, expected) in cases {
            let mut app = test_app(start, 100, 0, "", Settings::utc()).await;
            assert!(matches!(app.execute_action(&action), InputAction::Continue));
            assert_eq!(app.keymap_mode, expected);
        }
    }

    #[tokio::test]
    async fn execute_accept_sets_accept_flag() {
        let mut app = test_app(KeymapMode::Emacs, 100, 5, "", Settings::utc()).await;
        assert!(matches!(
            app.execute_action(&Action::Accept),
            InputAction::Accept(5)
        ));
        assert!(app.accept);
    }

    #[tokio::test]
    async fn execute_return_selection_does_not_set_accept() {
        let mut app = test_app(KeymapMode::Emacs, 100, 5, "", Settings::utc()).await;
        assert!(matches!(
            app.execute_action(&Action::ReturnSelection),
            InputAction::Accept(5)
        ));
        assert!(!app.accept);
    }

    #[tokio::test]
    async fn execute_accept_nth() {
        let mut app = test_app(KeymapMode::Emacs, 100, 5, "", Settings::utc()).await;
        assert!(matches!(
            app.execute_action(&Action::AcceptNth(3)),
            InputAction::Accept(8)
        ));
    }

    #[tokio::test]
    async fn execute_enter_prefix_mode() {
        let mut app = test_app(KeymapMode::Emacs, 100, 0, "", Settings::utc()).await;
        assert!(!app.prefix);
        app.execute_action(&Action::EnterPrefixMode);
        assert!(app.prefix);
    }

    #[tokio::test]
    async fn prefix_chord_ctrl_a_c_switches_context() {
        let mut app = test_app(KeymapMode::Emacs, 100, 7, "", Settings::utc()).await;

        assert!(matches!(
            app.handle_key_input(&ctrl('a')),
            InputAction::Continue
        ));
        assert!(app.prefix, "ctrl-a should enter prefix mode");

        let result = app.handle_key_input(&key(KeyCode::Char('c')));
        assert!(
            matches!(result, InputAction::SwitchContext(Some(7))),
            "prefix + c should switch context"
        );
        assert_eq!(app.search.input.as_str(), "", "c should not be inserted");
    }

    #[tokio::test]
    async fn execute_exit_returns_based_on_exit_mode() {
        let mut settings = Settings::utc();
        settings.exit_mode = ExitMode::ReturnOriginal;
        let mut app = test_app(KeymapMode::Emacs, 100, 0, "", settings).await;
        assert!(matches!(
            app.execute_action(&Action::Exit),
            InputAction::ReturnOriginal
        ));

        let mut settings = Settings::utc();
        settings.exit_mode = ExitMode::ReturnQuery;
        let mut app = test_app(KeymapMode::Emacs, 100, 0, "", settings).await;
        assert!(matches!(
            app.execute_action(&Action::Exit),
            InputAction::ReturnQuery
        ));
    }

    #[tokio::test]
    async fn execute_command_dispositions() {
        let mut app = test_app(KeymapMode::Emacs, 100, 7, "", Settings::utc()).await;
        assert!(matches!(
            app.execute_action(&Action::ReturnOriginal),
            InputAction::ReturnOriginal
        ));
        assert!(matches!(
            app.execute_action(&Action::Copy),
            InputAction::Copy(7)
        ));
        assert!(matches!(
            app.execute_action(&Action::Delete),
            InputAction::Delete(7)
        ));
        assert!(matches!(
            app.execute_action(&Action::SwitchContext),
            InputAction::SwitchContext(Some(7))
        ));
        assert!(matches!(
            app.execute_action(&Action::ClearContext),
            InputAction::SwitchContext(None)
        ));
        assert!(matches!(
            app.execute_action(&Action::Noop),
            InputAction::Continue
        ));
    }

    #[tokio::test]
    async fn execute_cycle_search_mode() {
        let mut app = test_app(KeymapMode::Emacs, 100, 0, "", Settings::utc()).await;
        let original_mode = app.search_mode;
        assert!(matches!(
            app.execute_action(&Action::CycleSearchMode),
            InputAction::Continue
        ));
        assert!(app.switched_search_mode);
        assert_ne!(app.search_mode, original_mode);
        assert!(
            app.pending_engine.is_some(),
            "the next query must install the new engine"
        );
    }

    #[tokio::test]
    async fn execute_vim_search_insert() {
        let mut app = test_app(KeymapMode::VimNormal, 100, 0, "", Settings::utc()).await;
        app.search.input.insert('h');
        app.search.input.insert('i');
        assert!(matches!(
            app.execute_action(&Action::VimSearchInsert),
            InputAction::Continue
        ));
        assert_eq!(app.search.input.as_str(), "");
        assert_eq!(app.keymap_mode, KeymapMode::VimInsert);
    }

    #[tokio::test]
    async fn execute_cursor_movement() {
        let mut app = test_app(KeymapMode::Emacs, 100, 0, "", Settings::utc()).await;
        for c in "hello".chars() {
            app.search.input.insert(c);
        }

        app.execute_action(&Action::CursorLeft);
        assert_eq!(app.search.input.position(), 4);
        app.execute_action(&Action::CursorStart);
        assert_eq!(app.search.input.position(), 0);
        app.execute_action(&Action::CursorEnd);
        assert_eq!(app.search.input.position(), 5);
        app.execute_action(&Action::CursorRight);
        assert_eq!(app.search.input.position(), 5);
    }

    #[tokio::test]
    async fn execute_editing() {
        let mut app = test_app(KeymapMode::Emacs, 100, 0, "", Settings::utc()).await;
        for c in "hello".chars() {
            app.search.input.insert(c);
        }

        app.execute_action(&Action::DeleteCharBefore);
        assert_eq!(app.search.input.as_str(), "hell");
        app.execute_action(&Action::ClearLine);
        assert_eq!(app.search.input.as_str(), "");
    }

    #[tokio::test]
    async fn keymap_config_return_query() {
        let mut settings = Settings::utc();
        settings.keymap.emacs = HashMap::from([(
            "tab".to_string(),
            KeyBindingConfig::Simple("return-query".to_string()),
        )]);
        let mut app = test_app(KeymapMode::Emacs, 100, 0, "test query", settings).await;
        // Rebuild from the same modified settings (test_app leaked them).
        app.keymaps = KeymapSet::from_settings(app.settings);

        assert!(
            matches!(
                app.handle_key_input(&key(KeyCode::Tab)),
                InputAction::ReturnQuery
            ),
            "Tab configured as return-query should resolve to ReturnQuery"
        );
    }

    /// End-to-end through the eye runtime: keys in, `Output` out.
    #[tokio::test]
    async fn runtime_enter_and_esc_produce_outputs() {
        let app = test_app(KeymapMode::Emacs, 3, 1, "", Settings::utc()).await;
        let mut runtime = eye_declare::Runtime::new(app, 120, 30);
        let (_bytes, exit) = runtime.handle(InputEvent::Key(key(KeyCode::Enter)));
        match exit {
            Some(Output::Selection { command, execute }) => {
                assert_eq!(command, "command number 1");
                // enter_accept defaults to false in Settings::utc()
                assert!(!execute);
            }
            other => panic!("expected Selection output, got {other:?}"),
        }

        let app = test_app(KeymapMode::Emacs, 3, 0, "", Settings::utc()).await;
        let mut runtime = eye_declare::Runtime::new(app, 120, 30);
        let (_bytes, exit) = runtime.handle(InputEvent::Key(key(KeyCode::Esc)));
        assert_eq!(exit, Some(Output::ReturnOriginal));
    }
    /// Regression: Esc in vim-insert must enter vim-normal, not exit.
    #[tokio::test]
    async fn esc_in_vim_insert_enters_normal_via_runtime() {
        let mut settings = Settings::utc();
        settings.keymap_mode = KeymapMode::VimInsert;
        // Build WITHOUT overriding keymap_mode: mirror production construction.
        let settings_ref: &'static Settings = Box::leak(Box::new(settings));
        let db = Sqlite::new("sqlite::memory:", 30.0).await.unwrap();
        let store = SqliteStore::new(":memory:", 30.0).await.unwrap();
        let history_store = HistoryStore::new(store, HostId(uuid_v7()), [0u8; 32]);
        let app = SearchApp::new(
            String::new(),
            settings_ref,
            test_theme(),
            Box::new(db),
            engines::engine(SearchMode::Fuzzy, settings_ref),
            engines::engine(SearchMode::Fuzzy, settings_ref),
            history_store,
            Context {
                session: String::new(),
                cwd: String::new(),
                hostname: String::new(),
                host_id: String::new(),
                git_root: None,
            },
            FilterMode::Global,
            SearchMode::Fuzzy,
            20,
            false,
        );
        assert_eq!(app.keymap_mode, KeymapMode::VimInsert);
        let mut runtime = eye_declare::Runtime::new(app, 120, 30);
        let (_bytes, exit) = runtime.handle(InputEvent::Key(key(KeyCode::Esc)));
        assert!(
            exit.is_none(),
            "esc in vim-insert must not exit, got {exit:?}"
        );
        assert_eq!(runtime.app().keymap_mode, KeymapMode::VimNormal);
    }
}
