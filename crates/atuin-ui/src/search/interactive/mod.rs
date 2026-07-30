use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Layout},
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};

use crate::models::{HistorySource, Model};
use crate::msg::Msg;
use crate::runtime::{App, Cmd};
use crate::search::views::history_list::HistoryListView;
use crate::search::views::titlebar::TitleBarView;

/// The atuin turtle, embedded at compile time.
const TURTLE_PNG: &[u8] = include_bytes!("../../../assets/atuin-turtle.png");

/// Rows to load at startup, before the first render establishes the real height.
const INITIAL_WINDOW: Range<usize> = 0..128;

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
        }
    }

    fn view(&mut self, frame: &mut Frame<'_>) {
        let [header, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(frame.area());

        // The list's viewport height is only known at render time (especially
        // inline); record it so `update`'s navigation and loading use the truth.
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
    }
}

impl<S: HistorySource> SearchInteractive<S> {
    fn on_key(&mut self, key: KeyEvent) -> Cmd {
        let handled = match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.model.history.select_next();
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.model.history.select_prev();
                true
            }
            KeyCode::PageDown => {
                self.model.history.page_down();
                true
            }
            KeyCode::PageUp => {
                self.model.history.page_up();
                true
            }
            KeyCode::Home => {
                self.model.history.select_first();
                true
            }
            KeyCode::End => {
                self.model.history.select_last();
                true
            }
            _ => false,
        };
        if handled {
            self.ensure_loaded()
        } else {
            Cmd::None
        }
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
    ctrl_c || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
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
