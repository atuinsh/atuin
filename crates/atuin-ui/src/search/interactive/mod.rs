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
            KeyCode::Esc => {
                self.model.clear_count();
                return Cmd::None;
            }
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

    fn app_with_mode(mode: Option<Mode>) -> SearchInteractive<TestSource> {
        let model = Model {
            theme: Theme::default(),
            enter_accept: true,
            history: HistoryList::new(),
            search: SearchInput::new(),
            mode,
        };
        SearchInteractive::new(model, None, TestSource)
    }

    fn app() -> SearchInteractive<TestSource> {
        app_with_mode(None)
    }

    fn modal_app_normal() -> SearchInteractive<TestSource> {
        app_with_mode(Some(Mode::Normal { count: None }))
    }

    fn modal_app_search() -> SearchInteractive<TestSource> {
        app_with_mode(Some(Mode::Search))
    }

    fn seed(app: &mut SearchInteractive<TestSource>, total: usize, height: u16) {
        app.model.history.set_viewport_height(height);
        app.model.history.set_total(total);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    #[test]
    fn typing_edits_the_query() {
        let mut app = app();
        app.on_key(key(KeyCode::Char('l')));
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(app.model.search.value(), "ls");
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.model.search.value(), "l");
    }

    #[test]
    fn q_is_typed_not_quit() {
        let mut app = app();
        app.on_key(key(KeyCode::Char('q')));
        assert_eq!(app.model.search.value(), "q");
    }

    #[test]
    fn esc_quits() {
        let mut app = app();
        assert!(matches!(app.on_key(key(KeyCode::Esc)), Cmd::Quit));
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

    #[test]
    fn matching_results_replace_the_list() {
        let mut app = app();
        for c in "git".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        let rows = vec![hrow("git status"), hrow("git log")];
        app.update(Msg::SearchResults {
            query: "git".into(),
            rows,
        });
        assert_eq!(app.model.history.total(), 2);
        assert_eq!(app.model.history.selected(), 0);
    }

    #[test]
    fn stale_results_are_ignored() {
        let mut app = app();
        for c in "new".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        // A result for an older query the user has since changed.
        app.update(Msg::SearchResults {
            query: "old".into(),
            rows: vec![hrow("old thing")],
        });
        assert_eq!(app.model.history.total(), 0);
    }

    #[test]
    fn browse_load_ignored_while_searching() {
        let mut app = app();
        for c in "git".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.update(Msg::SearchResults {
            query: "git".into(),
            rows: vec![hrow("git a"), hrow("git b")],
        });
        assert_eq!(app.model.history.total(), 2);
        // A stale browse load must NOT clobber the active search results.
        app.update(Msg::HistoryTotal(5_000_000));
        app.update(Msg::HistoryLoaded {
            start: 0,
            rows: vec![hrow("browse x")],
        });
        assert_eq!(app.model.history.total(), 2, "browse load clobbered search");
    }

    #[test]
    fn ctrl_modified_char_is_not_typed() {
        let mut app = app();
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(app.model.search.value(), "", "Ctrl+A must not insert 'a'");
    }

    #[test]
    fn clearing_query_reverts_to_browse() {
        let mut app = app();
        app.on_key(key(KeyCode::Char('g')));
        let cmd = app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.model.search.value(), "");
        assert!(matches!(cmd, Cmd::Batch(_)), "clearing query should browse");
        assert_eq!(app.model.history.total(), 0, "browse reset clears the list");
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = app();
        let cmd = app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(matches!(cmd, Cmd::Quit));
    }

    #[test]
    fn search_mode_types_query() {
        let mut app = modal_app_search();
        app.on_key(key(KeyCode::Char('l')));
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(app.model.search.value(), "ls");
    }

    #[test]
    fn search_esc_switches_to_normal_not_quit() {
        let mut app = modal_app_search();
        let cmd = app.on_key(key(KeyCode::Esc));
        assert!(matches!(cmd, Cmd::None));
        assert_eq!(app.model.mode(), Some(Mode::Normal { count: None }));
    }

    #[test]
    fn search_enter_accepts() {
        let mut app = modal_app_search();
        assert!(matches!(app.on_key(key(KeyCode::Enter)), Cmd::Quit));
    }

    #[test]
    fn normal_i_a_and_slash_enter_search() {
        for enter in ['i', 'a', '/'] {
            let mut app = modal_app_normal();
            app.on_key(key(KeyCode::Char(enter)));
            assert_eq!(
                app.model.mode(),
                Some(Mode::Search),
                "'{enter}' should enter SEARCH"
            );
        }
    }

    #[test]
    fn normal_keys_do_not_type() {
        let mut app = modal_app_normal();
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Char('k')));
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(app.model.search.value(), "");
    }

    #[test]
    fn normal_q_quits_and_enter_accepts() {
        let mut app = modal_app_normal();
        assert!(matches!(app.on_key(key(KeyCode::Char('q'))), Cmd::Quit));
        assert!(matches!(app.on_key(key(KeyCode::Enter)), Cmd::Quit));
    }

    #[test]
    fn normal_bare_j_moves_one() {
        let mut app = modal_app_normal();
        seed(&mut app, 100, 20);
        app.model.history.select_last(); // 99
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.model.history.selected(), 98);
    }

    #[test]
    fn normal_count_jumps_multiple_rows() {
        let mut app = modal_app_normal();
        seed(&mut app, 100, 20);
        app.model.history.select_last(); // 99
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('0')));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.model.history.selected(), 89);
    }

    #[test]
    fn normal_zero_without_count_is_noop() {
        let mut app = modal_app_normal();
        seed(&mut app, 100, 20);
        app.model.history.select_last(); // 99
        app.on_key(key(KeyCode::Char('0'))); // bare 0 → no-op
        app.on_key(key(KeyCode::Char('j'))); // moves 1
        assert_eq!(app.model.history.selected(), 98);
    }

    #[test]
    fn normal_esc_clears_pending_count() {
        let mut app = modal_app_normal();
        seed(&mut app, 100, 20);
        app.model.history.select_last(); // 99
        app.on_key(key(KeyCode::Char('5')));
        app.on_key(key(KeyCode::Esc)); // cancels the 5
        app.on_key(key(KeyCode::Char('j'))); // moves 1, not 5
        assert_eq!(app.model.history.selected(), 98);
        assert_eq!(app.model.mode(), Some(Mode::Normal { count: None }));
    }

    #[test]
    fn normal_h_l_are_reserved_noops() {
        let mut app = modal_app_normal();
        seed(&mut app, 100, 20);
        app.model.history.select_last(); // 99
        app.on_key(key(KeyCode::Char('h')));
        app.on_key(key(KeyCode::Char('l')));
        assert_eq!(app.model.history.selected(), 99, "h/l must not move");
        assert_eq!(app.model.search.value(), "", "h/l must not type");
    }

    #[test]
    fn normal_nav_during_active_search_keeps_results() {
        // Type a query in SEARCH, get results, drop to NORMAL, navigate.
        let mut app = modal_app_search();
        for c in "git".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.update(Msg::SearchResults {
            query: "git".into(),
            rows: vec![hrow("git status"), hrow("git log")],
        });
        assert_eq!(app.model.history.total(), 2);

        app.on_key(key(KeyCode::Esc)); // SEARCH -> NORMAL, query still "git"
        assert_eq!(app.model.mode(), Some(Mode::Normal { count: None }));

        // A NORMAL motion must not clobber the search results or fetch browse data:
        // the result set is fully resident, so no browse Cmd is emitted.
        let cmd = app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.model.history.total(), 2, "search results survived NORMAL nav");
        assert!(matches!(cmd, Cmd::None), "no browse load emitted during active search");
    }

    #[test]
    fn normal_pageupdown_page_the_list() {
        let mut app = modal_app_normal();
        seed(&mut app, 100, 20); // viewport height 20 => page = 20
        app.model.history.select_last(); // 99 (oldest / visual top)
        app.on_key(key(KeyCode::PageDown)); // pages toward newest by one viewport
        assert_eq!(app.model.history.selected(), 79);
        app.on_key(key(KeyCode::PageUp)); // pages back toward oldest
        assert_eq!(app.model.history.selected(), 99);
    }

    #[test]
    fn ctrl_c_quits_from_any_mode() {
        for mut app in [modal_app_search(), modal_app_normal()] {
            let cmd = app.update(Msg::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            )));
            assert!(matches!(cmd, Cmd::Quit));
        }
    }
}
