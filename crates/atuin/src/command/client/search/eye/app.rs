//! The Elm-shaped search application.
//!
//! Hardcoded emacs-style keys via keymap fallthrough, live queries as
//! cancel-on-drop effects, and a fixed-height frame element for rendering.
//! The real keybinding pipeline (`KeymapSet` + conditions) arrives in P2.

use std::cell::RefCell;
use std::sync::Arc;

use atuin_client::database::{Context, Database};
use atuin_client::history::History;
use atuin_client::settings::{FilterMode, Settings};
use atuin_client::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eye_declare::{
    App, Ctx, Element, ElementExt, Focus, FocusHandle, InputEvent, Keymap, Task, empty, keymap,
};
use time::OffsetDateTime;
use tokio::sync::Mutex;

use super::super::cursor::Cursor;
use super::super::engines::{AnySearchEngine, SearchEngine, SearchState};
use super::super::history_list::ListState;
use super::view::SearchFrame;

/// What the run loop hands back to `eye::history` for resolution into the
/// final shell-facing string. `Default` (stdin closing, driver teardown)
/// deliberately maps to "keep the original command line".
#[derive(Debug, Default)]
pub(super) enum Output {
    #[default]
    ReturnOriginal,
    /// Return the current search input to the command line.
    ReturnQuery(String),
    /// A selected history entry; `execute` distinguishes accept-and-run
    /// (Enter with `enter_accept`) from return-to-command-line (Tab).
    Selection { command: String, execute: bool },
}

#[derive(Clone)]
pub(super) enum Msg {
    Raw(InputEvent),
    Results {
        generation: u64,
        results: Vec<History>,
    },
}

/// Engine + database behind one lock so query effects serialize; the
/// generation counter drops results that a newer keystroke superseded.
struct QueryBackend {
    engine: AnySearchEngine,
    db: Box<dyn Database>,
}

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
    pub(super) inline_height: u16,
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
        context: Context,
        filter_mode: FilterMode,
        inline_height: u16,
    ) -> Self {
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
                context,
                custom_context: None,
                shells: settings.search.shells.clone(),
            },
            results: Vec::new(),
            results_state: RefCell::new(ListState::default()),
            highlight_engine,
            now,
            inline_height,
            generation: 0,
            backend: Arc::new(Mutex::new(QueryBackend { engine, db })),
            query_task: None,
            exiting: false,
            _focus: focus,
            input_focus,
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
        // prompt returns to where the search UI appeared.
        self.exiting = true;
        ctx.exit(output);
    }

    fn accept(&mut self, execute: bool, ctx: &mut Ctx<'_, Self>) {
        let selected = self.results_state.get_mut().selected();
        let output = match self.results.get(selected) {
            Some(entry) => Output::Selection {
                command: entry.command.clone(),
                execute,
            },
            None => Output::ReturnQuery(self.search.input.as_str().to_owned()),
        };
        self.finish(output, ctx);
    }

    /// Move the selection toward older entries (visually up when the list
    /// is bottom-anchored).
    fn scroll_up(&mut self, n: usize) {
        let len = self.results.len();
        let state = self.results_state.get_mut();
        let i = state.selected() + n;
        state.select(i.min(len.saturating_sub(1)));
    }

    /// Move the selection toward newer entries.
    fn scroll_down(&mut self, n: usize) {
        let state = self.results_state.get_mut();
        let i = state.selected().saturating_sub(n);
        state.select(i);
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx<'_, Self>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let invert = self.settings.invert;
        match key.code {
            KeyCode::Esc => self.finish(Output::ReturnOriginal, ctx),
            KeyCode::Char('c' | 'd' | 'g') if ctrl => self.finish(Output::ReturnOriginal, ctx),
            KeyCode::Enter => self.accept(self.settings.enter_accept, ctx),
            KeyCode::Tab => self.accept(false, ctx),
            KeyCode::Up => {
                if invert {
                    self.scroll_down(1);
                } else {
                    self.scroll_up(1);
                }
            }
            KeyCode::Down => {
                if invert {
                    self.scroll_up(1);
                } else {
                    self.scroll_down(1);
                }
            }
            KeyCode::Backspace => {
                self.search.input.back();
                self.spawn_query(ctx);
            }
            KeyCode::Left => {
                self.search.input.left();
            }
            KeyCode::Right => self.search.input.right(),
            KeyCode::Home => self.search.input.start(),
            KeyCode::End => self.search.input.end(),
            KeyCode::Char('a') if ctrl => self.search.input.start(),
            KeyCode::Char('e') if ctrl => self.search.input.end(),
            KeyCode::Char('u') if ctrl => {
                self.search.input.clear();
                self.spawn_query(ctx);
            }
            KeyCode::Char(c) if !ctrl => {
                self.search.input.insert(c);
                self.spawn_query(ctx);
            }
            _ => {}
        }
    }
}

impl App for SearchApp<'_> {
    type Msg = Msg;
    type Output = Output;

    fn init(&mut self, ctx: &mut Ctx<'_, Self>) {
        self.spawn_query(ctx);
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
                }
            }
            Msg::Raw(InputEvent::Key(key)) => self.handle_key(key, ctx),
            Msg::Raw(InputEvent::Paste(text)) => {
                for c in text.chars() {
                    self.search.input.insert(c);
                }
                self.spawn_query(ctx);
            }
        }
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
}
