use std::{env, io::stdout, ops::Sub, time::Duration};

use chrono::Utc;
use clap::Parser;
use eyre::Result;
use termion::{
    event::Event as TermEvent, event::Key, event::MouseButton, event::MouseEvent,
    input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen,
};
use tui::{
    backend::{Backend, TermionBackend},
    layout::{Alignment, Constraint, Corner, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use unicode_width::UnicodeWidthStr;

use atuin_client::{
    database::current_context,
    database::Context,
    database::Database,
    history::History,
    settings::{FilterMode, SearchMode, Settings},
};

use super::{
    event::{Event, Events},
    history::ListMode,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
pub struct Cmd {
    /// Filter search result by directory
    #[clap(long, short)]
    cwd: Option<String>,

    /// Exclude directory from results
    #[clap(long = "exclude-cwd")]
    exclude_cwd: Option<String>,

    /// Filter search result by exit code
    #[clap(long, short)]
    exit: Option<i64>,

    /// Exclude results with this exit code
    #[clap(long = "exclude-exit")]
    exclude_exit: Option<i64>,

    /// Only include results added before this date
    #[clap(long, short)]
    before: Option<String>,

    /// Only include results after this date
    #[clap(long)]
    after: Option<String>,

    /// How many entries to return at most
    #[clap(long)]
    limit: Option<i64>,

    /// Open interactive search UI
    #[clap(long, short)]
    interactive: bool,

    /// Use human-readable formatting for time
    #[clap(long)]
    human: bool,

    query: Vec<String>,

    /// Show only the text of the command
    #[clap(long)]
    cmd_only: bool,
}

impl Cmd {
    pub async fn run(self, db: &mut impl Database, settings: &Settings) -> Result<()> {
        if self.interactive {
            let item = select_history(
                &self.query,
                settings.search_mode,
                settings.filter_mode,
                settings.style,
                db,
            )
            .await?;
            eprintln!("{}", item);
        } else {
            let list_mode = ListMode::from_flags(self.human, self.cmd_only);
            run_non_interactive(
                settings,
                list_mode,
                self.cwd,
                self.exit,
                self.exclude_exit,
                self.exclude_cwd,
                self.before,
                self.after,
                self.limit,
                &self.query,
                db,
            )
            .await?;
        };
        Ok(())
    }
}

struct State {
    input: Cursor,

    filter_mode: FilterMode,

    results: Vec<History>,

    results_state: ListState,

    context: Context,
}

impl State {
    #[allow(clippy::cast_sign_loss)]
    fn durations(&self) -> Vec<(String, String)> {
        self.results
            .iter()
            .map(|h| {
                let duration =
                    Duration::from_millis(std::cmp::max(h.duration, 0) as u64 / 1_000_000);
                let duration = humantime::format_duration(duration).to_string();
                let duration: Vec<&str> = duration.split(' ').collect();

                let ago = chrono::Utc::now().sub(h.timestamp);

                // Account for the chance that h.timestamp is "in the future"
                // This would mean that "ago" is negative, and the unwrap here
                // would fail.
                // If the timestamp would otherwise be in the future, display
                // the time ago as 0.
                let ago = humantime::format_duration(
                    ago.to_std().unwrap_or_else(|_| Duration::new(0, 0)),
                )
                .to_string();
                let ago: Vec<&str> = ago.split(' ').collect();

                (
                    duration[0]
                        .to_string()
                        .replace("days", "d")
                        .replace("day", "d")
                        .replace("weeks", "w")
                        .replace("week", "w")
                        .replace("months", "mo")
                        .replace("month", "mo")
                        .replace("years", "y")
                        .replace("year", "y"),
                    ago[0]
                        .to_string()
                        .replace("days", "d")
                        .replace("day", "d")
                        .replace("weeks", "w")
                        .replace("week", "w")
                        .replace("months", "mo")
                        .replace("month", "mo")
                        .replace("years", "y")
                        .replace("year", "y")
                        + " ago",
                )
            })
            .collect()
    }

    fn render_results<T: tui::backend::Backend>(
        &mut self,
        f: &mut tui::Frame<T>,
        r: tui::layout::Rect,
        b: tui::widgets::Block,
    ) {
        let durations = self.durations();
        let max_length = durations.iter().fold(0, |largest, i| {
            std::cmp::max(largest, i.0.len() + i.1.len())
        });

        let results: Vec<ListItem> = self
            .results
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let command = m.command.to_string().replace('\n', " ").replace('\t', " ");

                let mut command = Span::raw(command);

                let (duration, mut ago) = durations[i].clone();

                while (duration.len() + ago.len()) < max_length {
                    ago = format!(" {}", ago);
                }

                let selected_index = match self.results_state.selected() {
                    None => Span::raw("   "),
                    Some(selected) => match i.checked_sub(selected) {
                        None => Span::raw("   "),
                        Some(diff) => {
                            if 0 < diff && diff < 10 {
                                Span::raw(format!(" {} ", diff))
                            } else {
                                Span::raw("   ")
                            }
                        }
                    },
                };

                let duration = Span::styled(
                    duration,
                    Style::default().fg(if m.success() {
                        Color::Green
                    } else {
                        Color::Red
                    }),
                );

                let ago = Span::styled(ago, Style::default().fg(Color::Blue));

                if let Some(selected) = self.results_state.selected() {
                    if selected == i {
                        command.style =
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
                    }
                }

                let spans = Spans::from(vec![
                    selected_index,
                    duration,
                    Span::raw(" "),
                    ago,
                    Span::raw(" "),
                    command,
                ]);

                ListItem::new(spans)
            })
            .collect();

        let results = List::new(results)
            .block(b)
            .start_corner(Corner::BottomLeft)
            .highlight_symbol(">> ");

        f.render_stateful_widget(results, r, &mut self.results_state);
    }
}

async fn query_results(
    app: &mut State,
    search_mode: SearchMode,
    db: &mut impl Database,
) -> Result<()> {
    let results = match app.input.as_str() {
        "" => {
            db.list(app.filter_mode, &app.context, Some(200), true)
                .await?
        }
        i => {
            db.search(Some(200), search_mode, app.filter_mode, &app.context, i)
                .await?
        }
    };

    app.results = results;

    if app.results.is_empty() {
        app.results_state.select(None);
    } else {
        app.results_state.select(Some(0));
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn key_handler<'app>(input: &TermEvent, app: &'app mut State) -> Option<&'app str> {
    match input {
        TermEvent::Key(Key::Esc | Key::Ctrl('c' | 'd' | 'g')) => return Some(""),
        TermEvent::Key(Key::Char('\n')) => {
            let i = app.results_state.selected().unwrap_or(0);

            return Some(
                app.results
                    .get(i)
                    .map_or(app.input.as_str(), |h| h.command.as_str()),
            );
        }
        TermEvent::Key(Key::Alt(c @ '1'..='9')) => {
            let c = c.to_digit(10)? as usize;
            let i = app.results_state.selected()? + c;

            return Some(
                app.results
                    .get(i)
                    .map_or(app.input.as_str(), |h| h.command.as_str()),
            );
        }
        TermEvent::Key(Key::Left | Key::Ctrl('h')) => {
            app.input.left();
        }
        TermEvent::Key(Key::Right | Key::Ctrl('l')) => app.input.right(),
        TermEvent::Key(Key::Ctrl('a')) => app.input.start(),
        TermEvent::Key(Key::Ctrl('e')) => app.input.end(),
        TermEvent::Key(Key::Char(c)) => app.input.insert(*c),
        TermEvent::Key(Key::Backspace) => {
            app.input.back();
        }
        TermEvent::Key(Key::Ctrl('w')) => {
            // remove the first batch of whitespace
            while matches!(app.input.back(), Some(c) if c.is_whitespace()) {}
            while app.input.left() {
                if app.input.char().unwrap().is_whitespace() {
                    app.input.right(); // found whitespace, go back right
                    break;
                }
                app.input.remove();
            }
        }
        TermEvent::Key(Key::Ctrl('u')) => app.input.clear(),
        TermEvent::Key(Key::Ctrl('r')) => {
            app.filter_mode = match app.filter_mode {
                FilterMode::Global => FilterMode::Host,
                FilterMode::Host => FilterMode::Session,
                FilterMode::Session => FilterMode::Directory,
                FilterMode::Directory => FilterMode::Global,
            };
        }
        TermEvent::Key(Key::Down | Key::Ctrl('n' | 'j'))
        | TermEvent::Mouse(MouseEvent::Press(MouseButton::WheelDown, _, _)) => {
            let i = match app.results_state.selected() {
                Some(i) => {
                    if i == 0 {
                        0
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };
            app.results_state.select(Some(i));
        }
        TermEvent::Key(Key::Up | Key::Ctrl('p' | 'k'))
        | TermEvent::Mouse(MouseEvent::Press(MouseButton::WheelUp, _, _)) => {
            let i = match app.results_state.selected() {
                Some(i) => {
                    if i >= app.results.len() - 1 {
                        app.results.len() - 1
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };
            app.results_state.select(Some(i));
        }
        _ => {}
    };

    None
}

#[allow(clippy::cast_possible_truncation)]
fn draw<T: Backend>(f: &mut Frame<'_, T>, history_count: i64, app: &mut State) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(3),
            ]
            .as_ref(),
        )
        .split(f.size());

    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(chunks[0]);

    let top_left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)].as_ref())
        .split(top_chunks[0]);

    let top_right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)].as_ref())
        .split(top_chunks[1]);

    let title = Paragraph::new(Text::from(Span::styled(
        format!("Atuin v{}", VERSION),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    let help = vec![
        Span::raw("Press "),
        Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" to exit."),
    ];

    let help = Text::from(Spans::from(help));
    let help = Paragraph::new(help);

    let filter_mode = match app.filter_mode {
        FilterMode::Global => "GLOBAL",
        FilterMode::Host => "HOST",
        FilterMode::Session => "SESSION",
        FilterMode::Directory => "DIRECTORY",
    };

    let input = Paragraph::new(app.input.as_str().to_owned())
        .block(Block::default().borders(Borders::ALL).title(filter_mode));

    let stats = Paragraph::new(Text::from(Span::raw(format!(
        "history count: {}",
        history_count,
    ))))
    .alignment(Alignment::Right);

    f.render_widget(title, top_left_chunks[0]);
    f.render_widget(help, top_left_chunks[1]);
    f.render_widget(stats, top_right_chunks[0]);

    app.render_results(
        f,
        chunks[1],
        Block::default().borders(Borders::ALL).title("History"),
    );
    f.render_widget(input, chunks[2]);

    let width = UnicodeWidthStr::width(app.input.substring());
    f.set_cursor(
        // Put cursor past the end of the input text
        chunks[2].x + width as u16 + 1,
        // Move one line down, from the border to the input line
        chunks[2].y + 1,
    );
}

#[allow(clippy::cast_possible_truncation)]
fn draw_compact<T: Backend>(f: &mut Frame<'_, T>, history_count: i64, app: &mut State) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(0)
        .horizontal_margin(1)
        .constraints(
            [
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ]
            .as_ref(),
        )
        .split(f.size());

    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ]
            .as_ref(),
        )
        .split(chunks[0]);

    let title = Paragraph::new(Text::from(Span::styled(
        format!("Atuin v{}", VERSION),
        Style::default().fg(Color::DarkGray),
    )));

    let help = Paragraph::new(Text::from(Spans::from(vec![
        Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" to exit"),
    ])))
    .style(Style::default().fg(Color::DarkGray))
    .alignment(Alignment::Center);

    let stats = Paragraph::new(Text::from(Span::raw(format!(
        "history count: {}",
        history_count,
    ))))
    .style(Style::default().fg(Color::DarkGray))
    .alignment(Alignment::Right);

    let filter_mode = match app.filter_mode {
        FilterMode::Global => "GLOBAL",
        FilterMode::Host => "HOST",
        FilterMode::Session => "SESSION",
        FilterMode::Directory => "DIRECTORY",
    };

    let input =
        Paragraph::new(format!("{}] {}", filter_mode, app.input.as_str())).block(Block::default());

    f.render_widget(title, header_chunks[0]);
    f.render_widget(help, header_chunks[1]);
    f.render_widget(stats, header_chunks[2]);

    app.render_results(f, chunks[1], Block::default());
    f.render_widget(input, chunks[2]);

    let extra_width = UnicodeWidthStr::width(app.input.substring()) + filter_mode.len();

    f.set_cursor(
        // Put cursor past the end of the input text
        chunks[2].x + extra_width as u16 + 2,
        // Move one line down, from the border to the input line
        chunks[2].y + 1,
    );
}

