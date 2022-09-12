use std::{io::stdout, ops::Sub, time::Duration};

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
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use unicode_width::UnicodeWidthStr;

use atuin_client::{
    database::current_context,
    database::Context,
    database::Database,
    history::History,
    settings::{FilterMode, SearchMode},
};

use super::{
    cursor::Cursor,
    event::{Event, Events},
    format_duration,
};
use crate::VERSION;

struct State {
    input: Cursor,

    filter_mode: FilterMode,

    results: Vec<History>,

    results_state: ListState,

    context: Context,
}

fn duration(h: &History) -> String {
    let duration = Duration::from_nanos(u64::try_from(h.duration).unwrap_or(0));
    format_duration(duration)
}

fn ago(h: &History) -> String {
    let ago = chrono::Utc::now().sub(h.timestamp);

    // Account for the chance that h.timestamp is "in the future"
    // This would mean that "ago" is negative, and the unwrap here
    // would fail.
    // If the timestamp would otherwise be in the future, display
    // the time ago as 0.
    let ago = ago.to_std().unwrap_or_default();
    format_duration(ago) + " ago"
}

impl State {
    fn render_results<T: tui::backend::Backend>(
        &mut self,
        f: &mut tui::Frame<T>,
        r: tui::layout::Rect,
        b: tui::widgets::Block,
    ) {
        let max_length = 12; // '123ms' + '59s ago'

        let results: Vec<ListItem> = self
            .results
            .iter()
            .enumerate()
            .map(|(i, m)| {
                // these encode the slices of `" > "`, `" {n} "`, or `"   "` in a compact form.
                // Yes, this is a hack, but it makes me feel happy
                let slices = " > 1 2 3 4 5 6 7 8 9   ";
                let index = self.results_state.selected().and_then(|s| i.checked_sub(s));
                let slice_index = index.unwrap_or(10).min(10) * 2;

                let status_colour = if m.success() {
                    Color::Green
                } else {
                    Color::Red
                };
                let ago = ago(m);
                let duration = format!("{:width$}", duration(m), width = max_length - ago.len());

                let command = m.command.replace(['\n', '\t'], " ");
                let mut command = Span::raw(command);
                if slice_index == 0 {
                    command.style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
                }

                let spans = Spans::from(vec![
                    Span::raw(&slices[slice_index..slice_index + 3]),
                    Span::styled(duration, Style::default().fg(status_colour)),
                    Span::raw(" "),
                    Span::styled(ago, Style::default().fg(Color::Blue)),
                    Span::raw(" "),
                    command,
                ]);

                ListItem::new(spans)
            })
            .collect();

        let results = List::new(results).block(b).start_corner(Corner::BottomLeft);

        f.render_stateful_widget(results, r, &mut self.results_state);
    }
}

impl State {
    async fn query_results(
        &mut self,
        search_mode: SearchMode,
        db: &mut impl Database,
    ) -> Result<()> {
        let i = self.input.as_str();
        let results = if i.is_empty() {
            db.list(self.filter_mode, &self.context, Some(200), true)
                .await?
        } else {
            db.search(Some(200), search_mode, self.filter_mode, &self.context, i)
                .await?
        };

        self.results = results;

        if self.results.is_empty() {
            self.results_state.select(None);
        } else {
            self.results_state.select(Some(0));
        }

        Ok(())
    }

    fn handle_input(&mut self, input: &TermEvent) -> Option<&str> {
        match input {
            TermEvent::Key(Key::Esc | Key::Ctrl('c' | 'd' | 'g')) => return Some(""),
            TermEvent::Key(Key::Char('\n')) => {
                let i = self.results_state.selected().unwrap_or(0);

                return Some(
                    self.results
                        .get(i)
                        .map_or(self.input.as_str(), |h| h.command.as_str()),
                );
            }
            TermEvent::Key(Key::Alt(c @ '1'..='9')) => {
                let c = c.to_digit(10)? as usize;
                let i = self.results_state.selected()? + c;

                return Some(
                    self.results
                        .get(i)
                        .map_or(self.input.as_str(), |h| h.command.as_str()),
                );
            }
            TermEvent::Key(Key::Left | Key::Ctrl('h')) => {
                self.input.left();
            }
            TermEvent::Key(Key::Right | Key::Ctrl('l')) => self.input.right(),
            TermEvent::Key(Key::Ctrl('a')) => self.input.start(),
            TermEvent::Key(Key::Ctrl('e')) => self.input.end(),
            TermEvent::Key(Key::Char(c)) => self.input.insert(*c),
            TermEvent::Key(Key::Backspace) => {
                self.input.back();
            }
            TermEvent::Key(Key::Ctrl('w')) => {
                // remove the first batch of whitespace
                while matches!(self.input.back(), Some(c) if c.is_whitespace()) {}
                while self.input.left() {
                    if self.input.char().unwrap().is_whitespace() {
                        self.input.right(); // found whitespace, go back right
                        break;
                    }
                    self.input.remove();
                }
            }
            TermEvent::Key(Key::Ctrl('u')) => self.input.clear(),
            TermEvent::Key(Key::Ctrl('r')) => {
                pub static FILTER_MODES: [FilterMode; 4] = [
                    FilterMode::Global,
                    FilterMode::Host,
                    FilterMode::Session,
                    FilterMode::Directory,
                ];
                let i = self.filter_mode as usize;
                let i = (i + 1) % FILTER_MODES.len();
                self.filter_mode = FILTER_MODES[i];
            }
            TermEvent::Key(Key::Down | Key::Ctrl('n' | 'j'))
            | TermEvent::Mouse(MouseEvent::Press(MouseButton::WheelDown, _, _)) => {
                let i = self
                    .results_state
                    .selected() // try get current selection
                    .map_or(0, |i| i.saturating_sub(1)); // subtract 1 if possible
                self.results_state.select(Some(i));
            }
            TermEvent::Key(Key::Up | Key::Ctrl('p' | 'k'))
            | TermEvent::Mouse(MouseEvent::Press(MouseButton::WheelUp, _, _)) => {
                let i = self
                    .results_state
                    .selected()
                    .map_or(0, |i| i + 1) // increment the selected index
                    .min(self.results.len() - 1); // clamp it to the last entry
                self.results_state.select(Some(i));
            }
            _ => {}
        };

        None
    }

