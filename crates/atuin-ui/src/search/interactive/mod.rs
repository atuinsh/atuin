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

use crate::models::{HistorySource, Model};
use crate::msg::Msg;
use crate::runtime::{App, Cmd};
use crate::search::views::history_list::HistoryListView;
use crate::search::views::search_input::SearchInputView;
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

    fn update(&mut self, msg: Msg) -> Cmd {
        match msg {
            Msg::Key(key) if is_quit(&key) => Cmd::Quit,
            Msg::Key(key) => self.on_key(key),
            Msg::Resize(..) => self.ensure_loaded(),
            Msg::HistoryTotal(total) => {
                self.model.history.set_total(total);
                self.ensure_loaded()
            }
            Msg::HistoryLoaded { start, rows } => {
                self.model.history.apply(start, rows);
                Cmd::None
            }
            // Wired up in a later task: applying results and the stale-query guard.
            Msg::SearchResults { .. } => Cmd::None,
        }
    }

    fn view(&mut self, frame: &mut Frame<'_>) {
        let [header, body, input] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());

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
            },
            input,
        );

        // Show the terminal cursor in the input, after the prompt + query cursor.
        let cursor_x =
            input.x + PROMPT.chars().count() as u16 + self.model.search.cursor_char() as u16;
        frame.set_cursor_position(Position::new(
            cursor_x.min(input.right().saturating_sub(1)),
            input.y,
        ));
    }
}

impl<S: HistorySource> SearchInteractive<S> {
    fn on_key(&mut self, key: KeyEvent) -> Cmd {
        if is_quit(&key) {
            return Cmd::Quit;
        }
        if key.code == KeyCode::Enter {
            return Cmd::Quit; // accept (returning the command is a follow-up)
        }

        // The list is inverted (newest at the bottom): up = older, down = newer.
        match key.code {
            KeyCode::Up => self.model.history.select_next(),
            KeyCode::Down => self.model.history.select_prev(),
            KeyCode::PageUp => self.model.history.page_down(),
            KeyCode::PageDown => self.model.history.page_up(),
            // Editing keys act on the query.
            KeyCode::Char(c) => self.model.search.insert(c),
            KeyCode::Backspace => self.model.search.backspace(),
            KeyCode::Delete => self.model.search.delete(),
            KeyCode::Left => self.model.search.left(),
            KeyCode::Right => self.model.search.right(),
            KeyCode::Home => self.model.search.home(),
            KeyCode::End => self.model.search.end(),
            _ => return Cmd::None,
        }
        // Task 7 replaces this line with query-aware search/browse dispatch.
        self.ensure_loaded()
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

fn is_quit(key: &KeyEvent) -> bool {
    let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
    ctrl_c || key.code == KeyCode::Esc
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

    fn app() -> SearchInteractive<TestSource> {
        let model = Model {
            theme: Theme::default(),
            enter_accept: true,
            history: HistoryList::new(),
            search: SearchInput::new(),
        };
        SearchInteractive::new(model, None, TestSource)
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
}