// this is a big blob of horrible! clean it up!
// for now, it works. But it'd be great if it were more easily readable, and
// modular. I'd like to add some more stats and stuff at some point
#[allow(clippy::cast_possible_truncation)]
async fn select_history(
    query: &[String],
    search_mode: SearchMode,
    filter_mode: FilterMode,
    style: atuin_client::settings::Style,
    db: &mut impl Database,
) -> Result<String> {
    let stdout = stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Setup event handlers
    let events = Events::new();

    let mut input = Cursor::from(query.join(" "));
    // Put the cursor at the end of the query by default
    input.end();
    let mut app = State {
        input,
        results: Vec::new(),
        results_state: ListState::default(),
        context: current_context(),
        filter_mode,
    };

    query_results(&mut app, search_mode, db).await?;

    loop {
        let history_count = db.history_count().await?;
        let initial_input = app.input.as_str().to_owned();
        let initial_filter_mode = app.filter_mode;

        // Handle input
        if let Event::Input(input) = events.next()? {
            if let Some(output) = key_handler(&input, &mut app) {
                return Ok(output.to_owned());
            }
        }

        // After we receive input process the whole event channel before query/render.
        while let Ok(Event::Input(input)) = events.try_next() {
            if let Some(output) = key_handler(&input, &mut app) {
                return Ok(output.to_owned());
            }
        }

        if initial_input != app.input.as_str() || initial_filter_mode != app.filter_mode {
            query_results(&mut app, search_mode, db).await?;
        }

        let compact = match style {
            atuin_client::settings::Style::Auto => {
                terminal.size().map(|size| size.height < 14).unwrap_or(true)
            }
            atuin_client::settings::Style::Compact => true,
            atuin_client::settings::Style::Full => false,
        };
        if compact {
            terminal.draw(|f| draw_compact(f, history_count, &mut app))?;
        } else {
            terminal.draw(|f| draw(f, history_count, &mut app))?;
        }
    }
}

