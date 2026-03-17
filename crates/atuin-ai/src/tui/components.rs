//! Leaf components for each content type and factory functions for building
//! the component tree from the view model.

use atuin_client::theme::{Meaning, Theme};
use ratatui::{
    Frame,
    backend::FromCrossterm,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::component::{Component, RenderContext, Separator, Spacer, SymbolRow, VStack};
use super::spinner::active_frame;
use super::view_model::{Block, Content, WarningKind};

// ---------------------------------------------------------------------------
// Text measurement utilities
// ---------------------------------------------------------------------------

/// Count lines when text is wrapped at given width.
/// Uses ratatui's Paragraph::line_count for accurate wrapping calculation.
pub(crate) fn line_count_wrapped(text: &str, width: usize) -> u16 {
    if width == 0 {
        return 1;
    }
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    paragraph.line_count(width as u16).max(1) as u16
}

/// Count lines using word-wrap algorithm (matches TextArea's WrapMode::Word).
/// Words won't be broken mid-word, so this may produce more lines than character wrapping.
/// Returns (line_count, last_line_width) so caller can determine if cursor needs extra space.
pub(crate) fn word_wrap_line_count_with_last_width(text: &str, width: usize) -> (u16, usize) {
    if width == 0 || text.is_empty() {
        return (1, 0);
    }

    let mut line_count = 0u16;
    let mut current_line_width = 0usize;

    for line in text.lines() {
        if line.is_empty() {
            line_count += 1;
            current_line_width = 0;
            continue;
        }

        let mut line_started = false;

        for word in line.split_whitespace() {
            let word_width = unicode_width::UnicodeWidthStr::width(word);

            if !line_started {
                if word_width > width {
                    line_count += word_width.div_ceil(width) as u16;
                    current_line_width = word_width % width;
                    if current_line_width == 0 {
                        current_line_width = 0;
                        line_started = false;
                    } else {
                        line_started = true;
                    }
                } else {
                    current_line_width = word_width;
                    line_started = true;
                }
            } else {
                let needed = current_line_width + 1 + word_width;
                if needed > width {
                    line_count += 1;
                    if word_width > width {
                        line_count += word_width.div_ceil(width) as u16;
                        current_line_width = word_width % width;
                        if current_line_width == 0 {
                            line_started = false;
                        }
                    } else {
                        current_line_width = word_width;
                    }
                } else {
                    current_line_width = needed;
                }
            }
        }

        if line_started {
            line_count += 1;
        }
    }

    if line_count == 0 {
        line_count = 1;
        current_line_width = 0;
    }

    (line_count, current_line_width)
}

// ---------------------------------------------------------------------------
// Inline markdown formatting
// ---------------------------------------------------------------------------

/// Parse inline markdown formatting (**bold** and `code`) into styled spans.
/// Preserves all other text — list prefixes, indentation, and line structure
/// are left exactly as-is.
fn style_inline_markdown(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    let base_style = Style::from_crossterm(theme.as_style(Meaning::Base));
    let code_style = Style::from_crossterm(theme.as_style(Meaning::Guidance));
    let bold_style = base_style.add_modifier(Modifier::BOLD);

    text.lines()
        .map(|line| {
            Line::from(parse_inline_formatting(
                line, base_style, bold_style, code_style,
            ))
        })
        .collect()
}

/// Parse a single line for `code` and **bold** markers, returning styled spans.
fn parse_inline_formatting(
    line: &str,
    base: Style,
    bold: Style,
    code: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '`' {
            // Flush accumulated plain text
            if !current.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut current), base));
            }
            // Collect until closing backtick
            let mut code_text = String::new();
            let mut closed = false;
            for next in chars.by_ref() {
                if next == '`' {
                    closed = true;
                    break;
                }
                code_text.push(next);
            }
            if closed {
                spans.push(Span::styled(code_text, code));
            } else {
                // Unclosed backtick — render as-is
                current.push('`');
                current.push_str(&code_text);
            }
        } else if ch == '*' && chars.peek() == Some(&'*') {
            chars.next(); // consume second *
            // Flush accumulated plain text
            if !current.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut current), base));
            }
            // Collect until closing **
            let mut bold_text = String::new();
            let mut closed = false;
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'*') {
                    chars.next();
                    closed = true;
                    break;
                }
                bold_text.push(next);
            }
            if closed {
                spans.push(Span::styled(bold_text, bold));
            } else {
                // Unclosed ** — render as-is
                current.push_str("**");
                current.push_str(&bold_text);
            }
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() {
        spans.push(Span::styled(current, base));
    }

    spans
}

