use eyre::Result;
use itertools::Itertools;
use std::io::stdout;
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

use crate::command::event::{Event, Events};
use crate::local::database::Database;
use crate::local::history::History;

const VERSION: &str = env!("CARGO_PKG_VERSION");

struct State {
    input: String,

    results: Vec<History>,

    results_state: ListState,
}

fn query_results(app: &mut State, db: &mut impl Database) {
    let results = match app.input.as_str() {
        "" => db.list(),
        i => db.prefix_search(i),
    };

    if let Ok(results) = results {
        app.results = results.into_iter().rev().unique().collect();
    }

    if app.results.is_empty() {
        app.results_state.select(None);
    } else {
        app.results_state.select(Some(0));
    }
}

fn key_handler(input: Key, db: &mut impl Database, app: &mut State) -> Option<String> {
    match input {
        Key::Esc | Key::Char('\n') => {
            let i = app.results_state.selected().unwrap_or(0);

            return Some(app.results.get(i).unwrap().command.clone());
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
fn select_history(query: Vec<String>, db: &mut impl Database) -> Result<String> {
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

            let input = Paragraph::new(app.input.as_ref())
                .block(Block::default().borders(Borders::ALL).title("Search"));

            let results: Vec<ListItem> = app
                .results
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let mut content = Span::raw(m.command.to_string());

                    if let Some(selected) = app.results_state.selected() {
                        if selected == i {
                            content.style =
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
                        }
                    }

                    ListItem::new(content)
                })
                .collect();

            let results = List::new(results)
                .block(Block::default().borders(Borders::ALL).title("History"))
                .start_corner(Corner::BottomLeft)
                .highlight_symbol(">> ");

            let stats = Paragraph::new(Text::from(Span::raw(format!(
                "history count: {}",
                db.history_count().unwrap()
            ))))
            .alignment(Alignment::Right);

            f.render_widget(title, top_left_chunks[0]);
            f.render_widget(help, top_left_chunks[1]);

            f.render_widget(stats, top_right_chunks[0]);
            f.render_stateful_widget(results, chunks[1], &mut app.results_state);
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

pub fn run(query: Vec<String>, db: &mut impl Database) -> Result<()> {
    let item = select_history(query, db)?;
    eprintln!("{}", item);

    Ok(())
}
