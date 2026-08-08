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
use std::sync::Arc;

use atuin_client::database::{Context, Database};
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{History, HistoryStats};
use atuin_client::settings::{ExitMode, FilterMode, KeymapMode, SearchMode, Settings};
use atuin_client::theme::Theme;
use crossterm::event::{KeyEvent, KeyEventKind, MouseEvent, MouseEventKind};
use eye_declare::{
    App, Ctx, CursorStyle, Element, ElementExt, Focus, FocusHandle, InputEvent, Keymap, empty,
    keymap,
};
use semver::Version;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use super::super::cursor::Cursor;
use super::super::engines::{self, AnySearchEngine, SearchState};
use super::super::interactive::InputAction;
use super::super::keybindings::key::{KeyCodeValue, KeyInput, SingleKey};
use super::super::keybindings::{Action, EvalContext};
use super::state::{
    self, Generation, Inspector, KeymapState, Launch, Listing, Querying, Status, Tab, Viewport,
};
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
        generation: Generation,
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
    /// A background operation failed *before anything was persisted*:
    /// re-run the query so the list reconverges with the database (the
    /// entry honestly reappears; the ratatui path aborted the whole TUI
    /// here instead). Not sent for failures after the delete tombstone is
    /// in the record store — at that point the optimistic removal IS the
    /// truth and the sqlite query index is merely stale.
    Requery,
}

