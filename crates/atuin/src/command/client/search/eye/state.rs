//! The search app's state, grouped by concern: each struct owns one slice
//! of the model and the methods that only touch that slice. `SearchApp`
//! composes them and keeps the logic that genuinely coordinates across
//! concerns (the keybinding pipeline, action dispatch, deletes).

use std::cell::RefCell;
use std::sync::Arc;

use atuin_client::database::{Context, Database};
use atuin_client::history::{History, HistoryId, HistoryStats};
use atuin_client::settings::{CursorStyle as CfgCursorStyle, FilterMode, KeymapMode, Settings};
use eye_declare::{Ctx, CursorStyle, Task};
use semver::Version;
use tokio::sync::Mutex;

use super::super::cursor::Cursor;
use super::super::engines::{AnySearchEngine, SearchEngine, SearchState};
use super::super::history_list::ListState;
use super::super::interactive::InspectingState;
use super::super::keybindings::{Keymap, KeymapSet};
use super::app::{Msg, SearchApp};

/// Which tab is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tab {
    Search,
    Inspect,
}

impl Tab {
    pub fn toggle(self) -> Self {
        match self {
            Tab::Search => Tab::Inspect,
            Tab::Inspect => Tab::Search,
        }
    }

    /// The index the ratatui `Tabs` widget highlights.
    pub fn index(self) -> usize {
        match self {
            Tab::Search => 0,
            Tab::Inspect => 1,
        }
    }
}

/// The result list: entries plus the windowed selection state the
/// `HistoryList` widget scrolls. The `RefCell` exists because rendering
/// updates the scroll window (offset, visible count) behind `&self`;
/// update code uses `get_mut` and pays no runtime check.
pub(super) struct Listing {
    pub entries: Vec<History>,
    pub state: RefCell<ListState>,
    /// Set when entering/leaving a custom context with an empty input: the
    /// next results delivery re-selects the context's anchor entry.
    pub highlight_context_anchor: bool,
}

impl Listing {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            state: RefCell::new(ListState::default()),
            highlight_context_anchor: false,
        }
    }

    pub fn selected(&mut self) -> usize {
        self.state.get_mut().selected()
    }

    pub fn select(&mut self, index: usize) {
        self.state.get_mut().select(index);
    }

    /// Move the selection toward older entries (visually up when the list
    /// is bottom-anchored), clamped to the end.
    pub fn scroll_up(&mut self, scroll_len: usize) {
        let len = self.entries.len();
        let state = self.state.get_mut();
        let i = state.selected() + scroll_len;
        state.select(i.min(len.saturating_sub(1)));
    }

    /// Move the selection toward newer entries.
    pub fn scroll_down(&mut self, scroll_len: usize) {
        let state = self.state.get_mut();
        let i = state.selected().saturating_sub(scroll_len);
        state.select(i);
    }

    /// Forget the scroll window entirely (e.g. after a bulk delete).
    pub fn reset_window(&mut self) {
        *self.state.get_mut() = ListState::default();
    }
}

/// Viewport geometry: the frame's height, and whether it tracks the
/// terminal (fullscreen) or stays at the configured inline height.
pub(super) struct Viewport {
    pub height: u16,
    pub fullscreen: bool,
}

impl Viewport {
    pub fn on_resize(&mut self, height: u16) {
        // Inline frames are a fixed height; only fullscreen tracks the
        // terminal.
        if self.fullscreen {
            self.height = height;
        }
    }
}

/// Keymap-driven input modes: which keymap is live, vim/prefix pending
/// state, and the hardware cursor shape those modes request.
pub(super) struct KeymapState {
    pub mode: KeymapMode,
    pub prefix: bool,
    pub pending_vim_key: Option<char>,
    pub set: KeymapSet,
    /// The keymap-driven cursor shape currently in effect; `None` until a
    /// configured shape first applies (so setups with no `keymap_cursor`
    /// config never touch the terminal's cursor shape).
    pub cursor: Option<CfgCursorStyle>,
}

impl KeymapState {
    pub fn new(settings: &Settings) -> Self {
        let mut this = Self {
            mode: match settings.keymap_mode {
                KeymapMode::Auto => KeymapMode::Emacs,
                value => value,
            },
            prefix: false,
            pending_vim_key: None,
            set: KeymapSet::from_settings(settings),
            cursor: None,
        };
        this.initialize_cursor(settings);
        this
    }