// This is supposed to more-or-less mirror the command line version, so ofc
// it is going to have a lot of args
#[allow(clippy::too_many_arguments)]
async fn run_non_interactive(
    settings: &Settings,
    list_mode: ListMode,
    cwd: Option<String>,
    exit: Option<i64>,
    exclude_exit: Option<i64>,
    exclude_cwd: Option<String>,
    before: Option<String>,
    after: Option<String>,
    limit: Option<i64>,
    query: &[String],
    db: &mut impl Database,
) -> Result<()> {
    let dir = if cwd.as_deref() == Some(".") {
        let current = std::env::current_dir()?;
        let current = current.as_os_str();
        let current = current.to_str().unwrap();

        Some(current.to_owned())
    } else {
        cwd
    };

    let context = current_context();

    let results = db
        .search(
            limit,
            settings.search_mode,
            settings.filter_mode,
            &context,
            query.join(" ").as_str(),
        )
        .await?;

    // TODO: This filtering would be better done in the SQL query, I just
    // need a nice way of building queries.
    let results: Vec<History> = results
        .iter()
        .filter(|h| {
            if let Some(exit) = exit {
                if h.exit != exit {
                    return false;
                }
            }

            if let Some(exit) = exclude_exit {
                if h.exit == exit {
                    return false;
                }
            }

            if let Some(cwd) = &exclude_cwd {
                if h.cwd.as_str() == cwd.as_str() {
                    return false;
                }
            }

            if let Some(cwd) = &dir {
                if h.cwd.as_str() != cwd.as_str() {
                    return false;
                }
            }

            if let Some(before) = &before {
                let before = chrono_english::parse_date_string(
                    before.as_str(),
                    Utc::now(),
                    chrono_english::Dialect::Uk,
                );

                if before.is_err() || h.timestamp.gt(&before.unwrap()) {
                    return false;
                }
            }

            if let Some(after) = &after {
                let after = chrono_english::parse_date_string(
                    after.as_str(),
                    Utc::now(),
                    chrono_english::Dialect::Uk,
                );

                if after.is_err() || h.timestamp.lt(&after.unwrap()) {
                    return false;
                }
            }

            true
        })
        .map(std::borrow::ToOwned::to_owned)
        .collect();

    super::history::print_list(&results, list_mode);
    Ok(())
}

