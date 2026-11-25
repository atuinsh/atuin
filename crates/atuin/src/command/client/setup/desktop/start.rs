//! The page of `atuin setup` that offers to start desktop capture after
//! `atuin desktop-capture stop` stopped it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eyre::Result;
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use super::super::view::next_key;
use super::super::widgets::{button, hints, paragraph, title};

/// How the user left the start page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartOutcome {
    Start,
    LeaveStopped,
}

/// Waits on the start page until the user chooses whether to start desktop capture.
///
/// # Errors
///
/// Fails when drawing fails, or when reading a key fails.
pub(super) async fn ask(terminal: &mut DefaultTerminal) -> Result<StartOutcome> {
    let mut page = StartPage::default();
    loop {
        terminal.draw(|frame| page.draw(frame))?;
        if let Some(key) = next_key().await?
            && let Some(outcome) = page.handle(key)
        {
            return Ok(outcome);
        }
    }
}

/// The buttons of the start page, from the top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Button {
    #[default]
    Start,
    LeaveStopped,
}

/// The start page's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StartPage {
    button: Button,
}

impl StartPage {
    const EXPLANATION: [&str; 2] = [
        "Desktop capture is on in your config, but you stopped it earlier with `atuin \
         desktop-capture stop`. Capture stays stopped until you start it.",
        "If you leave it stopped, you can start it later with `atuin desktop-capture start`.",
    ];

    /// The outcome that `key` chooses, if any.
    fn handle(&mut self, key: KeyEvent) -> Option<StartOutcome> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(StartOutcome::LeaveStopped);
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Some(StartOutcome::LeaveStopped),
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => self.button = Button::Start,
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => self.button = Button::LeaveStopped,
            KeyCode::Enter | KeyCode::Char(' ') => {
                return Some(match self.button {
                    Button::Start => StartOutcome::Start,
                    Button::LeaveStopped => StartOutcome::LeaveStopped,
                });
            }
            _ => {}
        }
        None
    }

    fn draw(self, frame: &mut Frame<'_>) {
        let mut lines = title(Vec::new());
        lines.push(Line::from("  Desktop capture is stopped".bold()));
        lines.push(Line::default());
        for text in Self::EXPLANATION {
            lines.extend(paragraph(text, 2, frame.area().width, Style::default()));
            lines.push(Line::default());
        }
        lines.push(button(self.button == Button::Start, "Start desktop capture now"));
        lines.push(button(self.button == Button::LeaveStopped, "Leave it stopped"));
        lines.push(Line::default());
        lines.push(hints(&[("enter", "choose"), ("↑↓", "move"), ("esc", "leave it stopped")]));
        frame.render_widget(Paragraph::new(lines), frame.area());
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;
    use rstest::rstest;

    use super::*;

    const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

    #[rstest]
    #[case::enter_starts(&[KeyEvent::from(KeyCode::Enter)], Some(StartOutcome::Start))]
    #[case::leave(
        &[KeyEvent::from(KeyCode::Down), KeyEvent::from(KeyCode::Enter)],
        Some(StartOutcome::LeaveStopped)
    )]
    #[case::back_up_to_start(
        &[KeyEvent::from(KeyCode::Tab), KeyEvent::from(KeyCode::Up), KeyEvent::from(KeyCode::Enter)],
        Some(StartOutcome::Start)
    )]
    #[case::esc(&[KeyEvent::from(KeyCode::Esc)], Some(StartOutcome::LeaveStopped))]
    #[case::q(&[KeyEvent::from(KeyCode::Char('q'))], Some(StartOutcome::LeaveStopped))]
    #[case::ctrl_c(&[CTRL_C], Some(StartOutcome::LeaveStopped))]
    #[case::moving(&[KeyEvent::from(KeyCode::Down), KeyEvent::from(KeyCode::Up)], None)]
    fn keys_choose_an_outcome(#[case] keys: &[KeyEvent], #[case] expected: Option<StartOutcome>) {
        let mut page = StartPage::default();

        assert_eq!(keys.iter().find_map(|&key| page.handle(key)), expected);
    }

    #[rstest]
    fn the_page_says_how_to_start_capture_later() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let screen =
            terminal.draw(|frame| StartPage::default().draw(frame)).unwrap().buffer.clone();
        let rows = screen.content().chunks(80);
        let shown: Vec<String> =
            rows.map(|cells| cells.iter().map(Cell::symbol).collect()).collect();
        let text = shown.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");

        for expected in [
            "Desktop capture is stopped",
            "you stopped it earlier with `atuin desktop-capture stop`",
            "you can start it later with `atuin desktop-capture start`",
            "Start desktop capture now",
            "Leave it stopped",
            "esc leave it stopped",
        ] {
            assert!(text.contains(expected), "{expected:?} isn't in {shown:#?}");
        }
    }
}