#[allow(clippy::struct_excessive_bools)]
pub(super) struct SearchApp<'a> {
    pub(super) settings: &'a Settings,
    pub(super) theme: &'a Theme,
    pub(super) search: SearchState,
    pub(super) listing: Listing,
    pub(super) viewport: Viewport,
    pub(super) keymap: KeymapState,
    pub(super) inspector: Inspector,
    pub(super) status: Status,
    pub(super) tab: Tab,
    pub(super) search_mode: SearchMode,
    pub(super) switched_search_mode: bool,
    /// A second engine instance used only for match highlighting at render
    /// time — the query engine lives behind an async lock the synchronous
    /// render can't take. Engine construction is cheap (the daemon variant
    /// connects lazily) and `get_highlight_indices` is pure local compute.
    pub(super) highlight_engine: AnySearchEngine,
    pub(super) now: Box<dyn Fn() -> OffsetDateTime + Send>,
    pub(super) query: Querying,
    launch: Launch,
    history_store: HistoryStore,
    /// Set by `Action::Accept`/`AcceptNth`; distinguishes accept-and-run
    /// from return-to-command-line when the exit resolves.
    accept: bool,
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

        Self {
            settings,
            theme,
            search: SearchState {
                input,
                filter_mode,
                context: context.clone(),
                custom_context: None,
                shells: settings.search.shells.clone(),
            },
            listing: Listing::new(),
            viewport: Viewport {
                height: initial_height,
                fullscreen,
            },
            keymap: KeymapState::new(settings),
            inspector: Inspector::new(),
            status: Status::default(),
            tab: Tab::Search,
            search_mode,
            switched_search_mode: false,
            highlight_engine,
            now,
            query: Querying::new(engine, db),
            launch: Launch {
                initial_context: context,
                default_filter_mode: filter_mode,
                original_input_empty,
            },
            history_store,
            accept: false,
            exiting: false,
            _focus: focus,
            input_focus,
        }
    }

    fn spawn_query(&mut self, ctx: &mut Ctx<'_, Self>) {
        let generation = self.listing.entries.begin_refresh();
        self.query.spawn(
            ctx,
            generation,
            state::snapshot(&self.search),
            self.settings.smart_sort,
        );
    }

    fn finish(&mut self, output: Output, ctx: &mut Ctx<'_, Self>) {
        // Emptying the tail in the same update as the exit makes the final
        // present vacate the region; finalize then reclaims the rows, so the
        // prompt returns to where the search UI appeared. The same present
        // carries the shell's cursor shape.
        self.keymap.finalize_cursor(self.settings);
        self.exiting = true;
        ctx.exit(output);
    }

    /// Move the selection toward older entries (visually up when the list
    /// Move the selection, resetting the inspector's navigation anchor —
    /// inspection follows the selection until explicitly navigated.
    fn scroll_up(&mut self, scroll_len: usize) {
        self.listing.scroll_up(scroll_len);
        self.inspector.reset_nav();
    }

    fn scroll_down(&mut self, scroll_len: usize) {
        self.listing.scroll_down(scroll_len);
        self.inspector.reset_nav();
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
            selected_index: self.listing.state.get_mut().selected(),
            results_len: self.listing.entries.len(),
            original_input_empty: self.launch.original_input_empty,
            has_context: self.search.custom_context.is_some(),
        };

        // Convert KeyEvent to SingleKey
        let Some(single) = SingleKey::from_event(input) else {
            return InputAction::Continue;
        };

        // --- Phase 1: Resolve (take pending key first, then immutable borrows) ---

        // Take pending key before any immutable borrows of self
        let pending = self.keymap.pending_vim_key.take();

        // If in prefix mode, try prefix keymap first (single keys only)
        let prefix_action = if self.keymap.prefix {
            let ki = KeyInput::Single(single.clone());
            self.keymap.set.prefix.resolve(&ki, &ctx)
        } else {
            None
        };

        // The if-let/else-if chain here is clearer than map_or_else with nested closures.
        #[allow(clippy::option_if_let_else)]
        let (action, new_pending) = if prefix_action.is_some() {
            (prefix_action, None)
        } else {
            // Use mode keymap (handles both single and multi-key sequences)
            let keymap = self.keymap.mode_keymap(self.tab);

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
        self.keymap.pending_vim_key = new_pending;

        // Reset prefix (before execute, so EnterPrefixMode can re-set it)
        self.keymap.prefix = false;

        if let Some(action) = action {
            self.execute_action(&action)
        } else {
            // No action matched. In insert-capable modes, insert the character.
            if self.keymap.is_insert_mode(self.tab) && !single.ctrl && !single.alt {
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
                    .listing
                    .state
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
                    .listing
                    .state
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
                    .listing
                    .state
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
                    .listing
                    .state
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
                    self.listing.select(0);
                } else {
                    let last_idx = self.listing.entries.len().saturating_sub(1);
                    self.listing.select(last_idx);
                }
                self.inspector.reset_nav();
                InputAction::Continue
            }
            Action::ScrollToBottom => {
                // Visual bottom of history
                if settings.invert {
                    let last_idx = self.listing.entries.len().saturating_sub(1);
                    self.listing.select(last_idx);
                } else {
                    self.listing.select(0);
                }
                self.inspector.reset_nav();
                InputAction::Continue
            }
            Action::ScrollToScreenTop => {
                // H — jump to top of visible screen
                let results_len = self.listing.entries.len();
                let state = self.listing.state.get_mut();
                let top = state.offset();
                let visible = state.max_entries().min(results_len);
                let bottom = top + visible.saturating_sub(1);
                state.select(bottom.min(results_len.saturating_sub(1)));
                self.inspector.reset_nav();
                InputAction::Continue
            }
            Action::ScrollToScreenMiddle => {
                // M — jump to middle of visible screen
                let results_len = self.listing.entries.len();
                let state = self.listing.state.get_mut();
                let top = state.offset();
                let visible = state.max_entries().min(results_len);
                let middle = top + visible / 2;
                state.select(middle.min(results_len.saturating_sub(1)));
                self.inspector.reset_nav();
                InputAction::Continue
            }
            Action::ScrollToScreenBottom => {
                // L — jump to bottom of visible screen
                let state = self.listing.state.get_mut();
                let top_visible = state.offset();
                state.select(top_visible);
                self.inspector.reset_nav();
                InputAction::Continue
            }

            // -- Commands --
            Action::Accept => {
                if self.tab == Tab::Inspect {
                    return InputAction::AcceptInspecting;
                }
                self.accept = true;
                InputAction::Accept(self.listing.state.get_mut().selected())
            }
            Action::AcceptNth(n) => {
                self.accept = true;
                InputAction::Accept(self.listing.state.get_mut().selected() + *n as usize)
            }
            Action::ReturnSelection => {
                if self.tab == Tab::Inspect {
                    return InputAction::AcceptInspecting;
                }
                InputAction::Accept(self.listing.state.get_mut().selected())
            }
            Action::ReturnSelectionNth(n) => {
                InputAction::Accept(self.listing.state.get_mut().selected() + *n as usize)
            }
            Action::Copy => InputAction::Copy(self.listing.state.get_mut().selected()),
            Action::Delete => InputAction::Delete(self.listing.state.get_mut().selected()),
            Action::DeleteAll => {
                InputAction::DeleteAllMatching(self.listing.state.get_mut().selected())
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
                self.query
                    .swap_engine(engines::engine(self.search_mode, settings));
                self.highlight_engine = engines::engine(self.search_mode, settings);
                InputAction::Continue
            }
            Action::SwitchContext => {
                InputAction::SwitchContext(Some(self.listing.state.get_mut().selected()))
            }
            Action::ClearContext => InputAction::SwitchContext(None),
            Action::ToggleTab => {
                self.tab = self.tab.toggle();
                InputAction::Continue
            }

            // -- Inspector --
            Action::InspectPrevious => {
                self.inspector.move_to_previous();
                InputAction::Redraw
            }
            Action::InspectNext => {
                self.inspector.move_to_next();
                InputAction::Redraw
            }

            // -- Mode changes --
            Action::VimEnterNormal => {
                self.keymap.set_cursor(self.settings, "vim_normal");
                self.keymap.mode = KeymapMode::VimNormal;
                InputAction::Continue
            }
            Action::VimEnterInsert => {
                self.keymap.set_cursor(self.settings, "vim_insert");
                self.keymap.mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAfter => {
                self.search.input.right();
                self.keymap.set_cursor(self.settings, "vim_insert");
                self.keymap.mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAtStart => {
                self.search.input.start();
                self.keymap.set_cursor(self.settings, "vim_insert");
                self.keymap.mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimEnterInsertAtEnd => {
                self.search.input.end();
                self.keymap.set_cursor(self.settings, "vim_insert");
                self.keymap.mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimSearchInsert => {
                self.search.input.clear();
                self.keymap.set_cursor(self.settings, "vim_insert");
                self.keymap.mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::VimChangeToEnd => {
                self.search.input.clear_to_end();
                self.keymap.set_cursor(self.settings, "vim_insert");
                self.keymap.mode = KeymapMode::VimInsert;
                InputAction::Continue
            }
            Action::EnterPrefixMode => {
                self.keymap.prefix = true;
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
                let output = match &self.inspector.entry {
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
                let output = match self.listing.entries.get(index) {
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
                if let Some(entry) = self.listing.entries.get(index)
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
        if self.listing.entries.is_empty() || index >= self.listing.entries.len() {
            return;
        }
        let state = self.listing.state.get_mut();
        let selected = state.selected();
        if selected == self.listing.entries.len() - 1 {
            state.select(selected.saturating_sub(1));
        }
        let entry = self.listing.entries.edit(|entries| entries.remove(index));
        self.inspector.reset_nav();
        self.tab = Tab::Search;

        let store = self.history_store.clone();
        let backend = Arc::clone(self.query.backend());
        ctx.perform(async move {
            let ids = match store.delete_entries([entry]).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!(?e, "failed to delete history entry");
                    return Msg::Requery;
                }
            };
            // The tombstone is persisted from here on: the optimistic
            // removal is the truth, and requerying would resurrect the
            // entry from a stale sqlite index (it would then vanish again
            // on the next successful rebuild). Retry once — the likely
            // failure is a transient SQLITE_BUSY from the daemon — then
            // accept the stale index; any later incremental build heals it.
            let backend = backend.lock().await;
            if let Err(e) = store.build_all(&*backend.db, &ids).await {
                tracing::warn!(?e, "history rebuild after delete failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Err(e) = store.build_all(&*backend.db, &ids).await {
                    tracing::error!(?e, "history rebuild after delete failed; index is stale");
                }
            }
            Msg::OpDone
        })
        .detach();
    }

    // See delete_single for why the guard spans the awaits.
    #[allow(clippy::significant_drop_tightening)]
    fn delete_all_matching(&mut self, index: usize, ctx: &mut Ctx<'_, Self>) {
        if self.listing.entries.is_empty() || index >= self.listing.entries.len() {
            return;
        }
        let command = self.listing.entries[index].command.clone();

        // Remove matching entries from the visible results
        self.listing
            .entries
            .edit(|entries| entries.retain(|e| e.command != command));
        self.listing.reset_window();
        self.inspector.reset_nav();
        self.tab = Tab::Search;

        // Query the DB for ALL entries with this command and delete them
        let store = self.history_store.clone();
        let backend = Arc::clone(self.query.backend());
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
                    return Msg::Requery;
                }
            };
            let ids = match store.delete_entries(all_matching).await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::error!(?e, "failed to delete history entries");
                    return Msg::Requery;
                }
            };
            // Tombstones persisted: keep the optimistic state (see
            // delete_single); retry once for transient sqlite contention.
            if let Err(e) = store.build_all(&*backend.db, &ids).await {
                tracing::warn!(?e, "history rebuild after delete-all failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Err(e) = store.build_all(&*backend.db, &ids).await {
                    tracing::error!(
                        ?e,
                        "history rebuild after delete-all failed; index is stale"
                    );
                }
            }
            Msg::OpDone
        })
        .detach();
    }

    fn sync_inspector(&mut self, ctx: &mut Ctx<'_, Self>) {
        let selected = self.listing.selected();
        self.inspector.sync(
            ctx,
            self.query.backend(),
            self.tab,
            &self.listing.entries,
            selected,
        );
    }

    fn switch_context(&mut self, selected: Option<usize>) {
        if let Some(index) = selected
            && let Some(entry) = self.listing.entries.get(index)
        {
            self.search.custom_context = Some(entry.id.clone());
            self.search.context = Context::from_history(entry);
            self.search.filter_mode = FilterMode::Session;
            self.search.input = Cursor::from(String::new());
            self.listing.reset_window();
        } else {
            self.search.custom_context = None;
            self.search.context = self.launch.initial_context.clone();
            self.search.filter_mode = self.launch.default_filter_mode;
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
        let backend = Arc::clone(self.query.backend());
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
                // `accept` rejects results superseded by a newer query or
                // by a local edit (a delete) since the query started.
                if self.listing.entries.accept(generation, results) {
                    // New results reset the selection, matching query_results.
                    self.listing.select(0);
                    self.inspector.reset_nav();
                    // In custom context mode with no filter, highlight the
                    // entry that was used to enter the context.
                    if self.listing.highlight_context_anchor {
                        self.listing.highlight_context_anchor = false;
                        if let Some(id) = self.search.custom_context.clone()
                            && let Some(pos) = self.listing.entries.iter().position(|e| e.id == id)
                        {
                            self.listing.select(pos);
                        }
                    }
                }
            }
            Msg::Inspected { entry, stats } => {
                if self.tab == Tab::Inspect {
                    self.inspector.apply(*entry, *stats);
                }
            }
            Msg::HistoryCount(count) => self.status.history_count = Some(count),
            Msg::Resize { height } => {
                // Inline frames are a fixed height; only fullscreen tracks
                // the terminal.
                self.viewport.on_resize(height);
            }
            Msg::UpdateNeeded(version) => self.status.update_needed = version,
            Msg::OpDone => {}
            Msg::Requery => self.spawn_query(ctx),
            Msg::Raw(event) => {
                // The old event loop re-queried when an input pass changed
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
                    self.listing.highlight_context_anchor = self.search.custom_context.is_some()
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
        self.keymap.eye_cursor_style()
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
        app.keymap.mode = keymap_mode;
        app.launch.original_input_empty = false;
        app.listing
            .entries
            .edit(|entries| *entries = dummy_results(results_len));
        app.listing.select(selected);
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
            app.keymap.set = KeymapSet::defaults(&settings);
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
        assert_eq!(app.keymap.pending_vim_key, Some('g'));
        assert_eq!(app.listing.selected(), 50);

        assert!(matches!(app.handle_key_input(&g), InputAction::Continue));
        assert_eq!(app.keymap.pending_vim_key, None);
        assert_eq!(app.listing.selected(), 99);
    }

    #[tokio::test]
    async fn vim_g_key_clears_on_other_input() {
        let mut app = test_app(KeymapMode::VimNormal, 100, 50, "", Settings::utc()).await;

        app.handle_key_input(&key(KeyCode::Char('g')));
        assert_eq!(app.keymap.pending_vim_key, Some('g'));

        app.handle_key_input(&key(KeyCode::Char('j')));
        assert_eq!(app.keymap.pending_vim_key, None);
    }

    #[tokio::test]
    async fn vim_big_g_jump_to_bottom() {
        let mut app = test_app(KeymapMode::VimNormal, 100, 50, "", Settings::utc()).await;

        assert!(matches!(
            app.handle_key_input(&key(KeyCode::Char('G'))),
            InputAction::Continue
        ));
        assert_eq!(app.listing.selected(), 0);
    }

    #[tokio::test]
    async fn vim_ctrl_scroll_clears_pending() {
        for c in ['d', 'u', 'f', 'b'] {
            let mut app = test_app(KeymapMode::VimNormal, 100, 50, "", Settings::utc()).await;
            app.keymap.pending_vim_key = Some('g');
            assert!(matches!(
                app.handle_key_input(&ctrl(c)),
                InputAction::Continue
            ));
            assert_eq!(app.keymap.pending_vim_key, None);
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
                app.listing.selected(),
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
            assert_eq!(app.keymap.mode, expected);
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
        assert!(!app.keymap.prefix);
        app.execute_action(&Action::EnterPrefixMode);
        assert!(app.keymap.prefix);
    }

    #[tokio::test]
    async fn prefix_chord_ctrl_a_c_switches_context() {
        let mut app = test_app(KeymapMode::Emacs, 100, 7, "", Settings::utc()).await;

        assert!(matches!(
            app.handle_key_input(&ctrl('a')),
            InputAction::Continue
        ));
        assert!(app.keymap.prefix, "ctrl-a should enter prefix mode");

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
            app.query.has_pending_engine(),
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
        assert_eq!(app.keymap.mode, KeymapMode::VimInsert);
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
        app.keymap.set = KeymapSet::from_settings(app.settings);

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
        assert_eq!(app.keymap.mode, KeymapMode::VimInsert);
        let mut runtime = eye_declare::Runtime::new(app, 120, 30);
        let (_bytes, exit) = runtime.handle(InputEvent::Key(key(KeyCode::Esc)));
        assert!(
            exit.is_none(),
            "esc in vim-insert must not exit, got {exit:?}"
        );
        assert_eq!(runtime.app().keymap.mode, KeymapMode::VimNormal);
    }

    /// A failed background delete asks for a requery so the list
    /// reconverges with the database.
    #[tokio::test]
    async fn requery_respawns_the_search() {
        let app = test_app(KeymapMode::Emacs, 3, 0, "", Settings::utc()).await;
        let mut rt = eye_declare::Runtime::new(app, 120, 30);
        let _ = rt.startup();
        let _ = rt.take_effects(); // init's query + background effects
        let (_bytes, exit) = rt.process(Msg::Requery);
        assert!(exit.is_none());
        assert!(
            !rt.take_effects().is_empty(),
            "Requery must spawn a fresh search effect"
        );
    }

    /// End-to-end through `Runtime` + a VTE terminal: real in-memory
    /// sqlite, effects driven to completion, frames verified by content —
    /// the CI form of the tmux A/B captures used during the migration.
    mod headless {
        use eye_declare::{Effect, Runtime};
        use eye_declare_engine::test_terminal::TestTerminal;
        use futures_util::StreamExt as _;

        use super::*;

        /// Run every queued effect to completion, feeding produced
        /// messages back through the runtime (and any effects those
        /// updates queue in turn), mirroring the async driver.
        async fn drive_effects(rt: &mut Runtime<SearchApp<'static>>, term: &mut TestTerminal) {
            loop {
                let effects = rt.take_effects();
                if effects.is_empty() {
                    break;
                }
                for effect in effects {
                    let Effect::Spawn { mut stream, .. } = effect;
                    while let Some(msg) = stream.next().await {
                        let (bytes, _) = rt.process(msg);
                        term.feed(&bytes);
                    }
                }
            }
        }

        #[allow(clippy::significant_drop_tightening, clippy::cast_possible_wrap)]
        async fn seeded_session() -> (Runtime<SearchApp<'static>>, TestTerminal) {
            let app = test_app(KeymapMode::Emacs, 0, 0, "", Settings::utc()).await;
            {
                let backend = Arc::clone(app.query.backend());
                let backend = backend.lock().await;
                for (i, cmd) in ["ls -la", "git status", "cargo build", "echo hi"]
                    .iter()
                    .enumerate()
                {
                    let entry: History = History::capture()
                        .timestamp(OffsetDateTime::now_utc() - time::Duration::seconds(i as i64))
                        .command((*cmd).to_string())
                        .cwd("/")
                        .build()
                        .into();
                    backend.db.save(&entry).await.unwrap();
                }
            }
            let mut rt = Runtime::new(app, 120, 30);
            let mut term = TestTerminal::new(120, 30);
            let (bytes, exit) = rt.startup();
            assert!(exit.is_none());
            term.feed(&bytes);
            drive_effects(&mut rt, &mut term).await;
            (rt, term)
        }

        fn screen(term: &TestTerminal) -> String {
            term.viewport_lines().join("\n")
        }

        #[tokio::test]
        async fn browse_renders_the_full_layout() {
            let (_rt, term) = seeded_session().await;
            let screen = screen(&term);
            assert!(screen.contains("Atuin v"), "header title\n{screen}");
            assert!(screen.contains("Search"), "tabs row\n{screen}");
            assert!(screen.contains("GLOBAL"), "filter-mode block\n{screen}");
            assert!(
                screen.contains("history count: 4"),
                "count effect\n{screen}"
            );
            // Bottom-anchored: the newest entry carries the indicator.
            let selected = term
                .viewport_lines()
                .into_iter()
                .find(|l| l.contains("ls -la"))
                .expect("newest entry rendered");
            assert!(
                selected.trim_start().starts_with('>'),
                "indicator: {selected}"
            );
        }

        #[tokio::test]
        async fn typing_filters_through_the_real_engine() {
            let (mut rt, mut term) = seeded_session().await;
            for c in "car".chars() {
                let (bytes, exit) = rt.handle(InputEvent::Key(key(KeyCode::Char(c))));
                assert!(exit.is_none());
                term.feed(&bytes);
            }
            drive_effects(&mut rt, &mut term).await;
            let screen = screen(&term);
            assert!(screen.contains("cargo build"), "fuzzy match\n{screen}");
            assert!(
                !screen.contains("git status"),
                "non-matches filtered\n{screen}"
            );
        }

        #[tokio::test]
        async fn inspector_tab_round_trip() {
            let (mut rt, mut term) = seeded_session().await;
            let (bytes, _) = rt.handle(InputEvent::Key(ctrl('o')));
            term.feed(&bytes);
            drive_effects(&mut rt, &mut term).await;
            let screen_now = screen(&term);
            assert!(
                screen_now.contains("Command stats"),
                "inspector\n{screen_now}"
            );
            assert!(
                screen_now.contains("Exit code distribution"),
                "stats charts\n{screen_now}"
            );

            let (bytes, _) = rt.handle(InputEvent::Key(ctrl('o')));
            term.feed(&bytes);
            drive_effects(&mut rt, &mut term).await;
            assert!(screen(&term).contains("GLOBAL"), "back to search");
        }

        /// A query still in flight when the user deletes must not
        /// resurrect the deleted entry when its results finally arrive:
        /// the optimistic removal advances the listing's generation, so
        /// `accept` drops them.
        #[tokio::test]
        async fn deletion_survives_a_stale_in_flight_query() {
            let (mut rt, mut term) = seeded_session().await;

            // Start a query and hold its effect: it is now "in flight".
            let (bytes, _) = rt.handle(InputEvent::Key(key(KeyCode::Char('l'))));
            term.feed(&bytes);
            let stale_query = rt.take_effects();

            // Delete the selected entry ("ls -la") through the inspector
            // while that query has not yet delivered.
            let (bytes, _) = rt.handle(InputEvent::Key(ctrl('o')));
            term.feed(&bytes);
            let (bytes, _) = rt.handle(InputEvent::Key(ctrl('d')));
            term.feed(&bytes);

            // Deliver the stale results first: they were computed before
            // the delete tombstone reached the index, so they still
            // contain "ls -la".
            for effect in stale_query {
                let Effect::Spawn { mut stream, .. } = effect;
                while let Some(msg) = stream.next().await {
                    let (bytes, _) = rt.process(msg);
                    term.feed(&bytes);
                }
            }
            // Then let the delete (and inspector) effects finish.
            drive_effects(&mut rt, &mut term).await;

            let screen = screen(&term);
            assert!(
                !screen.contains("ls -la"),
                "stale results must not resurrect the deleted entry\n{screen}"
            );
            assert!(
                screen.contains("git status"),
                "unrelated entries remain\n{screen}"
            );
        }

        #[tokio::test]
        async fn exit_reclaims_the_region() {
            let (mut rt, mut term) = seeded_session().await;
            let (bytes, exit) = rt.handle(InputEvent::Key(key(KeyCode::Esc)));
            assert_eq!(exit, Some(Output::ReturnOriginal));
            term.feed(&bytes);
            let screen = screen(&term);
            assert!(
                !screen.contains("Atuin v") && !screen.contains("GLOBAL"),
                "region must be fully reclaimed on exit\n{screen}"
            );
        }
    }
}
