use std::{
    io::{stdout, Write},
    ops::ControlFlow,
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent},
    execute, terminal,
};
use eyre::Result;
use futures_util::FutureExt;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame, Terminal,
};
use unicode_width::UnicodeWidthStr;

use atuin_client::{database::Database, settings::Settings};

use crate::command::client::search::{
    history_list::{HistoryList, PREFIX_LENGTH},
};
use crate::VERSION;

use super::core::{self, For, Line, To, Towards};

pub struct Skip;
impl TryFrom<Event> for core::Event {
    type Error = Skip;
    fn try_from(value: Event) -> Result<Self, Skip> {
        match value {
            Event::Key(key) => Self::try_from(key),
            Event::Mouse(mouse) => Self::try_from(mouse),
            Event::Paste(input) => Ok(Self::InputStr(input)),
            Event::FocusGained | Event::FocusLost | Event::Resize(_, _) => Err(Skip),
        }
    }
}
impl TryFrom<MouseEvent> for core::Event {
    type Error = Skip;
    fn try_from(value: MouseEvent) -> Result<Self, Skip> {
        match value.kind {
            event::MouseEventKind::ScrollDown => Ok(Self::Selection(Line::Down, For::SingleLine)),
            event::MouseEventKind::ScrollUp => Ok(Self::Selection(Line::Up, For::SingleLine)),
            _ => Err(Skip),
        }
    }
}
impl TryFrom<KeyEvent> for core::Event {
    type Error = Skip;
    fn try_from(input: KeyEvent) -> Result<Self, Skip> {
        let ctrl = input.modifiers.contains(KeyModifiers::CONTROL);
        let alt = input.modifiers.contains(KeyModifiers::ALT);
        match input.code {
            KeyCode::Down => Ok(Self::Selection(Line::Down, For::SingleLine)),
            KeyCode::Char('n' | 'j') if ctrl => Ok(Self::Selection(Line::Down, For::SingleLine)),
            KeyCode::Up => Ok(Self::Selection(Line::Up, For::SingleLine)),
            KeyCode::Char('p' | 'k') if ctrl => Ok(Self::Selection(Line::Up, For::SingleLine)),
            KeyCode::PageDown => Ok(Self::Selection(Line::Down, For::Page)),
            KeyCode::PageUp => Ok(Self::Selection(Line::Up, For::Page)),

            KeyCode::Left if ctrl => Ok(Self::Cursor(Towards::Left, To::Word)),
            KeyCode::Left => Ok(Self::Cursor(Towards::Left, To::Char)),
            KeyCode::Char('h') if ctrl => Ok(Self::Cursor(Towards::Left, To::Char)),
            KeyCode::Right if ctrl => Ok(Self::Cursor(Towards::Right, To::Word)),
            KeyCode::Right => Ok(Self::Cursor(Towards::Right, To::Char)),
            KeyCode::Char('l') if ctrl => Ok(Self::Cursor(Towards::Right, To::Char)),
            KeyCode::Char('a') if ctrl => Ok(Self::Cursor(Towards::Left, To::Edge)),
            KeyCode::Home => Ok(Self::Cursor(Towards::Left, To::Edge)),
            KeyCode::Char('e') if ctrl => Ok(Self::Cursor(Towards::Right, To::Edge)),
            KeyCode::End => Ok(Self::Cursor(Towards::Right, To::Edge)),

            KeyCode::Backspace if ctrl => Ok(Self::Delete(Towards::Left, To::Word)),
            KeyCode::Backspace => Ok(Self::Delete(Towards::Left, To::Char)),
            KeyCode::Delete if ctrl => Ok(Self::Delete(Towards::Right, To::Word)),
            KeyCode::Delete => Ok(Self::Delete(Towards::Right, To::Char)),
            KeyCode::Char('w') if ctrl => Ok(Self::Delete(Towards::Left, To::Word)),
            KeyCode::Char('u') if ctrl => Ok(Self::Clear),

            KeyCode::Char('c' | 'd' | 'g') if ctrl => Ok(Self::Cancel),
            KeyCode::Esc => Ok(Self::Exit),
            KeyCode::Enter => Ok(Self::SelectN(0)),
            KeyCode::Char(c @ '1'..='9') if alt => Ok(Self::SelectN(c.to_digit(10).unwrap())),
            KeyCode::Char('s') if ctrl => Ok(Self::CycleSearchMode),
            KeyCode::Char('r') if ctrl => Ok(Self::CycleFilterMode),
            KeyCode::Char(c) => Ok(Self::Input(c)),
            _ => Err(Skip),
        }
    }
}

