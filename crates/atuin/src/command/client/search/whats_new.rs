use std::sync::Arc;

use atuin_client::settings::Settings;
use eyre::Result;
use ratatui::Frame;
use ratatui::backend::FromCrossterm;
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use semver::Version;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use unicode_width::UnicodeWidthStr;

use crate::command::client::theme::{Meaning, Theme};

const NOTES: &str = include_str!("../../../../whats-new.txt");

const MAX_WIDTH: u16 = 68;
const MIN_WIDTH: u16 = 40;
/// The width taken by the borders and the padding inside them.
const CHROME_WIDTH: u16 = 4;
const CHROME_HEIGHT: u16 = 2;

/// Hand-written release notes, drawn over the middle of the search until the first input.
///
/// This is only ever a drawn layer. It has no keys of its own: the input that closes it is
/// handled by the search exactly as if it had never been there.
pub struct WhatsNew {
    version: Version,
    items: Vec<String>,
    drawn: bool,
    recorded: bool,
}

impl WhatsNew {
    pub async fn load(settings: &Settings) -> Result<Option<Self>> {
        if !settings.show_whats_new {
            return Ok(None);
        }
        let meta = settings.meta_store().await?;
        let seen = meta.whats_new_seen().await?;

        let Some((version, items)) = parse(NOTES) else {
            // Leave a mark even without notes, so this machine isn't taken for a fresh install
            // when notes do ship.
            if seen.is_none() {
                meta.save_whats_new_seen("0.0.0").await?;
            }
            return Ok(None);
        };

        // If nothing has been seen and no update check has ever run, this is a fresh install.
        // Nothing is new to a fresh install, so it starts with these notes already seen.
        if seen.is_none() && meta.last_version_check().await? == OffsetDateTime::UNIX_EPOCH {
            meta.save_whats_new_seen(&version.to_string()).await?;
            return Ok(None);
        }
        if seen.and_then(|seen| Version::parse(&seen).ok()).is_some_and(|seen| seen >= version) {
            return Ok(None);
        }

        Ok(Some(Self {
            version,
            items,
            drawn: false,
            recorded: false,
        }))
    }

    /// Remember these notes as seen, once they have actually been on screen.
    ///
    /// The write runs in the background, so a slow meta.db never holds up the search. The
    /// caller keeps the handle and waits for it on exit, so a quick exit doesn't lose it.
    pub fn record_if_drawn(&mut self, settings: &Arc<Settings>) -> Option<JoinHandle<()>> {
        if !self.drawn || self.recorded {
            return None;
        }
        self.recorded = true;
        let settings = Arc::clone(settings);
        let version = self.version.to_string();
        Some(tokio::spawn(async move {
            let saved = async { settings.meta_store().await?.save_whats_new_seen(&version).await };
            if let Err(e) = saved.await {
                tracing::warn!("failed to record what's new as seen: {e:#}");
            }
        }))
    }

    /// Draw centred on `screen`. If they don't fit, the notes wait for a later search.
    pub fn draw(&mut self, f: &mut Frame, screen: Rect, theme: &Theme) {
        let width = screen.width.min(MAX_WIDTH);
        if width < MIN_WIDTH {
            return;
        }

        let lines = self.lines(usize::from(width - CHROME_WIDTH), theme);
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX).saturating_add(CHROME_HEIGHT);
        if height > screen.height {
            return;
        }

        let x = screen.x + (screen.width - width) / 2;
        let y = screen.y + (screen.height - height) / 2;
        let area = Rect::new(x, y, width, height);

        let style = |meaning| Style::from_crossterm(theme.as_style(meaning));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(style(Meaning::Muted))
            .title(Span::styled(
                format!(" What's new in Atuin {} ", self.version),
                style(Meaning::Important).add_modifier(Modifier::BOLD),
            ))
            .padding(Padding::horizontal(1));

        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(block), area);
        self.drawn = true;
    }

    /// Each item is a paragraph, with `code` and **bold** picked out.
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let style = |meaning| Style::from_crossterm(theme.as_style(meaning));
        let base = style(Meaning::Base);
        let code = style(Meaning::SyntaxCommand);

        let mut lines = Vec::new();
        for item in &self.items {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.extend(wrap(item, width, base, code));
        }
        lines
    }
}

