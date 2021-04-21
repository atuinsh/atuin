use eyre::Result;
use std::time::Duration;
use std::{io::stdout, ops::Sub};

use termion::{event::Key, input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Alignment, Constraint, Corner, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};
use unicode_width::UnicodeWidthStr;

use atuin_client::database::Database;
use atuin_client::history::History;

use crate::command::event::{Event, Events};

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct State {
    input: String,

    results: Vec<History>,

    results_state: ListState,
}

#[allow(clippy::clippy::cast_sign_loss)]
impl State {
    fn durations(&self) -> Vec<(String, String)> {
        self.results
            .iter()
            .map(|h| {
                let duration =
                    Duration::from_millis(std::cmp::max(h.duration, 0) as u64 / 1_000_000);
                let duration = humantime::format_duration(duration).to_string();
                let duration: Vec<&str> = duration.split(' ').collect();

                let ago = chrono::Utc::now().sub(h.timestamp);
                let ago = humantime::format_duration(ago.to_std().unwrap()).to_string();
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
                let command = m.command.to_string().replace("\n", " ").replace("\t", " ");

                let mut command = Span::raw(command);

                let (duration, mut ago) = durations[i].clone();

                while (duration.len() + ago.len()) < max_length {
                    ago = " ".to_owned() + ago.as_str();
                }

                let duration = Span::styled(
                    duration,
                    Style::default().fg(if m.exit == 0 || m.duration == -1 {
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

                let spans =
                    Spans::from(vec![duration, Span::raw(" "), ago, Span::raw(" "), command]);

                ListItem::new(spans)
            })
            .collect();

        let results = List::new(results)
            .block(Block::default().borders(Borders::ALL).title("History"))
            .start_corner(Corner::BottomLeft)
            .highlight_symbol(">> ");

        f.render_stateful_widget(results, r, &mut self.results_state);
    }
}

fn query_results(app: &mut State, db: &mut impl Database) {
    let results = match app.input.as_str() {
        "" => db.list(Some(200), true),
        i => db.prefix_search(i),
    };

    if let Ok(results) = results {
        app.results = results;
    }

    if app.results.is_empty() {
        app.results_state.select(None);
    } else {
        app.results_state.select(Some(0));
    }
}

fn key_handler(input: Key, db: &mut impl Database, app: &mut State) -> Option<String> {
    match input {
        Key::Esc => return Some(String::from("")),
        Key::Char('\n') => {
            let i = app.results_state.selected().unwrap_or(0);

            return Some(
                app.results
                    .get(i)
                    .map_or("".to_string(), |h| h.command.clone()),
            );
        }
        Key::Char(c) => {
            app.input.push(c);
            query_results(app, db);
        }
        Key::Backspace => {
            app.input.pop();
            query_results(app, db);
        }
        Key::Down => {
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
        Key::Up => {
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

// this is a big blob of horrible! clean it up!
// for now, it works. But it'd be great if it were more easily readable, and
// modular. I'd like to add some more stats and stuff at some point
#[allow(clippy::clippy::cast_possible_truncation)]
fn select_history(query: &[String], db: &mut impl Database) -> Result<String> {
    let stdout = stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Setup event handlers
    let events = Events::new();

    let mut app = State {
        input: query.join(" "),
        results: Vec::new(),
        results_state: ListState::default(),
    };

    query_results(&mut app, db);

    loop {
        // Handle input
        if let Event::Input(input) = events.next()? {
            if let Some(output) = key_handler(input, db, &mut app) {
                return Ok(output);
            }
        }

        terminal.draw(|f| {
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
                format!("A'tuin v{}", VERSION),
                Style::default().add_modifier(Modifier::BOLD),
            )));

            let help = vec![
                Span::raw("Press "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" to exit."),
            ];

            let help = Text::from(Spans::from(help));
            let help = Paragraph::new(help);

            let input = Paragraph::new(app.input.clone())
                .block(Block::default().borders(Borders::ALL).title("Query"));

            let stats = Paragraph::new(Text::from(Span::raw(format!(
                "history count: {}",
                db.history_count().unwrap()
            ))))
            .alignment(Alignment::Right);

            f.render_widget(title, top_left_chunks[0]);
            f.render_widget(help, top_left_chunks[1]);

            app.render_results(f, chunks[1]);
            f.render_widget(stats, top_right_chunks[0]);
            f.render_widget(input, chunks[2]);

            f.set_cursor(
                // Put cursor past the end of the input text
                chunks[2].x + app.input.width() as u16 + 1,
                // Move one line down, from the border to the input line
                chunks[2].y + 1,
            );
        })?;
    }
}

pub fn run(
    cwd: Option<String>,
    exit: Option<i64>,
    interactive: bool,
    query: &[String],
    db: &mut impl Database,
) -> Result<()> {
    let dir = if let Some(cwd) = cwd {
        if cwd == "." {
            let current = std::env::current_dir()?;
            let current = current.as_os_str();
            let current = current.to_str().unwrap();

            Some(current.to_owned())
        } else {
            Some(cwd)
        }
    } else {
        None
    };

    if interactive {
        let item = select_history(query, db)?;
        eprintln!("{}", item);
    } else {
        let results = db.search(dir, exit, query.join(" ").as_str())?;

        for i in &results {
            println!("{}", i.command);
        }
    }

    Ok(())
}
