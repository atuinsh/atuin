use std::{
    ops::{ControlFlow, Deref},
    sync::Arc,
};

use eyre::Result;
use semver::Version;

use atuin_client::{
    database::Context,
    database::{current_context, Database},
    history::History,
    settings::{ExitMode, FilterMode, SearchMode, Settings},
};
use skim::SkimItem;

use super::{cursor::Cursor, history_list::ListState};

pub struct State<DB: Database> {
    pub db: DB,
    pub results_state: ListState,
    pub history: Vec<Arc<HistoryWrapper>>,
    pub history_count: i64,
    pub settings: Settings,
    pub update_needed: Option<Version>,

    pub search: SearchState,

    // only allocated if using skim
    pub all_history: Vec<Arc<HistoryWrapper>>,
}

pub struct SearchState {
    pub input: Cursor,
    pub filter_mode: FilterMode,
    pub search_mode: SearchMode,
    /// Store if the user has _just_ changed the search mode.
    /// If so, we change the UI to show the search mode instead
    /// of the filter mode until user starts typing again.
    switched_search_mode: bool,
    pub context: Context,
}

pub struct HistoryWrapper {
    history: History,
    pub count: i32,
}
impl Deref for HistoryWrapper {
    type Target = History;

    fn deref(&self) -> &Self::Target {
        &self.history
    }
}
impl SkimItem for HistoryWrapper {
    fn text(&self) -> std::borrow::Cow<str> {
        std::borrow::Cow::Borrowed(self.history.command.as_str())
    }
}

pub struct BatchGuard<DB: Database> {
    initial_input: String,
    initial_filter_mode: FilterMode,
    inner: State<DB>,
}

#[derive(Clone)]
pub enum Line {
    Up,
    Down,
}
#[derive(Clone)]
pub enum For {
    Page,
    SingleLine,
}
#[derive(Clone)]
pub enum Towards {
    Left,
    Right,
}
#[derive(Clone)]
pub enum To {
    Word,
    Char,
    Edge,
}

#[derive(Clone)]
pub enum Event {
    Input(char),
    InputStr(String),
    Selection(Line, For),
    Cursor(Towards, To),
    Delete(Towards, To),
    Clear,
    Exit,
    UpdateNeeded(Version),
    Cancel,
    SelectN(u32),
    CycleFilterMode,
    CycleSearchMode,
}

impl<DB: Database> State<DB> {
    /// Create a new core state
    pub async fn new(query: &[String], settings: Settings, db: DB) -> Result<Self> {
        let mut input = Cursor::from(query.join(" "));
        // Put the cursor at the end of the query by default
        input.end();

        let mut core = Self {
            history_count: db.history_count().await?,
            results_state: ListState::default(),
            update_needed: None,
            history: Vec::new(),
            db,
            all_history: Vec::new(),
            search: SearchState {
                input,
                context: current_context(),
                filter_mode: if settings.shell_up_key_binding {
                    settings
                        .filter_mode_shell_up_key_binding
                        .unwrap_or(settings.filter_mode)
                } else {
                    settings.filter_mode
                },
                search_mode: settings.search_mode,
                switched_search_mode: false,
            },
            settings,
        };
        core.refresh_query().await?;
        Ok(core)
    }

    async fn refresh_query(&mut self) -> Result<()> {
        let i = self.search.input.as_str();
        let results = if i.is_empty() {
            self.db
                .list(
                    self.search.filter_mode,
                    &self.search.context,
                    Some(200),
                    true,
                )
                .await?
                .into_iter()
                .map(|history| HistoryWrapper { history, count: 1 })
                .map(Arc::new)
                .collect::<Vec<_>>()
        } else if self.search.search_mode == SearchMode::Skim {
            if self.all_history.is_empty() {
                self.all_history = self
                    .db
                    .all_with_count()
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|(history, count)| HistoryWrapper { history, count })
                    .map(Arc::new)
                    .collect::<Vec<_>>();
            }

            super::skim_impl::fuzzy_search(&self.search, &self.all_history).await
        } else {
            self.db
                .search(
                    self.search.search_mode,
                    self.search.filter_mode,
                    &self.search.context,
                    i,
                    Some(200),
                    None,
                    None,
                )
                .await?
                .into_iter()
                .map(|history| HistoryWrapper { history, count: 1 })
                .map(Arc::new)
                .collect::<Vec<_>>()
        };

