#[allow(dead_code)]
mod util;

use crate::util::event::{Event, Events};
use std::{error::Error, io};
use termion::{event::Key, input::MouseTerminal, raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};

fn main() -> Result<(), Box<dyn Error>> {
    // Terminal initialization
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = MouseTerminal::from(stdout);
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let events = Events::new();

    let mut scroll: u16 = 0;
    loop {
        terminal.draw(|f| {
            let size = f.size();

            // Words made "loooong" to demonstrate line breaking.
            let s = "Veeeeeeeeeeeeeeeery    loooooooooooooooooong   striiiiiiiiiiiiiiiiiiiiiiiiiing.   ";
            let mut long_line = s.repeat(usize::from(size.width) / s.len() + 4);
            long_line.push('\n');

            let block = Block::default()
                .style(Style::default().bg(Color::White).fg(Color::Black));
            f.render_widget(block, size);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(5)
                .constraints(
                    [
                        Constraint::Percentage(25),
                        Constraint::Percentage(25),
                        Constraint::Percentage(25),
                        Constraint::Percentage(25),
                    ]
                    .as_ref(),
                )
                .split(size);

            let text = vec![
                Spans::from("This is a line "),
                Spans::from(Span::styled("This is a line   ", Style::default().fg(Color::Red))),
                Spans::from(Span::styled("This is a line", Style::default().bg(Color::Blue))),
                Spans::from(Span::styled(
                    "This is a longer line",
                    Style::default().add_modifier(Modifier::CROSSED_OUT),
                )),
                Spans::from(Span::styled(&long_line, Style::default().bg(Color::Green))),
                Spans::from(Span::styled(
                    "This is a line",
                    Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC),
                )),
            ];

            let create_block = |title| {
                Block::default()
                    .borders(Borders::ALL)
                    .style(Style::default().bg(Color::White).fg(Color::Black))
                    .title(Span::styled(title, Style::default().add_modifier(Modifier::BOLD)))
            };
            let paragraph = Paragraph::new(text.clone())
                .style(Style::default().bg(Color::White).fg(Color::Black))
                .block(create_block("Left, no wrap"))
                .alignment(Alignment::Left);
            f.render_widget(paragraph, chunks[0]);
            let paragraph = Paragraph::new(text.clone())
                .style(Style::default().bg(Color::White).fg(Color::Black))
                .block(create_block("Left, wrap"))
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true });
            f.render_widget(paragraph, chunks[1]);
            let paragraph = Paragraph::new(text.clone())
                .style(Style::default().bg(Color::White).fg(Color::Black))
                .block(create_block("Center, wrap"))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .scroll((scroll, 0));
            f.render_widget(paragraph, chunks[2]);
            let paragraph = Paragraph::new(text)
                .style(Style::default().bg(Color::White).fg(Color::Black))
                .block(create_block("Right, wrap"))
                .alignment(Alignment::Right)
                .wrap(Wrap { trim: true });
            f.render_widget(paragraph, chunks[3]);
        })?;

        scroll += 1;
        scroll %= 10;

        if let Event::Input(key) = events.next()? {
            if key == Key::Char('q') {
                break;
            }
        }
    }
    Ok(())
}
