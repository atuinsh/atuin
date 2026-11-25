//! The page of `atuin setup` that says that another `atuin` binary will take the daemon, and so
//! stop desktop capture. That binary comes first on `PATH`, or `brew services` runs it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eyre::Result;
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use super::super::view::next_key;
use super::super::widgets::{button, hints, paragraph, title};

/// Shows `explanation` until the user moves on.
///
/// # Errors
///
/// Fails when drawing fails, or when reading a key fails.
pub(super) async fn show(terminal: &mut DefaultTerminal, explanation: &str) -> Result<()> {
    let page = PathPage { explanation };
    loop {
        terminal.draw(|frame| page.draw(frame))?;
        if let Some(key) = next_key().await?
            && PathPage::closes(key)
        {
            return Ok(());
        }
    }
}

/// The page that explains which `atuin` binary will take the daemon.
#[derive(Clone, Copy, Debug)]
struct PathPage<'a> {
    explanation: &'a str,
}

impl PathPage<'_> {
    /// Whether `key` moves on from the page.
    fn closes(key: KeyEvent) -> bool {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        ctrl_c || matches!(key.code, KeyCode::Enter | KeyCode::Char(' ' | 'q') | KeyCode::Esc)
    }

    fn draw(self, frame: &mut Frame<'_>) {
        let mut lines = title(Vec::new());
        lines.push(Line::from("  Another atuin will stop desktop capture".bold()));
        lines.push(Line::default());
        lines.extend(paragraph(self.explanation, 2, frame.area().width, Style::default()));
        lines.push(Line::default());
        lines.extend(paragraph(
            "The summary of setup repeats this.",
            2,
            frame.area().width,
            Style::default(),
        ));
        lines.push(Line::default());
        lines.push(button(true, "Continue"));
        lines.push(Line::default());
        lines.push(hints(&[("enter", "continue")]));
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

    #[rstest]
    #[case::enter(KeyEvent::from(KeyCode::Enter), true)]
    #[case::esc(KeyEvent::from(KeyCode::Esc), true)]
    #[case::ctrl_c(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), true)]
    #[case::down(KeyEvent::from(KeyCode::Down), false)]
    fn keys_close_the_page(#[case] key: KeyEvent, #[case] closes: bool) {
        assert_eq!(PathPage::closes(key), closes);
    }

    #[rstest]
    fn the_page_shows_the_explanation() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let page = PathPage {
            explanation: "PATH runs /opt/released/atuin, another atuin.",
        };
        let screen = terminal.draw(|frame| page.draw(frame)).unwrap().buffer.clone();
        let rows = screen.content().chunks(80);
        let shown: Vec<String> =
            rows.map(|cells| cells.iter().map(Cell::symbol).collect()).collect();
        let text = shown.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");

        for expected in [
            "Another atuin will stop desktop capture",
            "PATH runs /opt/released/atuin, another atuin.",
            "Continue",
        ] {
            assert!(text.contains(expected), "{expected:?} isn't in {shown:#?}");
        }
    }
}
