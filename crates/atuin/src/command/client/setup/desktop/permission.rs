//! The last page of `atuin setup` on macOS, which waits while the user grants desktop capture
//! the Accessibility permission.

use atuin_client::settings::Settings;
use atuin_daemon::Signing;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use eyre::Result;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use super::super::super::desktop_capture::{helper_name, request_access};
use super::super::view::next_key;
use super::super::widgets::{ACCENT, SPINNER, button, hints, paragraph, title};

/// How the user left the permission page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PermissionOutcome {
    Granted,
    TurnOff,
    /// The user grants the permission later, through another run of `atuin setup`.
    Later,
}

/// What a key on the permission page asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageAction {
    Leave(PermissionOutcome),
    /// Open System Settings again, through a new request of the helper.
    OpenAgain,
}

/// Waits on the permission page until the user grants desktop capture the Accessibility
/// permission, or leaves the page. `signing` prepares the helper that asks.
///
/// # Errors
///
/// Fails when the helper that asks for the permission fails, or when drawing fails.
pub(super) async fn ask(
    terminal: &mut DefaultTerminal,
    settings: &Settings,
    signing: Option<&Signing>,
) -> Result<PermissionOutcome> {
    let mut page = Permission::new(&helper_name(signing).await);
    // Dropping a request stops its helper, so each way out of the loop stops the helper.
    let mut request = request_access(settings, signing).await?;
    let mut tick = 0;
    loop {
        terminal.draw(|frame| page.draw(frame, tick))?;
        tick += 1;
        tokio::select! {
            granted = request.granted() => {
                granted?;
                return Ok(PermissionOutcome::Granted);
            }
            key = next_key() => {
                let Some(key) = key? else { continue };
                match page.handle(key) {
                    Some(PageAction::Leave(outcome)) => return Ok(outcome),
                    Some(PageAction::OpenAgain) => {
                        drop(request);
                        request = request_access(settings, signing).await?;
                    }
                    None => {}
                }
            }
        }
    }
}

/// The buttons of the permission page, from the top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Button {
    #[default]
    OpenAgain,
    TurnOff,
}

/// The permission page's state.
#[derive(Debug)]
struct Permission {
    /// The paragraphs that explain the permission, which name the helper.
    explanation: [String; 3],
    button: Button,
}

impl Permission {
    /// The page for the helper that System Settings lists as `helper`.
    fn new(helper: &str) -> Self {
        Self {
            explanation: [
                "macOS lets desktop capture read your windows only after an administrator grants \
                 it the Accessibility permission."
                    .to_owned(),
                format!(
                    "In System Settings → Privacy & Security → Accessibility, turn on {helper}. \
                     Setup opened System Settings for you."
                ),
                format!(
                    "If macOS asks whether {helper} may control this computer, choose Open System \
                     Settings."
                ),
            ],
            button: Button::default(),
        }
    }

    /// What `key` asks for, if anything.
    fn handle(&mut self, key: KeyEvent) -> Option<PageAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(PageAction::Leave(PermissionOutcome::Later));
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                return Some(PageAction::Leave(PermissionOutcome::Later));
            }
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => self.button = Button::OpenAgain,
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => self.button = Button::TurnOff,
            KeyCode::Enter | KeyCode::Char(' ') => {
                return Some(match self.button {
                    Button::OpenAgain => PageAction::OpenAgain,
                    Button::TurnOff => PageAction::Leave(PermissionOutcome::TurnOff),
                });
            }
            _ => {}
        }
        None
    }

    fn draw(&self, frame: &mut Frame<'_>, tick: usize) {
        let mut lines = title(Vec::new());
        lines.push(Line::from("  Desktop capture needs a permission".bold()));
        lines.push(Line::default());
        for text in &self.explanation {
            lines.extend(paragraph(text, 2, frame.area().width, Style::default()));
            lines.push(Line::default());
        }
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(SPINNER[tick % SPINNER.len()]).fg(ACCENT),
            Span::raw(" Waiting for the permission…"),
        ]));
        lines.push(Line::default());
        lines.push(button(self.button == Button::OpenAgain, "Open System Settings again"));
        lines.push(button(self.button == Button::TurnOff, "Turn off desktop capture"));
        lines.push(Line::default());
        lines.push(hints(&[("enter", "choose"), ("↑↓", "move"), ("esc", "grant it later")]));
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

    fn page() -> Permission {
        Permission::new("atuin-desktop-capture (dev1)")
    }

    fn press(page: &mut Permission, keys: &[KeyEvent]) -> Option<PageAction> {
        keys.iter().find_map(|&key| page.handle(key))
    }

    /// The text of each row of the page, drawn 80 columns wide and 24 rows tall.
    fn shown(page: &Permission) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let screen = terminal.draw(|frame| page.draw(frame, 0)).unwrap().buffer.clone();
        let rows = screen.content().chunks(80);
        rows.map(|cells| cells.iter().map(Cell::symbol).collect()).collect()
    }

    const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

    const LATER: Option<PageAction> = Some(PageAction::Leave(PermissionOutcome::Later));

    #[rstest]
    #[case::esc(&[KeyEvent::from(KeyCode::Esc)], LATER)]
    #[case::q(&[KeyEvent::from(KeyCode::Char('q'))], LATER)]
    #[case::ctrl_c(&[CTRL_C], LATER)]
    #[case::turn_off(
        &[KeyEvent::from(KeyCode::Down), KeyEvent::from(KeyCode::Enter)],
        Some(PageAction::Leave(PermissionOutcome::TurnOff))
    )]
    #[case::open_again(&[KeyEvent::from(KeyCode::Enter)], Some(PageAction::OpenAgain))]
    #[case::back_up_to_open_again(
        &[KeyEvent::from(KeyCode::Tab), KeyEvent::from(KeyCode::Up), KeyEvent::from(KeyCode::Enter)],
        Some(PageAction::OpenAgain)
    )]
    #[case::moving(&[KeyEvent::from(KeyCode::Down), KeyEvent::from(KeyCode::Up)], None)]
    fn keys_choose_an_action(#[case] keys: &[KeyEvent], #[case] expected: Option<PageAction>) {
        assert_eq!(press(&mut page(), keys), expected);
    }

    #[rstest]
    fn the_page_says_where_to_grant_the_permission() {
        let shown = shown(&page());
        let text = shown.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");

        for expected in [
            "Desktop capture needs a permission",
            "System Settings → Privacy & Security → Accessibility, turn on atuin-desktop-capture \
             (dev1).",
            "If macOS asks whether atuin-desktop-capture (dev1) may control this computer",
            "Waiting for the permission…",
            "Open System Settings again",
            "Turn off desktop capture",
            "esc grant it later",
        ] {
            assert!(text.contains(expected), "{expected:?} isn't in {shown:#?}");
        }
    }
}