    #[allow(clippy::cast_possible_truncation)]
    fn draw<T: Backend>(&mut self, f: &mut Frame<'_, T>, history_count: i64) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .split(f.size());

        let top_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50); 2])
            .split(chunks[0]);

        let top_left_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1); 3])
            .split(top_chunks[0]);

        let top_right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1); 3])
            .split(top_chunks[1]);

        let title = Paragraph::new(Text::from(Span::styled(
            format!(" Atuin v{VERSION}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));

        let help = vec![
            Span::raw(" Press "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" to exit."),
        ];

        let help = Paragraph::new(Text::from(Spans::from(help)));
        let stats = Paragraph::new(Text::from(Span::raw(format!(
            "history count: {history_count} ",
        ))));

        f.render_widget(title, top_left_chunks[1]);
        f.render_widget(help, top_left_chunks[2]);
        f.render_widget(stats.alignment(Alignment::Right), top_right_chunks[1]);

        self.render_results(
            f,
            chunks[1],
            Block::default()
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Rounded), // .title("History"),
        );

        let input = format!(
            "[{:^14}] {}",
            self.filter_mode.as_str(),
            self.input.as_str(),
        );
        let input = Paragraph::new(input).block(
            Block::default()
                .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Rounded)
                .title(format!(
                    "{:─>width$}",
                    "",
                    width = chunks[2].width as usize - 2
                )),
        );
        f.render_widget(input, chunks[2]);

        let width = UnicodeWidthStr::width(self.input.substring());
        f.set_cursor(
            // Put cursor past the end of the input text
            chunks[2].x + width as u16 + 18,
            // Move one line down, from the border to the input line
            chunks[2].y + 1,
        );
    }

    #[allow(clippy::cast_possible_truncation)]
    fn draw_compact<T: Backend>(&mut self, f: &mut Frame<'_, T>, history_count: i64) {
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

        f.render_widget(title, header_chunks[0]);
        f.render_widget(help, header_chunks[1]);
        f.render_widget(stats, header_chunks[2]);

        self.render_results(f, chunks[1], Block::default());

        let input = format!(
            "[{:^14}] {}",
            self.filter_mode.as_str(),
            self.input.as_str(),
        );
        let input = Paragraph::new(input).block(Block::default());
        f.render_widget(input, chunks[2]);

        let extra_width = UnicodeWidthStr::width(self.input.substring());

        f.set_cursor(
            // Put cursor past the end of the input text
            chunks[2].x + extra_width as u16 + 17,
            // Move one line down, from the border to the input line
            chunks[2].y + 1,
        );
    }
}

// this is a big blob of horrible! clean it up!
// for now, it works. But it'd be great if it were more easily readable, and
// modular. I'd like to add some more stats and stuff at some point
#[allow(clippy::cast_possible_truncation)]
pub async fn history(
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

    app.query_results(search_mode, db).await?;

    loop {
        let history_count = db.history_count().await?;
        let initial_input = app.input.as_str().to_owned();
        let initial_filter_mode = app.filter_mode;

        // Handle input
        if let Event::Input(input) = events.next()? {
            if let Some(output) = app.handle_input(&input) {
                return Ok(output.to_owned());
            }
        }

        // After we receive input process the whole event channel before query/render.
        while let Ok(Event::Input(input)) = events.try_next() {
            if let Some(output) = app.handle_input(&input) {
                return Ok(output.to_owned());
            }
        }

        if initial_input != app.input.as_str() || initial_filter_mode != app.filter_mode {
            app.query_results(search_mode, db).await?;
        }

        let compact = match style {
            atuin_client::settings::Style::Auto => {
                terminal.size().map(|size| size.height < 14).unwrap_or(true)
            }
            atuin_client::settings::Style::Compact => true,
            atuin_client::settings::Style::Full => false,
        };
        if compact {
            terminal.draw(|f| app.draw_compact(f, history_count))?;
        } else {
            terminal.draw(|f| app.draw(f, history_count))?;
        }
    }
}
