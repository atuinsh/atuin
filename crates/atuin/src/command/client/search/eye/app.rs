//! The Elm-shaped search application.
//!
//! P0 skeleton: hardcoded emacs-style keys via keymap fallthrough, live
//! queries as cancel-on-drop effects, height-clamped bottom-anchored list.
//! The real keybinding pipeline (`KeymapSet` + conditions) arrives in P2.

use std::sync::Arc;

use atuin_client::database::{Context, Database};
use atuin_client::history::History;
use atuin_client::settings::{FilterMode, Settings};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eye_declare::{
    App, Ctx, Element, ElementExt, Focus, FocusHandle, InputEvent, Keymap, Task, col, empty, keymap,
};
use tokio::sync::Mutex;

use super::super::cursor::Cursor;
use super::super::engines::{AnySearchEngine, SearchEngine, SearchState};
use super::view::{InputLine, ResultsList};

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

pub(super) struct SearchApp {
    search: SearchState,
    results: Vec<History>,
    selected: usize,
    generation: u64,
    backend: Arc<Mutex<QueryBackend>>,
    query_task: Option<Task>,
    inline_height: u16,
    smart_sort: bool,
    execute_on_enter: bool,
    exiting: bool,
    _focus: Focus,
    input_focus: FocusHandle,
}

impl SearchApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        search_input: String,
        settings: &Settings,
        db: Box<dyn Database>,
        engine: AnySearchEngine,
        context: Context,
        filter_mode: FilterMode,
        inline_height: u16,
    ) -> Self {
        let mut input = Cursor::from(search_input);
        input.end();

        let focus = Focus::new();
        let input_focus = focus.handle();
        input_focus.focus();

        Self {
            search: SearchState {
                input,
                filter_mode,
                context,
                custom_context: None,
                shells: settings.search.shells.clone(),
            },
            results: Vec::new(),
            selected: 0,
            generation: 0,
            backend: Arc::new(Mutex::new(QueryBackend { engine, db })),
            query_task: None,
            inline_height,
            smart_sort: settings.smart_sort,
            execute_on_enter: settings.enter_accept,
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
        let smart_sort = self.smart_sort;
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
        let output = match self.results.get(self.selected) {
            Some(entry) => Output::Selection {
                command: entry.command.clone(),
                execute,
            },
            None => Output::ReturnQuery(self.search.input.as_str().to_owned()),
        };
        self.finish(output, ctx);
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &mut Ctx<'_, Self>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.finish(Output::ReturnOriginal, ctx),
            KeyCode::Char('c' | 'd' | 'g') if ctrl => self.finish(Output::ReturnOriginal, ctx),
            KeyCode::Enter => self.accept(self.execute_on_enter, ctx),
            KeyCode::Tab => self.accept(false, ctx),
            KeyCode::Up => {
                self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1));
            }
            KeyCode::Down => self.selected = self.selected.saturating_sub(1),
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

impl App for SearchApp {
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
                    self.selected = 0;
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
        col()
            .child(ResultsList {
                results: &self.results,
                selected: self.selected,
                rows: self.inline_height.saturating_sub(1),
            })
            .child(InputLine {
                input: &self.search.input,
            })
            .any()
    }

    fn keymap(&self) -> Keymap<Msg> {
        keymap().fallthrough(&self.input_focus, Msg::Raw)
    }
}