    /// The keymap for the current mode (ignoring prefix).
    pub fn mode_keymap(&self, tab: Tab) -> &Keymap {
        if tab == Tab::Inspect {
            return &self.set.inspector;
        }
        match self.mode {
            KeymapMode::Emacs | KeymapMode::Auto => &self.set.emacs,
            KeymapMode::VimNormal => &self.set.vim_normal,
            KeymapMode::VimInsert => &self.set.vim_insert,
        }
    }

    /// Whether the current mode supports character insertion on unmatched
    /// keys. The inspector tab has no text input, so unmatched keys are
    /// dropped there rather than leaking into the (hidden) search input.
    pub fn is_insert_mode(&self, tab: Tab) -> bool {
        tab == Tab::Search
            && matches!(
                self.mode,
                KeymapMode::Emacs | KeymapMode::Auto | KeymapMode::VimInsert
            )
    }

    pub fn set_cursor(&mut self, settings: &Settings, keymap_name: &str) {
        let cursor_style = if keymap_name == "__clear__" {
            None
        } else {
            settings.keymap_cursor.get(keymap_name).copied()
        }
        .or_else(|| self.cursor.map(|_| CfgCursorStyle::DefaultUserShape));

        if cursor_style != self.cursor && cursor_style.is_some() {
            self.cursor = cursor_style;
        }
    }

    fn initialize_cursor(&mut self, settings: &Settings) {
        match self.mode {
            KeymapMode::Emacs => self.set_cursor(settings, "emacs"),
            KeymapMode::VimNormal => self.set_cursor(settings, "vim_normal"),
            KeymapMode::VimInsert => self.set_cursor(settings, "vim_insert"),
            KeymapMode::Auto => {}
        }
    }

    /// The shell gets the shape configured for its keymap mode; the final
    /// (exit) present emits it.
    pub fn finalize_cursor(&mut self, settings: &Settings) {
        match settings.keymap_mode_shell {
            KeymapMode::Emacs => self.set_cursor(settings, "emacs"),
            KeymapMode::VimNormal => self.set_cursor(settings, "vim_normal"),
            KeymapMode::VimInsert => self.set_cursor(settings, "vim_insert"),
            KeymapMode::Auto => self.set_cursor(settings, "__clear__"),
        }
    }

    /// The shape for eye-declare's `App::cursor_style`.
    pub fn eye_cursor_style(&self) -> CursorStyle {
        match self.cursor {
            None | Some(CfgCursorStyle::DefaultUserShape) => CursorStyle::DefaultUserShape,
            Some(CfgCursorStyle::BlinkingBlock) => CursorStyle::BlinkingBlock,
            Some(CfgCursorStyle::SteadyBlock) => CursorStyle::SteadyBlock,
            Some(CfgCursorStyle::BlinkingUnderScore) => CursorStyle::BlinkingUnderScore,
            Some(CfgCursorStyle::SteadyUnderScore) => CursorStyle::SteadyUnderScore,
            Some(CfgCursorStyle::BlinkingBar) => CursorStyle::BlinkingBar,
            Some(CfgCursorStyle::SteadyBar) => CursorStyle::SteadyBar,
        }
    }
}

/// Engine + database behind one lock so query effects serialize.
pub(super) struct QueryBackend {
    pub engine: AnySearchEngine,
    pub db: Box<dyn Database>,
}

/// The async search machinery: the backend lock, the generation guard
/// that drops results a newer keystroke superseded, the cancel-on-drop
/// task slot, and the engine swap requested by `CycleSearchMode`.
pub(super) struct Querying {
    backend: Arc<Mutex<QueryBackend>>,
    generation: u64,
    task: Option<Task>,
    pending_engine: Option<AnySearchEngine>,
}

impl Querying {
    pub fn new(engine: AnySearchEngine, db: Box<dyn Database>) -> Self {
        Self {
            backend: Arc::new(Mutex::new(QueryBackend { engine, db })),
            generation: 0,
            task: None,
            pending_engine: None,
        }
    }

    /// The shared backend, for effects that need db access (deletes, the
    /// inspector fetch) and must serialize against in-flight queries.
    pub fn backend(&self) -> &Arc<Mutex<QueryBackend>> {
        &self.backend
    }

    /// Whether a delivered result set is from the newest query.
    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    /// `CycleSearchMode` can't replace the engine synchronously (it lives
    /// behind the async lock); the next spawned query installs it.
    pub fn swap_engine(&mut self, engine: AnySearchEngine) {
        self.pending_engine = Some(engine);
    }

