use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Position},
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};

use crate::models::{HistorySource, Mode, Model};
use crate::msg::Msg;
use crate::runtime::{App, Cmd};
use crate::search::views::history_list::HistoryListView;
use crate::search::views::search_input::{MODE_INDICATOR_WIDTH, SearchInputView};
use crate::search::views::titlebar::TitleBarView;

/// The atuin turtle, embedded at compile time.
const TURTLE_PNG: &[u8] = include_bytes!("../../../assets/atuin-turtle.png");

/// Rows to load at startup, before the first render establishes the real height.
const INITIAL_WINDOW: Range<usize> = 0..128;

/// The search prompt shown before the query.
const PROMPT: &str = "> ";

/// The interactive search application, driven by the [`runtime`](crate::runtime)
/// and backed by a [`HistorySource`].
pub struct SearchInteractive<S> {
    model: Model,
    /// The turtle logo, or `None` if the terminal can't show images. Render
    /// state (its encoded image is cached per size), not model state.
    logo: Option<StatefulProtocol>,
    source: S,
}

impl<S> SearchInteractive<S> {
    pub fn new(model: Model, logo: Option<StatefulProtocol>, source: S) -> Self {
        Self {
            model,
            logo,
            source,
        }
    }
}

impl<S: HistorySource> App for SearchInteractive<S> {
    fn init(&mut self) -> Cmd {
        self.browse()
    }

    fn update(&mut self, msg: Msg) -> Cmd {
        match msg {
            Msg::Key(key) if is_ctrl_c(&key) => Cmd::Quit,
            Msg::Key(key) => self.on_key(key),
            Msg::Resize(..) => self.ensure_loaded(),
            Msg::HistoryTotal(total) => {
                if self.model.search.value().is_empty() {
                    self.model.history.set_total(total);
                    self.ensure_loaded()
                } else {
                    Cmd::None
                }
            }
            Msg::HistoryLoaded { start, rows } => {
                if self.model.search.value().is_empty() {
                    self.model.history.apply(start, rows);
                }
                Cmd::None
            }
            Msg::SearchResults { query, rows } => {
                // Ignore results for a query the user has since changed.
                if query == self.model.search.value() {
                    self.model.history.set_results(rows);
                }
                Cmd::None
            }
        }
    }

    fn view(&mut self, frame: &mut Frame<'_>) {
        let [header, body, input] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        // The list's viewport height is only known at render time (especially inline);
        // record it so `update`'s navigation and loading use the truth.
        self.model.history.set_viewport_height(body.height);

        frame.render_stateful_widget(
            TitleBarView {
                model: &self.model,
                update_available: false,
                count: Some(self.model.history.total() as u64),
            },
            header,
            &mut self.logo,
        );

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        frame.render_widget(
            HistoryListView {
                model: &self.model.history,
                theme: &self.model.theme,
                now,
            },
            body,
        );

        frame.render_widget(
            SearchInputView {
                prompt: PROMPT,
                input: &self.model.search,
                theme: &self.model.theme,
                mode: self.model.mode(),
            },
            input,
        );

        // The terminal cursor lives in the query — shown while typing (non-modal
        // or SEARCH), hidden in NORMAL (navigation, no text entry).
        let mode = self.model.mode();
        if !matches!(mode, Some(Mode::Normal { .. })) {
            let indicator = if matches!(mode, Some(Mode::Search)) {
                MODE_INDICATOR_WIDTH
            } else {
                0
            };
            let cursor_x = input.x
                + indicator
                + PROMPT.chars().count() as u16
                + self.model.search.cursor_char() as u16;
            frame.set_cursor_position(Position::new(
                cursor_x.min(input.right().saturating_sub(1)),
                input.y,
            ));
        }
    }
}

impl<S: HistorySource> SearchInteractive<S> {
    fn on_key(&mut self, key: KeyEvent) -> Cmd {
        match self.model.mode() {
            None => self.on_key_plain(key),
            Some(Mode::Search) => self.on_key_search(key),
            Some(Mode::Normal { .. }) => self.on_key_normal(key),
        }
    }