struct UILayout {
    compact: bool,
    size: Rect,
    title: Rect,
    help: Rect,
    stats: Rect,
    list: Rect,
    input: Rect,
    preview: Rect,
}

impl UILayout {
    #[allow(clippy::bool_to_int_with_if, clippy::cast_possible_truncation)]
    fn new(size: Rect, compact: bool, preview_height: u16) -> Self {
        let border_size = if compact { 0 } else { 1 };
        let show_help = !compact || size.height > 1;
        let &[header, list, input, preview] = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .horizontal_margin(1)
            .constraints([
                Constraint::Length(if show_help { 1 } else { 0 }),
                Constraint::Min(1),
                Constraint::Length(1 + border_size),
                Constraint::Length(preview_height),
            ])
            .split(size)
            .as_ref()
        else {
            panic!("constraints broken")
        };

        let &[title, help, stats] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .split(header)
            .as_ref()
        else {
            panic!("constraints broken")
        };

        Self {
            compact,
            size,
            title,
            help,
            stats,
            list,
            input,
            preview,
        }
    }

    #[allow(clippy::bool_to_int_with_if, clippy::cast_possible_truncation)]
    fn render(&self, f: &mut Frame<'_, impl Backend>, mut view: core::View<'_>) {
        self.render_title(f, &view);
        self.render_help(f);
        self.render_stats(f, &view);
        self.render_results_list(f, &mut view);
        self.render_input(f, &view);
        self.render_preview(f, &view);

        let extra_width = UnicodeWidthStr::width(view.search.input.substring());

        let cursor_offset = if self.compact { 0 } else { 1 };
        f.set_cursor(
            // Put cursor past the end of the input text
            self.input.x + extra_width as u16 + PREFIX_LENGTH + 1 + cursor_offset,
            self.input.y + cursor_offset,
        );
    }

    fn render_title(&self, f: &mut Frame<'_, impl Backend>, view: &core::View<'_>) {
        let title = if view.update_needed.is_some() {
            let version = view.update_needed.unwrap();

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
        f.render_widget(title, self.title);
    }

    fn render_help(&self, f: &mut Frame<'_, impl Backend>) {
        let help = Paragraph::new(Text::from(Spans::from(vec![
            Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" to exit"),
        ])))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
        f.render_widget(help, self.help);
    }

    fn render_stats(&self, f: &mut Frame<'_, impl Backend>, view: &core::View<'_>) {
        let stats = Paragraph::new(Text::from(Span::raw(format!(
            "history count: {}",
            view.history_count,
        ))))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Right);
        f.render_widget(stats, self.stats);
    }

    fn render_results_list(&self, f: &mut Frame<'_, impl Backend>, view: &mut core::View<'_>) {
        let results_list = if self.compact {
            HistoryList::new(view.history)
        } else {
            HistoryList::new(view.history).block(
                Block::default()
                    .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded),
            )
        };
        f.render_stateful_widget(results_list, self.list, view.results_state);
    }

    fn render_input(&self, f: &mut Frame<'_, impl Backend>, view: &core::View<'_>) {
        let input = format!(
            "[{:^14}] {}",
            view.search.filter_mode.as_str(),
            view.search.input.as_str(),
        );
        let input = if self.compact {
            Paragraph::new(input)
        } else {
            Paragraph::new(input).block(
                Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded)
                    .title(format!(
                        "{:─>width$}",
                        "",
                        width = self.input.width as usize - 2
                    )),
            )
        };
        f.render_widget(input, self.input);
    }

    fn render_preview(&self, f: &mut Frame<'_, impl Backend>, view: &core::View<'_>) {
        let command = view.history[view.results_state.selected()].command.as_str();
        let command = if command.is_empty() {
            String::new()
        } else {
            use itertools::Itertools as _;
            command
                .char_indices()
                .step_by(self.preview.width.into())
                .map(|(i, _)| i)
                .chain(Some(command.len()))
                .tuple_windows()
                .map(|(a, b)| &command[a..b])
                .join("\n")
        };
        let preview = if self.compact {
            Paragraph::new(command).style(Style::default().fg(Color::DarkGray))
        } else {
            Paragraph::new(command).block(
                Block::default()
                    .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded)
                    .title(format!(
                        "{:─>width$}",
                        "",
                        width = self.preview.width as usize - 2
                    )),
            )
        };
        f.render_widget(preview, self.preview);
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

