//! Markdown rendering component using pulldown-cmark.
//!
//! More robust than eye-declare's built-in Markdown component:
//! uses a proper CommonMark parser rather than line-by-line regex.

use eye_declare::{Component, props};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Widget,
};
use ratatui_widgets::paragraph::{Paragraph, Wrap};

/// A markdown rendering component backed by pulldown-cmark.
#[props]
pub struct Markdown {
    pub source: String,
}

impl Markdown {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

/// Style configuration for markdown rendering.
pub struct MarkdownStyles {
    pub base: Style,
    pub code_inline: Style,
    pub code_block: Style,
    pub bold: Style,
    pub italic: Style,
    pub heading: Style,
}

impl MarkdownStyles {
    pub fn new() -> Self {
        let base = Style::default();
        Self {
            base,
            code_inline: Style::default().fg(Color::Yellow),
            code_block: Style::default().fg(Color::Green),
            bold: base.add_modifier(Modifier::BOLD),
            italic: base.add_modifier(Modifier::ITALIC),
            heading: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        }
    }
}

impl Default for MarkdownStyles {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Markdown {
    type State = MarkdownStyles;

    fn render(&self, area: Rect, buf: &mut Buffer, state: &Self::State) {
        if self.source.is_empty() || area.width == 0 || area.height == 0 {
            return;
        }
        let text = parse_markdown(&self.source, state);
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn desired_height(&self, width: u16, state: &Self::State) -> Option<u16> {
        if self.source.is_empty() || width == 0 {
            return Some(0);
        }
        let text = parse_markdown(&self.source, state);
        Some(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .line_count(width) as u16,
        )
    }

    fn initial_state(&self) -> Option<MarkdownStyles> {
        Some(MarkdownStyles::new())
    }
}

/// Parse markdown source into styled ratatui Text using pulldown-cmark.
fn parse_markdown<'a>(source: &'a str, styles: &'a MarkdownStyles) -> Text<'static> {
    let parser = Parser::new(source);
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut current_line = 0;

    let mut style_stack: Vec<Style> = vec![styles.base];
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(Tag::Strong) => {
                let bold = style_stack
                    .last()
                    .copied()
                    .unwrap_or(styles.base)
                    .add_modifier(Modifier::BOLD);
                style_stack.push(bold);
            }
            Event::End(TagEnd::Strong) => {
                style_stack.pop();
            }
            Event::Start(Tag::Emphasis) => {
                let italic = style_stack
                    .last()
                    .copied()
                    .unwrap_or(styles.base)
                    .add_modifier(Modifier::ITALIC);
                style_stack.push(italic);
            }
            Event::End(TagEnd::Emphasis) => {
                style_stack.pop();
            }
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                if !lines[current_line].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                if !lines[current_line].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::Code(code) => {
                lines[current_line].push(Span::styled(format!("{}", code), styles.code_inline));
            }
            Event::Text(text) => {
                let current_style = if in_code_block {
                    styles.code_block
                } else {
                    style_stack.last().copied().unwrap_or(styles.base)
                };
                let prefix = if in_code_block { "  " } else { "" };
                let parts: Vec<&str> = text.split('\n').collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        current_line += 1;
                        lines.push(Vec::new());
                    }
                    if !part.is_empty() {
                        lines[current_line]
                            .push(Span::styled(format!("{}{}", prefix, part), current_style));
                    }
                }
            }
            Event::SoftBreak => {
                let current_style = style_stack.last().copied().unwrap_or(styles.base);
                lines[current_line].push(Span::styled(" ", current_style));
            }
            Event::HardBreak => {
                current_line += 1;
                lines.push(Vec::new());
            }
            Event::Start(Tag::Paragraph) => {
                if current_line > 0 || !lines[0].is_empty() {
                    // Two line advances: one to end the current line, one for a blank separator.
                    current_line += 1;
                    lines.push(Vec::new());
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::End(TagEnd::Paragraph) => {}
            Event::Start(Tag::Heading { .. }) => {
                if current_line > 0 || !lines[0].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                    current_line += 1;
                    lines.push(Vec::new());
                }
                style_stack.push(styles.heading);
            }
            Event::End(TagEnd::Heading(_)) => {
                style_stack.pop();
            }
            Event::Start(Tag::Item) => {
                if current_line > 0 || !lines[0].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                }
                lines[current_line].push(Span::styled("- ", Style::default().fg(Color::DarkGray)));
            }
            Event::End(TagEnd::Item) => {}
            Event::Start(Tag::List(_)) => {
                if current_line > 0 || !lines[0].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::End(TagEnd::List(_)) => {}
            _ => {}
        }
    }

    let text_lines: Vec<Line<'static>> = lines.into_iter().map(Line::from).collect();
    Text::from(text_lines)
}