    // The lock is deliberately held across the query await: it serializes
    // engine+db access so a stale query can't interleave with a fresh one.
    #[allow(clippy::significant_drop_tightening)]
    pub fn spawn(
        &mut self,
        ctx: &mut Ctx<'_, SearchApp<'_>>,
        state: SearchState,
        smart_sort: bool,
    ) {
        self.generation += 1;
        let generation = self.generation;
        let backend = Arc::clone(&self.backend);
        let new_engine = self.pending_engine.take();
        // Replacing the task drops (cancels) the previous query.
        self.task = Some(ctx.perform(async move {
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
}

#[cfg(test)]
impl Querying {
    pub fn has_pending_engine(&self) -> bool {
        self.pending_engine.is_some()
    }
}

/// The inspector tab's data and its fetch machinery.
pub(super) struct Inspector {
    /// The explicitly inspected entry, loaded by id; `None` means "the
    /// selected result".
    pub entry: Option<History>,
    pub stats: Option<HistoryStats>,
    /// The entry id `stats` was computed for, so the inspector only hits
    /// the database when the inspected entry actually changes.
    stats_for: Option<HistoryId>,
    nav: InspectingState,
    task: Option<Task>,
}

impl Inspector {
    pub fn new() -> Self {
        Self {
            entry: None,
            stats: None,
            stats_for: None,
            nav: InspectingState {
                current: None,
                next: None,
                previous: None,
            },
            task: None,
        }
    }

    /// Forget the navigation anchor (the selection moved).
    pub fn reset_nav(&mut self) {
        self.nav.reset();
    }

    pub fn move_to_previous(&mut self) {
        self.nav.move_to_previous();
    }

    pub fn move_to_next(&mut self) {
        self.nav.move_to_next();
    }

    /// Keep the inspected entry + stats in sync with the model: when the
    /// inspector tab is showing, fetch whenever the target entry (the
    /// explicitly inspected id, else the selection) differs from what
    /// `stats` was computed for. Runs after every update; the resulting
    /// `Msg::Inspected` re-enters it, so a target that moved mid-fetch
    /// converges on the next pass. Mirrors the per-iteration stats block
    /// of the ratatui event loop.
    pub fn sync(
        &mut self,
        ctx: &mut Ctx<'_, SearchApp<'_>>,
        backend: &Arc<Mutex<QueryBackend>>,
        tab: Tab,
        entries: &[History],
        selected: usize,
    ) {
        if tab != Tab::Inspect || entries.is_empty() {
            self.stats = None;
            self.stats_for = None;
            return;
        }
        let fallback = entries[selected.min(entries.len() - 1)].clone();
        let inspected = self.nav.current.clone();
        let target = inspected.clone().unwrap_or_else(|| fallback.id.clone());
        if self.stats_for.as_ref() == Some(&target) {
            return;
        }
        let backend = Arc::clone(backend);
        // Replacing the task cancels a fetch for a stale target.
        self.task = Some(ctx.perform(async move {
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

    /// A fetch arrived: adopt the entry, its stats, and the prev/next ids
    /// they establish for navigation.
    pub fn apply(&mut self, entry: History, stats: HistoryStats) {
        self.nav.current = Some(entry.id.clone());
        self.nav.previous = stats.previous.as_ref().map(|p| p.id.clone());
        self.nav.next = stats.next.as_ref().map(|n| n.id.clone());
        self.stats_for = Some(entry.id.clone());
        self.entry = Some(entry);
        self.stats = Some(stats);
    }
}

/// Values fixed when the search was launched.
pub(super) struct Launch {
    pub initial_context: Context,
    pub default_filter_mode: FilterMode,
    pub original_input_empty: bool,
}

/// Background header info, each delivered by its own startup effect.
#[derive(Default)]
pub(super) struct Status {
    pub history_count: Option<i64>,
    pub update_needed: Option<Version>,
}

/// Snapshot the query-relevant fields for a spawned search.
/// `SearchState` isn't `Clone` (`Cursor`), hence the field-wise copy.
pub(super) fn snapshot(search: &SearchState) -> SearchState {
    SearchState {
        input: Cursor::from(search.input.as_str().to_owned()),
        filter_mode: search.filter_mode,
        context: search.context.clone(),
        custom_context: search.custom_context.clone(),
        shells: search.shells.clone(),
    }
}