    /// Non-modal handling: Esc quits; everything else is text entry / nav.
    fn on_key_plain(&mut self, key: KeyEvent) -> Cmd {
        if key.code == KeyCode::Esc {
            return Cmd::Quit;
        }
        self.on_key_edit(key)
    }

    /// SEARCH mode: like the non-modal interface, but Esc drops to NORMAL.
    fn on_key_search(&mut self, key: KeyEvent) -> Cmd {
        if key.code == KeyCode::Esc {
            self.model.enter_normal();
            return Cmd::None;
        }
        self.on_key_edit(key)
    }

    /// Shared text-entry handling (non-modal + SEARCH): Enter accepts, printable
    /// keys edit the query, arrows/page navigate the list.
    fn on_key_edit(&mut self, key: KeyEvent) -> Cmd {
        if key.code == KeyCode::Enter {
            return Cmd::Quit; // accept (returning the command is a follow-up)
        }

        let before = self.model.search.value().to_owned();

        // The list is inverted (newest at the bottom): up = older, down = newer.
        match key.code {
            KeyCode::Up => self.model.history.select_next(),
            KeyCode::Down => self.model.history.select_prev(),
            KeyCode::PageUp => self.model.history.page_down(),
            KeyCode::PageDown => self.model.history.page_up(),
            // Editing keys act on the query. Ctrl/Alt combos are shortcuts, not text.
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.model.search.insert(c)
            }
            KeyCode::Backspace => self.model.search.backspace(),
            KeyCode::Delete => self.model.search.delete(),
            KeyCode::Left => self.model.search.left(),
            KeyCode::Right => self.model.search.right(),
            KeyCode::Home => self.model.search.home(),
            KeyCode::End => self.model.search.end(),
            _ => return Cmd::None,
        }

        if self.model.search.value() != before {
            self.search() // query changed → re-run search (or browse if now empty)
        } else {
            self.ensure_loaded() // nav / cursor move → maybe fetch a browse window
        }
    }

    /// NORMAL (command) mode: a small vim keymap. No key types text.
    fn on_key_normal(&mut self, key: KeyEvent) -> Cmd {
        match key.code {
            KeyCode::Enter => return Cmd::Quit, // accept
            KeyCode::Char('q') => return Cmd::Quit,
            KeyCode::Esc => return Cmd::Quit, // exit the UI
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Char('/') => {
                self.model.enter_search();
                return Cmd::None;
            }
            // Numeric count prefix: 1–9 always; 0 only extends an existing count.
            KeyCode::Char(d @ '1'..='9') => {
                self.model.push_count_digit(d as u8 - b'0');
                return Cmd::None;
            }
            KeyCode::Char('0') if self.model.count_pending() => {
                self.model.push_count_digit(0);
                return Cmd::None;
            }
            // --- motions: consume the count, then load the new window below ---
            KeyCode::Char('j') | KeyCode::Down => {
                let n = self.model.take_count();
                self.model.history.select_prev_by(n);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let n = self.model.take_count();
                self.model.history.select_next_by(n);
            }
            KeyCode::PageDown => {
                self.model.clear_count();
                self.model.history.page_up();
            }
            KeyCode::PageUp => {
                self.model.clear_count();
                self.model.history.page_down();
            }
            // h/l complete the hjkl set but have no vertical target yet (reserved
            // for future horizontal scroll): consume without moving or typing.
            KeyCode::Char('h') | KeyCode::Char('l') => {
                self.model.clear_count();
                return Cmd::None;
            }
            // Any other key just cancels a pending count. Doesn't type text.
            _ => {
                self.model.clear_count();
                return Cmd::None;
            }
        }
        self.ensure_loaded()
    }

    /// Load browse mode: the total history count and the first window.
    fn browse(&self) -> Cmd {
        let total_src = self.source.clone();
        let window_src = self.source.clone();
        Cmd::Batch(vec![
            Cmd::task(async move { Msg::HistoryTotal(total_src.total().await) }),
            Cmd::task(async move {
                Msg::HistoryLoaded {
                    start: INITIAL_WINDOW.start,
                    rows: window_src.load(INITIAL_WINDOW).await,
                }
            }),
        ])
    }

    /// Dispatch on the current query: browse when empty, otherwise search.
    fn search(&mut self) -> Cmd {
        let query = self.model.search.value().to_owned();
        if query.is_empty() {
            self.model.history.reset();
            return self.browse();
        }
        let source = self.source.clone();
        Cmd::task(async move {
            let rows = source.search(&query).await;
            Msg::SearchResults { query, rows }
        })
    }

    /// Load the window the model wants resident, if it isn't already.
    fn ensure_loaded(&mut self) -> Cmd {
        let range = self.model.history.desired_range();
        if range.is_empty() || self.model.history.has(range.clone()) {
            return Cmd::None;
        }
        let source = self.source.clone();
        let start = range.start;
        Cmd::task(async move {
            Msg::HistoryLoaded {
                start,
                rows: source.load(range).await,
            }
        })
    }
}

fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Prepare the turtle logo for rendering — but only when the terminal supports
/// a real image protocol (kitty, iTerm2, sixel). If it would fall back to block
/// characters (halfblocks), render nothing: better no logo than a blocky one.
/// Also `None` if the terminal can't be queried or the image won't decode.
///
/// **Call this before starting the runtime** (before it enters the alternate
/// screen / raw mode): `Picker::from_query_stdio` queries the terminal for its
/// graphics protocol and font size, which only works from a normal TTY.
pub fn build_turtle_logo() -> Option<StatefulProtocol> {
    let picker = Picker::from_query_stdio().ok()?;
    if picker.protocol_type() == ProtocolType::Halfblocks {
        return None;
    }
    let image = image::load_from_memory(TURTLE_PNG).ok()?;
    Some(picker.new_resize_protocol(image))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{HistoryList, HistoryRow, SearchInput};
    use crate::theme::Theme;
    use rstest::{fixture, rstest};

    #[derive(Clone)]
    struct TestSource;
    impl HistorySource for TestSource {
        async fn total(&self) -> usize {
            0
        }
        async fn load(&self, _range: Range<usize>) -> Vec<HistoryRow> {
            Vec::new()
        }
        async fn search(&self, _query: &str) -> Vec<HistoryRow> {
            Vec::new()
        }
    }

    // `App` (the runtime trait) is in scope via `use super::*`, so alias the
    // concrete test app under a different name.
    type TestApp = SearchInteractive<TestSource>;

    fn app_with_mode(mode: Option<Mode>) -> TestApp {
        let model = Model {
            theme: Theme::default(),
            enter_accept: true,
            history: HistoryList::new(),
            search: SearchInput::new(),
            mode,
        };
        SearchInteractive::new(model, None, TestSource)
    }

    /// The non-modal interface (today's plain search).
    #[fixture]
    fn plain() -> TestApp {
        app_with_mode(None)
    }

    /// The modal interface booted in NORMAL mode.
    #[fixture]
    fn normal() -> TestApp {
        app_with_mode(Some(Mode::Normal { count: None }))
    }

    /// The modal interface booted in SEARCH mode.
    #[fixture]
    fn search() -> TestApp {
        app_with_mode(Some(Mode::Search))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_chars(app: &mut TestApp, s: &str) {
        for c in s.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    /// Give the list `total` rows over a `height`-row viewport.
    fn seed(app: &mut TestApp, total: usize, height: u16) {
        app.model.history.set_viewport_height(height);
        app.model.history.set_total(total);
    }

    fn hrow(cmd: &str) -> HistoryRow {
        HistoryRow {
            id: cmd.into(),
            command: cmd.into(),
            timestamp: 0,
            duration: 0,
            exit: 0,
        }
    }

    // --- non-modal (plain) interface -----------------------------------------

    #[rstest]
    fn typing_edits_the_query(mut plain: TestApp) {
        type_chars(&mut plain, "ls");
        assert_eq!(plain.model.search.value(), "ls");
        plain.on_key(key(KeyCode::Backspace));
        assert_eq!(plain.model.search.value(), "l");
    }

    #[rstest]
    fn q_is_typed_not_quit(mut plain: TestApp) {
        plain.on_key(key(KeyCode::Char('q')));
        assert_eq!(plain.model.search.value(), "q");
    }

    #[rstest]
    fn esc_quits(mut plain: TestApp) {
        assert!(matches!(plain.on_key(key(KeyCode::Esc)), Cmd::Quit));
    }

    #[rstest]
    fn ctrl_modified_char_is_not_typed(mut plain: TestApp) {
        plain.on_key(ctrl('a'));
        assert_eq!(plain.model.search.value(), "", "Ctrl+A must not insert 'a'");
    }

    #[rstest]
    fn matching_results_replace_the_list(mut plain: TestApp) {
        type_chars(&mut plain, "git");
        plain.update(Msg::SearchResults {
            query: "git".into(),
            rows: vec![hrow("git status"), hrow("git log")],
        });
        assert_eq!(plain.model.history.total(), 2);
        assert_eq!(plain.model.history.selected(), 0);
    }

    #[rstest]
    fn stale_results_are_ignored(mut plain: TestApp) {
        type_chars(&mut plain, "new");
        // A result for an older query the user has since changed.
        plain.update(Msg::SearchResults {
            query: "old".into(),
            rows: vec![hrow("old thing")],
        });
        assert_eq!(plain.model.history.total(), 0);
    }

    #[rstest]
    fn browse_load_ignored_while_searching(mut plain: TestApp) {
        type_chars(&mut plain, "git");
        plain.update(Msg::SearchResults {
            query: "git".into(),
            rows: vec![hrow("git a"), hrow("git b")],
        });
        assert_eq!(plain.model.history.total(), 2);
        // A stale browse load must NOT clobber the active search results.
        plain.update(Msg::HistoryTotal(5_000_000));
        plain.update(Msg::HistoryLoaded {
            start: 0,
            rows: vec![hrow("browse x")],
        });
        assert_eq!(plain.model.history.total(), 2, "browse load clobbered search");
    }

    #[rstest]
    fn clearing_query_reverts_to_browse(mut plain: TestApp) {
        plain.on_key(key(KeyCode::Char('g')));
        let cmd = plain.on_key(key(KeyCode::Backspace));
        assert_eq!(plain.model.search.value(), "");
        assert!(matches!(cmd, Cmd::Batch(_)), "clearing query should browse");
        assert_eq!(plain.model.history.total(), 0, "browse reset clears the list");
    }

    // --- SEARCH mode ---------------------------------------------------------

    #[rstest]
    fn search_mode_types_query(mut search: TestApp) {
        type_chars(&mut search, "ls");
        assert_eq!(search.model.search.value(), "ls");
    }

    #[rstest]
    fn search_esc_switches_to_normal(mut search: TestApp) {
        let cmd = search.on_key(key(KeyCode::Esc));
        assert!(matches!(cmd, Cmd::None));
        assert_eq!(search.model.mode(), Some(Mode::Normal { count: None }));
    }

    #[rstest]
    fn search_enter_accepts(mut search: TestApp) {
        assert!(matches!(search.on_key(key(KeyCode::Enter)), Cmd::Quit));
    }

    // --- NORMAL mode ---------------------------------------------------------

    #[rstest]
    #[case::insert('i')]
    #[case::append('a')]
    #[case::search('/')]
    fn normal_i_a_and_slash_enter_search(mut normal: TestApp, #[case] enter: char) {
        normal.on_key(key(KeyCode::Char(enter)));
        assert_eq!(
            normal.model.mode(),
            Some(Mode::Search),
            "'{enter}' should enter SEARCH"
        );
    }

    #[rstest]
    fn normal_keys_do_not_type(mut normal: TestApp) {
        for c in ['j', 'k', 'x'] {
            normal.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(normal.model.search.value(), "");
    }

    #[rstest]
    fn normal_q_quits(mut normal: TestApp) {
        assert!(matches!(normal.on_key(key(KeyCode::Char('q'))), Cmd::Quit));
    }

    #[rstest]
    fn normal_enter_accepts(mut normal: TestApp) {
        assert!(matches!(normal.on_key(key(KeyCode::Enter)), Cmd::Quit));
    }

    #[rstest]
    fn normal_esc_exits(mut normal: TestApp) {
        assert!(matches!(normal.on_key(key(KeyCode::Esc)), Cmd::Quit));
    }

    #[rstest]
    // keys pressed after selecting the oldest row (99), then the expected selection
    #[case::bare_j(&['j'], 98)]
    #[case::ten_j(&['1', '0', 'j'], 89)]
    #[case::bare_zero_is_noop(&['0', 'j'], 98)]
    #[case::count_cancelled_by_unmapped_key(&['5', 'x', 'j'], 98)]
    fn normal_count_navigation(mut normal: TestApp, #[case] keys: &[char], #[case] expected: usize) {
        seed(&mut normal, 100, 20);
        normal.model.history.select_last(); // 99 (oldest / visual top)
        for &c in keys {
            normal.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(normal.model.history.selected(), expected);
    }

    #[rstest]
    #[case::h('h')]
    #[case::l('l')]
    fn normal_h_l_are_reserved_noops(mut normal: TestApp, #[case] c: char) {
        seed(&mut normal, 100, 20);
        normal.model.history.select_last(); // 99
        normal.on_key(key(KeyCode::Char(c)));
        assert_eq!(normal.model.history.selected(), 99, "{c} must not move");
        assert_eq!(normal.model.search.value(), "", "{c} must not type");
    }

    #[rstest]
    fn normal_pageupdown_page_the_list(mut normal: TestApp) {
        seed(&mut normal, 100, 20); // viewport height 20 => page = 20
        normal.model.history.select_last(); // 99 (oldest / visual top)
        normal.on_key(key(KeyCode::PageDown)); // pages toward newest by one viewport
        assert_eq!(normal.model.history.selected(), 79);
        normal.on_key(key(KeyCode::PageUp)); // pages back toward oldest
        assert_eq!(normal.model.history.selected(), 99);
    }

    #[rstest]
    fn normal_nav_during_active_search_keeps_results(mut search: TestApp) {
        // Type a query in SEARCH, get results, drop to NORMAL, navigate.
        type_chars(&mut search, "git");
        search.update(Msg::SearchResults {
            query: "git".into(),
            rows: vec![hrow("git status"), hrow("git log")],
        });
        assert_eq!(search.model.history.total(), 2);

        search.on_key(key(KeyCode::Esc)); // SEARCH -> NORMAL, query still "git"
        assert_eq!(search.model.mode(), Some(Mode::Normal { count: None }));

        // A NORMAL motion must not clobber the search results or fetch browse data:
        // the result set is fully resident, so no browse Cmd is emitted.
        let cmd = search.on_key(key(KeyCode::Char('j')));
        assert_eq!(search.model.history.total(), 2, "search results survived NORMAL nav");
        assert!(matches!(cmd, Cmd::None), "no browse load emitted during active search");
    }

    // --- mode-independent ----------------------------------------------------

    #[rstest]
    #[case::plain(None)]
    #[case::search(Some(Mode::Search))]
    #[case::normal(Some(Mode::Normal { count: None }))]
    fn ctrl_c_quits_from_any_mode(#[case] mode: Option<Mode>) {
        let mut app = app_with_mode(mode);
        assert!(matches!(app.update(Msg::Key(ctrl('c'))), Cmd::Quit));
    }
}
