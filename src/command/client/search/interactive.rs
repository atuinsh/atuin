use std::{
    io::{stdout, Write},
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent},
    execute, terminal,
};
use eyre::Result;
use semver::Version;
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Span, Spans, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame, Terminal,
};
use unicode_width::UnicodeWidthStr;

use atuin_client::{
    database::current_context,
    database::Context,
    database::Database,
    history::History,
    settings::{ExitMode, FilterMode, SearchMode, Settings},
};

use super::{
    cursor::Cursor,
    history_list::{HistoryList, ListState, PREFIX_LENGTH},
};
use crate::VERSION;

const RETURN_ORIGINAL: usize = usize::MAX;
const RETURN_QUERY: usize = usize::MAX - 1;

struct State {
    history_count: i64,
    input: Cursor,
    filter_mode: FilterMode,
    results_state: ListState,
    context: Context,
    update_needed: Option<Version>,
}

impl State {
    async fn query_results(
        &mut self,
        search_mode: SearchMode,
        db: &mut impl Database,
    ) -> Result<Vec<History>> {
        let i = self.input.as_str();
        let results = if i.is_empty() {
            db.list(self.filter_mode, &self.context, Some(200), true)
                .await?
        } else {
            db.search(Some(200), search_mode, self.filter_mode, &self.context, i)
                .await?
        };

        self.results_state.select(0);
        Ok(results)
    }

    fn handle_input(&mut self, settings: &Settings, input: &Event, len: usize) -> Option<usize> {
        match input {
            Event::Key(k) => self.handle_key_input(settings, k, len),
            Event::Mouse(m) => self.handle_mouse_input(*m, len),
            _ => None,
        }
    }

    fn handle_mouse_input(&mut self, input: MouseEvent, len: usize) -> Option<usize> {
        match input.kind {
            event::MouseEventKind::ScrollDown => {
                let i = self.results_state.selected().saturating_sub(1);
                self.results_state.select(i);
            }
            event::MouseEventKind::ScrollUp => {
                let i = self.results_state.selected() + 1;
                self.results_state.select(i.min(len - 1));
            }
            _ => {}
        }
        None
    }

    fn handle_key_input(
        &mut self,
        settings: &Settings,
        input: &KeyEvent,
        len: usize,
    ) -> Option<usize> {
        let ctrl = input.modifiers.contains(KeyModifiers::CONTROL);
        let alt = input.modifiers.contains(KeyModifiers::ALT);
        match input.code {
            KeyCode::Char('c' | 'd' | 'g') if ctrl => return Some(RETURN_ORIGINAL),
            KeyCode::Esc => {
                return Some(match settings.exit_mode {
                    ExitMode::ReturnOriginal => RETURN_ORIGINAL,
                    ExitMode::ReturnQuery => RETURN_QUERY,
                })
            }
            KeyCode::Enter => {
                return Some(self.results_state.selected());
            }
            KeyCode::Char(c @ '1'..='9') if alt => {
                let c = c.to_digit(10)? as usize;
                return Some(self.results_state.selected() + c);
            }
            KeyCode::Left => {
                self.input.left();
            }
            KeyCode::Char('h') if ctrl => {
                self.input.left();
            }
            KeyCode::Right => self.input.right(),
            KeyCode::Char('l') if ctrl => self.input.right(),
            KeyCode::Char('a') if ctrl => self.input.start(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Backspace => {
                self.input.back();
            }
            KeyCode::Delete => {
                self.input.remove();
            }
            KeyCode::Char('w') if ctrl => {
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
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('r') if ctrl => {
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
            KeyCode::Down if self.results_state.selected() == 0 => return Some(RETURN_ORIGINAL),
            KeyCode::Down => {
                let i = self.results_state.selected().saturating_sub(1);
                self.results_state.select(i);
            }
            KeyCode::Char('n' | 'j') if ctrl => {
                let i = self.results_state.selected().saturating_sub(1);
                self.results_state.select(i);
            }
            KeyCode::Up => {
                let i = self.results_state.selected() + 1;
                self.results_state.select(i.min(len - 1));
            }
            KeyCode::Char('p' | 'k') if ctrl => {
                let i = self.results_state.selected() + 1;
                self.results_state.select(i.min(len - 1));
            }
            KeyCode::Char(c) => self.input.insert(c),
            _ => {}
        };

        None
    }

    #[allow(clippy::cast_possible_truncation)]
    fn draw<T: Backend>(&mut self, f: &mut Frame<'_, T>, results: &[History]) {
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

        let title = if self.update_needed.is_some() {
            let version = self.update_needed.clone().unwrap();

            Paragraph::new(Text::from(Span::styled(
                format!(" Atuin v{VERSION} - UPDATE AVAILABLE {version}"),
                Style::default().add_modifier(Modifier::BOLD).fg(Color::Red),
            )))
        } else {
            Paragraph::new(Text::from(Span::styled(
                format!(" Atuin v{VERSION}"),
                Style::default().add_modifier(Modifier::BOLD),
            )))
        };

        let help = vec![
            Span::raw(" Press "),
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" to exit."),
        ];

        let help = Paragraph::new(Text::from(Spans::from(help)));
        let stats = Paragraph::new(Text::from(Span::raw(format!(
            "history count: {} ",
            self.history_count
        ))));

        f.render_widget(title, top_left_chunks[1]);
        f.render_widget(help, top_left_chunks[2]);
        f.render_widget(stats.alignment(Alignment::Right), top_right_chunks[1]);

        let results = HistoryList::new(results).block(
            Block::default()
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Rounded),
        );

        f.render_stateful_widget(results, chunks[1], &mut self.results_state);

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
            chunks[2].x + width as u16 + PREFIX_LENGTH + 2,
            // Move one line down, from the border to the input line
            chunks[2].y + 1,
        );
    }

