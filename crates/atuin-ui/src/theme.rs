use ratatui::style::{Color, Style};

/// The UI's own theme, expressed directly in ratatui [`Style`]s.
///
/// The `Default` fills the syntax / time / duration roles with sensible ANSI
/// palette colours (which follow the user's terminal scheme), so callers only
/// override what they care about, e.g. `Theme { base, ..Default::default() }`.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Default style for general text.
    pub base: Style,
    /// Attention / error style (e.g. the update-available title variant).
    pub error: Style,
    /// Secondary / annotation style (shortcut hints, stats, placeholders).
    pub annotation: Style,
    /// Relative-time ("… ago") colour.
    pub time: Style,
    /// Duration colour for a successful command.
    pub duration_ok: Style,
    /// Duration colour for a failed command.
    pub duration_err: Style,
    /// Shell-syntax highlighting colours for command text.
    pub syntax: SyntaxTheme,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            base: Style::default(),
            error: Style::default(),
            annotation: Style::default(),
            time: Style::default().fg(Color::Blue),
            duration_ok: Style::default().fg(Color::Green),
            duration_err: Style::default().fg(Color::Red),
            syntax: SyntaxTheme::default(),
        }
    }
}

/// Colours for the shell-syntax categories the highlighter recognises.
#[derive(Debug, Clone)]
pub struct SyntaxTheme {
    pub command: Style,
    pub flag: Style,
    pub string: Style,
    pub variable: Style,
    pub operator: Style,
    pub comment: Style,
}

impl Default for SyntaxTheme {
    fn default() -> Self {
        Self {
            command: Style::default().fg(Color::Green),
            flag: Style::default().fg(Color::Cyan),
            string: Style::default().fg(Color::Yellow),
            variable: Style::default().fg(Color::Magenta),
            operator: Style::default(),
            comment: Style::default().fg(Color::DarkGray),
        }
    }
}