        self.results_state.select(0);
        self.history = results;
        Ok(())
    }

    fn handle(mut self, event: Event) -> ControlFlow<String, Self> {
        // reset the state, will be set to true later if user really did change it
        self.search.switched_search_mode = false;
        let len = self.history.len();
        match event {
            // moving the selection up and down
            Event::Selection(Line::Up, For::SingleLine) => {
                let i = self.results_state.selected() + 1;
                self.results_state.select(i.min(len - 1));
            }
            Event::Selection(Line::Down, For::SingleLine) => {
                let Some(i) = self.results_state.selected().checked_sub(1) else {
                    return ControlFlow::Break(String::new())
                };
                self.results_state.select(i);
            }
            Event::Selection(Line::Down, For::Page) => {
                let scroll_len =
                    self.results_state.max_entries() - self.settings.scroll_context_lines;
                let i = self.results_state.selected().saturating_sub(scroll_len);
                self.results_state.select(i);
            }
            Event::Selection(Line::Up, For::Page) => {
                let scroll_len =
                    self.results_state.max_entries() - self.settings.scroll_context_lines;
                let i = self.results_state.selected() + scroll_len;
                self.results_state.select(i.min(len - 1));
            }

            // moving the search cursor left and right
            Event::Cursor(Towards::Left, To::Char) => {
                self.search.input.left();
            }
            Event::Cursor(Towards::Right, To::Char) => self.search.input.right(),
            Event::Cursor(Towards::Left, To::Word) => self
                .search
                .input
                .prev_word(&self.settings.word_chars, self.settings.word_jump_mode),
            Event::Cursor(Towards::Right, To::Word) => self
                .search
                .input
                .next_word(&self.settings.word_chars, self.settings.word_jump_mode),
            Event::Cursor(Towards::Left, To::Edge) => self.search.input.start(),
            Event::Cursor(Towards::Right, To::Edge) => self.search.input.end(),

            // modifying the search
            Event::Input(c) => self.search.input.insert(c),
            Event::InputStr(s) => s.chars().for_each(|c| self.search.input.insert(c)),
            Event::Delete(Towards::Left, To::Word) => self
                .search
                .input
                .remove_prev_word(&self.settings.word_chars, self.settings.word_jump_mode),
            Event::Delete(Towards::Left, To::Char) => self.search.input.back(),
            Event::Delete(Towards::Left, To::Edge) => self.search.input.clear_from_start(),
            Event::Delete(Towards::Right, To::Word) => self
                .search
                .input
                .remove_next_word(&self.settings.word_chars, self.settings.word_jump_mode),
            Event::Delete(Towards::Right, To::Char) => self.search.input.remove(),
            Event::Delete(Towards::Right, To::Edge) => self.search.input.clear_to_end(),
            Event::Clear => self.search.input.clear(),

            // exiting
            Event::Cancel => return ControlFlow::Break(String::new()),
            Event::Exit => {
                return ControlFlow::Break(match self.settings.exit_mode {
                    ExitMode::ReturnOriginal => String::new(),
                    ExitMode::ReturnQuery => self.search.input.into_inner(),
                })
            }
            Event::SelectN(n) => {
                let i = self.results_state.selected().saturating_add(n as usize);
                return ControlFlow::Break(if i < self.history.len() {
                    self.search.input.into_inner()
                } else {
                    self.history.swap_remove(i).command.clone()
                });
            }

            // misc
            Event::UpdateNeeded(version) => self.update_needed = Some(version),
            Event::CycleSearchMode => {
                self.search.switched_search_mode = true;
                self.search.search_mode = self.search.search_mode.next(&self.settings);
            }
            Event::CycleFilterMode => {
                pub static FILTER_MODES: [FilterMode; 4] = [
                    FilterMode::Global,
                    FilterMode::Host,
                    FilterMode::Session,
                    FilterMode::Directory,
                ];
                let i = self.search.filter_mode as usize;
                let i = (i + 1) % FILTER_MODES.len();
                self.search.filter_mode = FILTER_MODES[i];
            }
        }
        ControlFlow::Continue(self)
    }

    /// Start a batch process of events
    pub fn start_batch(self) -> BatchGuard<DB> {
        BatchGuard {
            initial_input: self.search.input.as_str().to_owned(),
            initial_filter_mode: self.search.filter_mode,
            inner: self,
        }
    }

    /// Get the view of the state for rendering
    pub fn view(&mut self) -> View<'_> {
        View {
            history_count: self.history_count,
            search: &self.search,
            results_state: &mut self.results_state,
            update_needed: self.update_needed.as_ref(),
            history: &self.history,
        }
    }
}

impl<DB: Database> BatchGuard<DB> {
    /// Handle an event in the batch
    pub fn handle(mut self, event: Event) -> ControlFlow<String, Self> {
        match self.inner.handle(event) {
            ControlFlow::Continue(inner) => self.inner = inner,
            ControlFlow::Break(result) => return ControlFlow::Break(result),
        }
        ControlFlow::Continue(self)
    }

    /// Finish processing a batch of events, maybe refreshing the DB in the process
    pub async fn finish(self) -> Result<(State<DB>, bool)> {
        let Self {
            initial_input,
            initial_filter_mode,
            mut inner,
        } = self;
        let should_update = initial_input != inner.search.input.as_str()
            || initial_filter_mode != inner.search.filter_mode;
        if should_update {
            inner.refresh_query().await?;
        }

        Ok((inner, should_update))
    }
}

/// A view of the state for rendering
pub struct View<'a> {
    pub history_count: i64,
    pub results_state: &'a mut ListState,
    pub update_needed: Option<&'a Version>,
    pub history: &'a [Arc<HistoryWrapper>],

    pub search: &'a SearchState,
}