pub async fn history(query: &[String], settings: &Settings, db: impl Database) -> Result<String> {
    let stdout = Stdout::new()?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // start request for version
    let update_needed = settings.needs_update().fuse();
    tokio::pin!(update_needed);

    let mut app = core::State::new(query, settings.clone(), db).await?;

    let mut longest_command = app
        .history
        .iter()
        .map(|h| h.command.len())
        .max()
        .unwrap_or_default();

    let mut layout = None::<UILayout>;

    loop {
        render(
            &mut terminal,
            &mut app,
            settings,
            &mut layout,
            longest_command,
        )?;

        // handle events
        let event_ready = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(250)));
        let mut batch = app.start_batch();

        tokio::select! {
            event_ready = event_ready => {
                if event_ready?? {
                    loop {
                        if event::poll(Duration::ZERO)? {
                            let event = event::read()?;
                            if let Ok(event) = core::Event::try_from(event) {
                                match batch.handle(event) {
                                    ControlFlow::Continue(b) => batch = b,
                                    ControlFlow::Break(result) => return Ok(result),
                                }
                            }
                        } else {
                            break
                        }
                    }
                }
            }
            Some(update_needed) = &mut update_needed => {
                match batch.handle(core::Event::UpdateNeeded(update_needed)) {
                    ControlFlow::Continue(b) => batch = b,
                    ControlFlow::Break(result) => return Ok(result),
                }
            }
        };

        // invalidate layout if needed
        let did_update;
        (app, did_update) = batch.finish().await?;

        if did_update {
            longest_command = app
                .history
                .iter()
                .map(|h| h.command.len())
                .max()
                .unwrap_or_default();
        }
    }
}

#[allow(clippy::bool_to_int_with_if, clippy::cast_possible_truncation)]
fn render(
    terminal: &mut Terminal<impl Backend>,
    app: &mut core::State<impl Database>,
    settings: &Settings,
    layout: &mut Option<UILayout>,
    longest_command: usize,
) -> std::io::Result<()> {
    let compact = match settings.style {
        atuin_client::settings::Style::Auto => {
            terminal.size().map(|size| size.height < 14).unwrap_or(true)
        }
        atuin_client::settings::Style::Compact => true,
        atuin_client::settings::Style::Full => false,
    };

    // render terminal
    terminal.draw(|f| {
        let view = app.view();
        // recompute layout if resized
        let border_size = if compact { 0 } else { 1 };
        let preview_width = f.size().width - 2;
        let preview_height = if settings.show_preview {
            let width = preview_width - border_size;
            // div_ceil
            let height = (longest_command as u16 + width - 1) / width;
            height.clamp(0, 4) + border_size * 2
        } else {
            border_size
        };

        // invalidate layout of size or preview height changes
        if matches!(&layout, Some(l) if l.size != f.size()
            || (settings.show_preview && l.preview.height != preview_height))
        {
            *layout = None;
        }

        layout
            .get_or_insert(UILayout::new(f.size(), compact, preview_height))
            .render(f, view);
    })?;

    Ok(())
}