// ---------------------------------------------------------------------------
// Leaf components
// ---------------------------------------------------------------------------

/// User input display (active textarea or static text).
pub struct InputContent {
    pub text: String,
    pub active: bool,
}

impl Component for InputContent {
    fn height(&self, width: u16) -> u16 {
        let w = width as usize;
        if self.active {
            let (lines, last_width) = word_wrap_line_count_with_last_width(&self.text, w);
            if last_width >= w {
                lines.saturating_add(1)
            } else {
                lines
            }
        } else {
            line_count_wrapped(&self.text, w)
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        if self.active {
            if let Some(textarea) = ctx.textarea {
                frame.render_widget(textarea, area);
            }
        } else {
            let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
            frame.render_widget(
                Paragraph::new(self.text.as_str())
                    .style(style)
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

/// Command suggestion ($ prefix).
pub struct CommandContent {
    pub text: String,
    pub faded: bool,
}

impl Component for CommandContent {
    fn height(&self, width: u16) -> u16 {
        line_count_wrapped(&self.text, width as usize)
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        let mut style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
        if self.faded {
            style = style.add_modifier(Modifier::DIM);
        }
        frame.render_widget(
            Paragraph::new(self.text.as_str())
                .style(style)
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

/// Markdown text content (indented, no symbol).
pub struct TextContent {
    pub markdown: String,
}

impl Component for TextContent {
    fn height(&self, width: u16) -> u16 {
        // Height uses raw text — slightly overestimates since markdown syntax
        // characters (**, `) are stripped in rendering, but this is harmless
        // (allocates equal or more space than needed, never less).
        line_count_wrapped(&self.markdown, width as usize)
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        let lines = style_inline_markdown(&self.markdown, ctx.theme);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }
}

/// Error message (! prefix).
pub struct ErrorContent {
    pub message: String,
}

impl Component for ErrorContent {
    fn height(&self, width: u16) -> u16 {
        line_count_wrapped(&self.message, width as usize)
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
        frame.render_widget(
            Paragraph::new(self.message.as_str())
                .style(style)
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

/// Warning for dangerous or low-confidence commands.
pub struct WarningContent {
    pub kind: WarningKind,
    pub text: String,
    pub pending_confirm: bool,
}

impl Component for WarningContent {
    fn height(&self, width: u16) -> u16 {
        let display_text = if self.pending_confirm {
            "Press Enter again to run this dangerous command"
        } else {
            self.text.as_str()
        };
        line_count_wrapped(display_text, width as usize)
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
        let display_text = if self.pending_confirm {
            "Press Enter again to run this dangerous command"
        } else {
            self.text.as_str()
        };
        frame.render_widget(
            Paragraph::new(display_text)
                .style(style)
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

/// Animated spinner with status text.
pub struct SpinnerContent {
    pub status_text: String,
}

impl Component for SpinnerContent {
    fn height(&self, _width: u16) -> u16 {
        1
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));
        frame.render_widget(Paragraph::new(self.status_text.as_str()).style(style), area);
    }
}

/// Tool call progress (in-flight spinner or completed checkmark).
pub struct ToolStatusContent {
    pub completed_count: usize,
    pub current_label: Option<String>,
    pub frame: usize,
}

impl Component for ToolStatusContent {
    fn height(&self, _width: u16) -> u16 {
        1
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));
        let text = if let Some(ref label) = self.current_label {
            if self.completed_count > 0 {
                format!(
                    "{} (used {} tool{})",
                    label,
                    self.completed_count,
                    if self.completed_count == 1 { "" } else { "s" }
                )
            } else {
                label.clone()
            }
        } else {
            format!(
                "Used {} tool{}",
                self.completed_count,
                if self.completed_count == 1 { "" } else { "s" }
            )
        };
        frame.render_widget(Paragraph::new(text).style(style), area);
    }
}

// ---------------------------------------------------------------------------
// Factory functions
// ---------------------------------------------------------------------------

/// Convert a view model `Content` item into a `SymbolRow`-wrapped component.
fn content_to_component(content: &Content) -> Box<dyn Component> {
    match content {
        Content::Input { text, active, .. } => Box::new(SymbolRow {
            symbol: ">".to_string(),
            symbol_meaning: Meaning::Guidance,
            inner: Box::new(InputContent {
                text: text.clone(),
                active: *active,
            }),
        }),

        Content::Command { text, faded } => Box::new(SymbolRow {
            symbol: "$".to_string(),
            symbol_meaning: Meaning::Important,
            inner: Box::new(CommandContent {
                text: text.clone(),
                faded: *faded,
            }),
        }),

        Content::Text { markdown } => Box::new(SymbolRow {
            symbol: " ".to_string(),
            symbol_meaning: Meaning::Base,
            inner: Box::new(TextContent {
                markdown: markdown.clone(),
            }),
        }),

        Content::Error { message } => Box::new(SymbolRow {
            symbol: "!".to_string(),
            symbol_meaning: Meaning::AlertError,
            inner: Box::new(ErrorContent {
                message: message.clone(),
            }),
        }),

        Content::Warning {
            kind,
            text,
            pending_confirm,
        } => {
            let (symbol, meaning) = match kind {
                WarningKind::Danger => ("!", Meaning::AlertError),
                WarningKind::LowConfidence => ("?", Meaning::AlertWarn),
            };
            Box::new(SymbolRow {
                symbol: symbol.to_string(),
                symbol_meaning: meaning,
                inner: Box::new(WarningContent {
                    kind: *kind,
                    text: text.clone(),
                    pending_confirm: *pending_confirm,
                }),
            })
        }

        Content::Spinner { frame, status_text } => Box::new(SymbolRow {
            symbol: active_frame(*frame).to_string(),
            symbol_meaning: Meaning::Annotation,
            inner: Box::new(SpinnerContent {
                status_text: status_text.clone(),
            }),
        }),

        Content::ToolStatus {
            completed_count,
            current_label,
            frame,
        } => {
            let symbol = if current_label.is_some() {
                active_frame(*frame).to_string()
            } else {
                "\u{2713}".to_string() // ✓
            };
            Box::new(SymbolRow {
                symbol,
                symbol_meaning: Meaning::Annotation,
                inner: Box::new(ToolStatusContent {
                    completed_count: *completed_count,
                    current_label: current_label.clone(),
                    frame: *frame,
                }),
            })
        }
    }
}

/// Convert a view model `Block` into a `VStack` of content components.
fn build_block_component(block: &Block) -> Box<dyn Component> {
    let mut children: Vec<Box<dyn Component>> = Vec::new();

    for (idx, content) in block.content.iter().enumerate() {
        if idx > 0 {
            children.push(Box::new(Spacer(1))); // blank line between items
        }
        children.push(content_to_component(content));
    }

    // Trailing blank line (padding after content)
    children.push(Box::new(Spacer(1)));

    Box::new(VStack::new(children))
}

/// Build the full component tree from an ordered list of view model blocks.
///
/// The tree is a `VStack` with blocks separated by `Separator` + `Spacer` pairs.
/// The caller sets `scroll_offset` on the returned `VStack` before rendering.
pub fn build_component_tree(items: &[&Block], card_width: u16) -> VStack {
    let mut children: Vec<Box<dyn Component>> = Vec::new();

    for (idx, block) in items.iter().enumerate() {
        if idx > 0 {
            children.push(Box::new(Separator { card_width }));
            children.push(Box::new(Spacer(1))); // leading blank after separator
        }
        children.push(build_block_component(block));
    }

    VStack::new(children)
}