struct Cursor {
    source: String,
    index: usize,
}

impl From<String> for Cursor {
    fn from(source: String) -> Self {
        Self { source, index: 0 }
    }
}

impl Cursor {
    pub fn as_str(&self) -> &str {
        self.source.as_str()
    }

    /// Returns the string before the cursor
    pub fn substring(&self) -> &str {
        &self.source[..self.index]
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns the currently selected [`char`]
    pub fn char(&self) -> Option<char> {
        self.source[self.index..].chars().next()
    }

    pub fn right(&mut self) {
        if self.index < self.source.len() {
            loop {
                self.index += 1;
                if self.source.is_char_boundary(self.index) {
                    break;
                }
            }
        }
    }

    pub fn left(&mut self) -> bool {
        if self.index > 0 {
            loop {
                self.index -= 1;
                if self.source.is_char_boundary(self.index) {
                    break true;
                }
            }
        } else {
            false
        }
    }

    pub fn insert(&mut self, c: char) {
        self.source.insert(self.index, c);
        self.index += c.len_utf8();
    }

    pub fn remove(&mut self) -> char {
        self.source.remove(self.index)
    }

    pub fn back(&mut self) -> Option<char> {
        if self.left() {
            Some(self.remove())
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.source.clear();
        self.index = 0;
    }

    pub fn end(&mut self) {
        self.index = self.source.len();
    }

    pub fn start(&mut self) {
        self.index = 0;
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::Cursor;

    #[test]
    fn right() {
        // ö is 2 bytes
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        let indices = [0, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 20, 20, 20];
        for i in indices {
            assert_eq!(c.index(), i);
            c.right();
        }
    }

    #[test]
    fn left() {
        // ö is 2 bytes
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        c.end();
        let indices = [20, 18, 17, 15, 14, 12, 11, 9, 8, 6, 5, 3, 2, 0, 0, 0, 0];
        for i in indices {
            assert_eq!(c.index(), i);
            c.left();
        }
    }

    #[test]
    fn pop() {
        let mut s = String::from("öaöböcödöeöfö");
        let mut c = Cursor::from(s.clone());
        c.end();
        while !s.is_empty() {
            let c1 = s.pop();
            let c2 = c.back();
            assert_eq!(c1, c2);
            assert_eq!(s.as_str(), c.substring());
        }
        let c1 = s.pop();
        let c2 = c.back();
        assert_eq!(c1, c2);
    }

    #[test]
    fn back() {
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        // move to                                 ^
        for _ in 0..4 { c.right() }
        assert_eq!(c.substring(), "öaöb");
        assert_eq!(c.back(), Some('b'));
        assert_eq!(c.back(), Some('ö'));
        assert_eq!(c.back(), Some('a'));
        assert_eq!(c.back(), Some('ö'));
        assert_eq!(c.back(), None);
        assert_eq!(c.as_str(), "öcödöeöfö");
    }

    #[test]
    fn insert() {
        let mut c = Cursor::from(String::from("öaöböcödöeöfö"));
        // move to                                 ^
        for _ in 0..4 { c.right() }
        assert_eq!(c.substring(), "öaöb");
        c.insert('ö');
        c.insert('g');
        c.insert('ö');
        c.insert('h');
        assert_eq!(c.substring(), "öaöbögöh");
        assert_eq!(c.as_str(), "öaöbögöhöcödöeöfö");
    }
}