    #[allow(clippy::cast_possible_truncation)]
    fn draw_compact<T: Backend>(&mut self, f: &mut Frame<'_, T>, results: &[History]) {
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
            format!("Atuin v{VERSION}"),
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
            self.history_count,
        ))))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Right);

        f.render_widget(title, header_chunks[0]);
        f.render_widget(help, header_chunks[1]);
        f.render_widget(stats, header_chunks[2]);

        let results = HistoryList::new(results);
        f.render_stateful_widget(results, chunks[1], &mut self.results_state);

        let input = format!(
            "[{:^14}] {}",
            self.filter_mode.as_str(),
            self.input.as_str(),
        );
        let input = Paragraph::new(input);
        f.render_widget(input, chunks[2]);

        let extra_width = UnicodeWidthStr::width(self.input.substring());

        f.set_cursor(
            // Put cursor past the end of the input text
            chunks[2].x + extra_width as u16 + PREFIX_LENGTH + 1,
            // Move one line down, from the border to the input line
            chunks[2].y + 1,
        );
    }
}

struct Stdout {
    stdout: std::io::Stdout,
}

impl Stdout {
    pub fn new() -> std::io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(
            stdout,
            terminal::EnterAlternateScreen,
            event::EnableMouseCapture
        )?;
        Ok(Self { stdout })
    }
}

impl Drop for Stdout {
    fn drop(&mut self) {
        execute!(
            self.stdout,
            terminal::LeaveAlternateScreen,
            event::DisableMouseCapture
        )
        .unwrap();
        terminal::disable_raw_mode().unwrap();
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stdout.flush()
    }
}

// this is a big blob of horrible! clean it up!
// for now, it works. But it'd be great if it were more easily readable, and
// modular. I'd like to add some more stats and stuff at some point
#[allow(clippy::cast_possible_truncation)]
pub async fn history(
    query: &[String],
    settings: &Settings,
    db: &mut impl Database,
) -> Result<String> {
    let stdout = Stdout::new()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut input = Cursor::from(query.join(" "));
    // Put the cursor at the end of the query by default
    input.end();

    let update_needed = settings.needs_update().await;

    let mut app = State {
        history_count: db.history_count().await?,
        input,
        results_state: ListState::default(),
        context: current_context(),
        filter_mode: if settings.shell_up_key_binding {
            settings.filter_mode_shell_up_key_binding
        } else {
            settings.filter_mode
        },
        update_needed,
    };

    let mut results = app.query_results(settings.search_mode, db).await?;

    let index = 'render: loop {
        let compact = match settings.style {
            atuin_client::settings::Style::Auto => {
                terminal.size().map(|size| size.height < 14).unwrap_or(true)
            }
            atuin_client::settings::Style::Compact => true,
            atuin_client::settings::Style::Full => false,
        };
        if compact {
            terminal.draw(|f| app.draw_compact(f, &results))?;
        } else {
            terminal.draw(|f| app.draw(f, &results))?;
        }

        let initial_input = app.input.as_str().to_owned();
        let initial_filter_mode = app.filter_mode;

        if event::poll(Duration::from_millis(250))? {
            loop {
                if let Some(i) = app.handle_input(settings, &event::read()?, results.len()) {
                    break 'render i;
                }
                if !event::poll(Duration::ZERO)? {
                    break
                }
            }
        }

        if initial_input != app.input.as_str() || initial_filter_mode != app.filter_mode {
            results = app.query_results(settings.search_mode, db).await?;
        }
    };

    if index < results.len() {
        // index is in bounds so we return that entry
        Ok(results.swap_remove(index).command)
    } else if index == RETURN_ORIGINAL {
        Ok(String::new())
    } else {
        // Either:
        // * index == RETURN_QUERY, in which case we should return the input
        // * out of bounds -> usually implies no selected entry so we return the input
        Ok(app.input.into_inner())
    }
}