/// Whether this input closes the notes: any real key, paste or click, but not a bare modifier,
/// a key release, mouse movement or a resize.
pub fn dismisses(event: &Event) -> bool {
    match event {
        Event::Key(k) => k.kind != KeyEventKind::Release && !matches!(k.code, KeyCode::Modifier(_)),
        Event::Paste(_) => true,
        Event::Mouse(m) => m.kind != MouseEventKind::Moved,
        _ => false,
    }
}

/// Read the version and the items, each a paragraph, from the notes file.
///
/// Comment lines are dropped first. Any run of blank lines then separates two items.
fn parse(file: &str) -> Option<(Version, Vec<String>)> {
    let mut lines = file.lines().map(str::trim).filter(|line| !line.starts_with('#'));
    let version = Version::parse(lines.find(|line| !line.is_empty())?).ok()?;
    let lines = lines.collect::<Vec<_>>();
    let items = lines
        .split(|line| line.is_empty())
        .filter(|item| !item.is_empty())
        .map(|item| item.join(" "))
        .collect::<Vec<_>>();

    (!items.is_empty()).then_some((version, items))
}

fn wrap(text: &str, width: usize, base: Style, code: Style) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut column = 0;
    let mut in_code = false;
    let mut bold = false;
    let style = |in_code: bool, bold: bool| {
        let style = if in_code {
            code
        } else {
            base
        };
        if bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    };

    for word in text.split_whitespace() {
        let space = style(in_code, bold);
        // Each run of text with its style, after the `code` and **bold** markers in the word.
        let mut parts = Vec::new();
        let mut rest = word;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('`') {
                in_code = !in_code;
                rest = after;
                continue;
            }
            if !in_code && let Some(after) = rest.strip_prefix("**") {
                bold = !bold;
                rest = after;
                continue;
            }
            let end = rest
                .char_indices()
                .skip(1)
                .map(|(i, _)| i)
                .find(|&i| rest[i..].starts_with('`') || (!in_code && rest[i..].starts_with("**")))
                .unwrap_or(rest.len());
            parts.push(Span::styled(rest[..end].to_string(), style(in_code, bold)));
            rest = &rest[end..];
        }
        let word_width = parts.iter().map(|part| part.content.width()).sum::<usize>();

        if column > 0 && column + 1 + word_width > width {
            lines.push(Line::from(std::mem::take(&mut line)));
            column = 0;
        }
        if column > 0 {
            line.push(Span::styled(" ", space));
            column += 1;
        }
        line.extend(parts);
        column += word_width;
    }
    if !line.is_empty() {
        lines.push(Line::from(line));
    }
    lines
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn the_shipped_notes_parse_or_are_empty() {
        let empty = NOTES.lines().all(|line| line.trim().is_empty() || line.starts_with('#'));
        assert!(
            empty || parse(NOTES).is_some(),
            "whats-new.txt needs a version line and at least one item, or nothing but comments"
        );
    }

    #[rstest]
    #[case::plain("1.2.3\n\nOne\nfirst body\n\nTwo\nsecond\nbody\n")]
    #[case::comment_between_items("1.2.3\n\nOne\nfirst body\n\n# note\n\nTwo\nsecond\nbody\n")]
    #[case::stray_whitespace("\n  1.2.3 \n \nOne  \nfirst body\n   \n\n\nTwo\n  second\nbody")]
    fn items_are_separated_by_blank_lines(#[case] file: &str) {
        let (version, items) = parse(file).expect("the notes should parse");
        assert_eq!(version, Version::new(1, 2, 3));
        assert_eq!(items, ["One first body", "Two second body"]);
    }

    #[rstest]
    fn markers_pick_out_bold_and_code() {
        let base = Style::default();
        let code = Style::default().fg(ratatui::style::Color::Green);
        let bold = base.add_modifier(Modifier::BOLD);
        let lines = wrap("**Atuin Threads** try `atuin resume`. a*b", 80, base, code);
        let spans =
            lines[0].spans.iter().map(|s| (s.content.as_ref(), s.style)).collect::<Vec<_>>();
        assert_eq!(spans, [
            ("Atuin", bold),
            (" ", bold),
            ("Threads", bold),
            (" ", base),
            ("try", base),
            (" ", base),
            ("atuin", code),
            (" ", code),
            ("resume", code),
            (".", base),
            (" ", base),
            ("a*b", base),
        ]);
    }

    #[rstest]
    #[case::bad_version("1.2\n\nOne\nbody\n")]
    #[case::no_items("1.2.3\n# Only a comment.\n\n")]
    fn unusable_notes_show_nothing(#[case] file: &str) {
        assert!(parse(file).is_none());
    }
}
