use atuin_client::theme::{Meaning, Theme};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    backend::FromCrossterm,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as RatatuiBlock, Borders, Padding, Paragraph},
};

use super::state::AppState;
use super::view_model::{Blocks, Content, WarningKind};

const SPINNER_FRAMES: [&str; 4] = ["/", "-", "\\", "|"];

pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub anchor_col: u16,
}

/// Main render function: derives view model from state, then renders it
pub fn render(frame: &mut Frame, state: &AppState, ctx: &RenderContext) {
    // PURE DERIVATION: view model is always rebuilt from state
    let view = Blocks::from_state(state);

    // Render the derived view model
    render_view(frame, &view, ctx);
}

fn render_view(frame: &mut Frame, view: &Blocks, ctx: &RenderContext) {
    let area = frame.area();

    // Calculate frame dimensions (64 chars wide max, min 32)
    let desired_width = 64u16.min(area.width.saturating_sub(2)).max(32);
    let content_width = usize::from(desired_width.saturating_sub(4)).max(1);

    // Position at anchor_col
    let max_x = area.x + area.width.saturating_sub(desired_width);
    let preferred_x = area.x + ctx.anchor_col.saturating_sub(2);

    // Calculate height from view model
    let mut total_height = 0u16;
    for (idx, block) in view.items.iter().enumerate() {
        if idx > 0 {
            total_height = total_height.saturating_add(1); // separator
        }
        total_height =
            total_height.saturating_add(calculate_block_height(&block.content, content_width));
    }

    let desired_height = total_height
        .saturating_add(3) // borders (2) + top padding (1), no bottom padding
        .min(area.height.max(1))
        .max(5);

    let card = Rect {
        x: preferred_x.min(max_x),
        y: area.y,
        width: desired_width,
        height: desired_height,
    };

    // Get title from first block (if any)
    let title = view
        .items
        .first()
        .and_then(|b| b.title.as_deref())
        .unwrap_or("Describe the command you'd like to generate:");

    // Create bordered frame
    // Padding: left=1, right=1, top=1, bottom=0 (blocks have trailing blanks)
    let outer_block = RatatuiBlock::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(Line::from(view.footer).alignment(Alignment::Right))
        .padding(Padding::new(1, 1, 1, 0));

    let inner_area = outer_block.inner(card);
    frame.render_widget(outer_block, card);

    // Render blocks
    render_blocks_content(frame, view, ctx, inner_area, card.width);
}

fn render_blocks_content(
    frame: &mut Frame,
    view: &Blocks,
    ctx: &RenderContext,
    area: Rect,
    card_width: u16,
) {
    let content_width = usize::from(area.width).max(1);

    // Build layout constraints
    let mut constraints = Vec::new();
    for (idx, block) in view.items.iter().enumerate() {
        if idx > 0 {
            constraints.push(Constraint::Length(1)); // separator
        }
        let height = calculate_block_height(&block.content, content_width);
        constraints.push(Constraint::Length(height));
    }

    if constraints.is_empty() {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut chunk_idx = 0;
    for (idx, block) in view.items.iter().enumerate() {
        if idx > 0 {
            render_separator(frame, chunks[chunk_idx], ctx, card_width);
            chunk_idx += 1;
        }

        render_block_content(frame, &block.content, chunks[chunk_idx], ctx);

        // Set cursor if any content item is active input
        for content in &block.content {
            if let Content::Input {
                text,
                active: true,
                cursor_pos,
            } = content
            {
                let (cursor_row, cursor_col) =
                    calculate_cursor_position(text, *cursor_pos, content_width);
                let cursor_x = chunks[chunk_idx].x.saturating_add(cursor_col);
                let cursor_y = chunks[chunk_idx].y.saturating_add(cursor_row);
                frame.set_cursor_position((cursor_x, cursor_y));
            }
        }

        chunk_idx += 1;
    }
}

/// Render all content items in a block
fn render_block_content(frame: &mut Frame, content: &[Content], area: Rect, ctx: &RenderContext) {
    if content.is_empty() {
        return;
    }

    let content_width = usize::from(area.width).max(1);

    // Build layout constraints for each content item WITH spacing between items
    let mut constraints = Vec::new();
    for (idx, c) in content.iter().enumerate() {
        if idx > 0 {
            constraints.push(Constraint::Length(1)); // blank line between items
        }
        constraints.push(Constraint::Length(calculate_single_content_height(
            c,
            content_width,
        )));
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut chunk_idx = 0;
    for (idx, item) in content.iter().enumerate() {
        if idx > 0 {
            chunk_idx += 1; // skip the blank line chunk
        }
        render_single_content(frame, item, chunks[chunk_idx], ctx);
        chunk_idx += 1;
    }
}

/// Render a single content item
fn render_single_content(frame: &mut Frame, content: &Content, area: Rect, ctx: &RenderContext) {
    let width = area.width as usize;

    match content {
        Content::Input { text, .. } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Guidance));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            let prefix = vec![Span::styled("> ", symbol_style)];
            let lines = wrap_with_indent(prefix, text, text_style, width, "  ");

            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
        }

        Content::Command { text, faded } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Important));
            let mut text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
            if *faded {
                text_style = text_style.add_modifier(Modifier::DIM);
            }

            let prefix = vec![Span::styled("$ ", symbol_style)];
            let lines = wrap_with_indent(prefix, text, text_style, width, "  ");

            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
        }

        Content::Text { markdown } => {
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            // For markdown text, use simple indent (no symbol)
            let prefix = vec![Span::raw("  ")];
            let lines = wrap_with_indent(prefix, markdown, text_style, width, "  ");

            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
        }

        Content::Error { message } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::AlertError));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            let prefix = vec![Span::styled("! ", symbol_style)];
            let lines = wrap_with_indent(prefix, message, text_style, width, "  ");

            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
        }

        Content::Warning {
            kind,
            text,
            pending_confirm,
        } => {
            let (symbol, meaning) = match kind {
                WarningKind::Danger => ("!", Meaning::AlertError),
                WarningKind::LowConfidence => ("?", Meaning::AlertWarn),
            };
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(meaning));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            let display_text = if *pending_confirm {
                "Press Enter again to run this dangerous command"
            } else {
                text.as_str()
            };

            let prefix = vec![Span::styled(format!("{} ", symbol), symbol_style)];
            let lines = wrap_with_indent(prefix, display_text, text_style, width, "  ");

            let paragraph = Paragraph::new(lines);
            frame.render_widget(paragraph, area);
        }

        Content::Spinner {
            frame: spinner_frame,
            status_text,
        } => {
            let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));
            let symbol = SPINNER_FRAMES[*spinner_frame % SPINNER_FRAMES.len()];
            let text = format!("{} {}", symbol, status_text);

            let paragraph = Paragraph::new(Span::styled(text, style));
            frame.render_widget(paragraph, area);
        }

        Content::ToolStatus {
            completed_count,
            current_label,
            frame: spinner_frame,
        } => {
            let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));

            let text = if let Some(label) = current_label {
                // In-flight: show spinner + current tool
                let spinner = SPINNER_FRAMES[*spinner_frame % SPINNER_FRAMES.len()];
                if *completed_count > 0 {
                    format!(
                        "{} {} (used {} tool{})",
                        spinner,
                        label,
                        completed_count,
                        if *completed_count == 1 { "" } else { "s" }
                    )
                } else {
                    format!("{} {}", spinner, label)
                }
            } else {
                // Completed: show checkmark + summary
                format!(
                    "\u{2713} Used {} tool{}",
                    completed_count,
                    if *completed_count == 1 { "" } else { "s" }
                )
            };

            let paragraph = Paragraph::new(Span::styled(text, style));
            frame.render_widget(paragraph, area);
        }
    }
}

fn render_separator(frame: &mut Frame, area: Rect, ctx: &RenderContext, card_width: u16) {
    let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Muted));

    // Build separator: ├ + ─ repeated + ┤ spanning the full card width
    // -2 for the ├ and ┤ characters themselves
    let inner_width = card_width.saturating_sub(2) as usize;
    let separator = format!(
        "\u{251c}{}\u{2524}",           // ├ ... ┤
        "\u{2500}".repeat(inner_width)  // ─
    );

    let paragraph = Paragraph::new(Span::styled(separator, style));

    // Render at x offset to overlap the border (area is inside padding, border is 2 chars left)
    let sep_area = Rect {
        x: area.x.saturating_sub(2), // move left to overlap left border
        y: area.y,
        width: card_width,
        height: 1,
    };
    frame.render_widget(paragraph, sep_area);
}

/// Calculate total height for all content items in a block
fn calculate_block_height(content: &[Content], width: usize) -> u16 {
    let content_height: u16 = content
        .iter()
        .map(|c| calculate_single_content_height(c, width))
        .sum();

    // Add spacing between items (n-1 blank lines for n items)
    let spacing = if content.len() > 1 {
        (content.len() - 1) as u16
    } else {
        0
    };

    // Add 1 for trailing blank line (padding after content)
    content_height.saturating_add(spacing).saturating_add(1)
}

/// Calculate height for a single content item
fn calculate_single_content_height(content: &Content, width: usize) -> u16 {
    // Use the same wrapping logic as render to ensure consistency
    match content {
        Content::Input { text, .. } => {
            wrapped_line_count_with_indent(text, width, 2) as u16 // "> " prefix
        }
        Content::Command { text, .. } => {
            wrapped_line_count_with_indent(text, width, 2) as u16 // "$ " prefix
        }
        Content::Text { markdown } => {
            wrapped_line_count_with_indent(markdown, width, 2) as u16 // "  " indent
        }
        Content::Error { message } => {
            wrapped_line_count_with_indent(message, width, 2) as u16 // "! " prefix
        }
        Content::Warning {
            text,
            pending_confirm,
            ..
        } => {
            let display_text = if *pending_confirm {
                "Press Enter again to run this dangerous command"
            } else {
                text.as_str()
            };
            wrapped_line_count_with_indent(display_text, width, 2) as u16 // "! " or "? " prefix
        }
        Content::Spinner { .. } => 1,
        Content::ToolStatus { .. } => 1, // Always single line
    }
}

/// Count lines needed when wrapping with a prefix/indent
fn wrapped_line_count_with_indent(text: &str, width: usize, indent_len: usize) -> usize {
    if width == 0 {
        return 1;
    }

    let available_first = width.saturating_sub(indent_len);
    let available_rest = width.saturating_sub(indent_len);

    let mut total_lines = 0;

    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            total_lines += 1;
            continue;
        }

        let char_count = paragraph.chars().count();

        // First line
        let first_line_chars = char_count.min(available_first);
        total_lines += 1;

        // Remaining chars for continuation lines
        let remaining = char_count.saturating_sub(first_line_chars);
        if remaining > 0 && available_rest > 0 {
            total_lines += remaining.div_ceil(available_rest);
        }
    }

    total_lines.max(1)
}

/// Wrap text with a prefix on the first line and indent on continuation lines.
/// Returns a Vec of Lines ready for rendering.
fn wrap_with_indent<'a>(
    prefix_spans: Vec<Span<'a>>,
    text: &str,
    text_style: Style,
    width: usize,
    indent: &'static str,
) -> Vec<Line<'a>> {
    if width == 0 {
        let mut spans = prefix_spans;
        spans.push(Span::styled(text.to_string(), text_style));
        return vec![Line::from(spans)];
    }

    let prefix_len: usize = prefix_spans.iter().map(|s| s.content.chars().count()).sum();
    let indent_len = indent.chars().count();

    let mut lines = Vec::new();
    let mut first_line = true;

    for paragraph in text.split('\n') {
        let mut remaining = paragraph;

        while !remaining.is_empty() {
            let available = if first_line {
                width.saturating_sub(prefix_len)
            } else {
                width.saturating_sub(indent_len)
            };

            if available == 0 {
                // Can't fit anything, just take one char
                let (chunk, rest) =
                    remaining.split_at(remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(0));
                if first_line {
                    let mut spans = prefix_spans.clone();
                    spans.push(Span::styled(chunk.to_string(), text_style));
                    lines.push(Line::from(spans));
                    first_line = false;
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(indent),
                        Span::styled(chunk.to_string(), text_style),
                    ]));
                }
                remaining = rest;
                continue;
            }

            // Find break point (word boundary or max chars)
            let char_indices: Vec<(usize, char)> = remaining.char_indices().collect();
            let max_chars = available.min(char_indices.len());

            // Try to break at word boundary
            let break_at = if char_indices.len() <= available {
                // Whole remaining string fits
                remaining.len()
            } else {
                // Find last space before max_chars, otherwise break at max_chars
                let max_byte = char_indices
                    .get(max_chars)
                    .map(|(i, _)| *i)
                    .unwrap_or(remaining.len());
                let mut break_byte = max_byte;
                for i in (0..max_chars).rev() {
                    if char_indices[i].1 == ' ' {
                        break_byte = char_indices[i].0;
                        break;
                    }
                }
                break_byte
            };

            let (chunk, rest) = remaining.split_at(break_at);
            let chunk = chunk.trim_end();
            let rest = rest.trim_start();

            if first_line {
                let mut spans = prefix_spans.clone();
                if !chunk.is_empty() {
                    spans.push(Span::styled(chunk.to_string(), text_style));
                }
                lines.push(Line::from(spans));
                first_line = false;
            } else {
                lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(chunk.to_string(), text_style),
                ]));
            }

            remaining = rest;
            if remaining.is_empty() {
                break;
            }
        }

        // Handle empty paragraphs (newlines in source)
        if paragraph.is_empty() {
            if first_line {
                lines.push(Line::from(prefix_spans.clone()));
                first_line = false;
            } else {
                lines.push(Line::from(vec![Span::raw(indent)]));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(prefix_spans));
    }

    lines
}

/// Calculate cursor position accounting for prefix and wrapping
fn calculate_cursor_position(_input: &str, cursor_pos: usize, width: usize) -> (u16, u16) {
    if width == 0 {
        return (0, 0);
    }

    // The visible prompt line is `> {input}`
    // cursor_pos is character position in input
    // Need to account for "> " prefix (2 chars)

    let prefix_len = 2;
    let total_pos = prefix_len + cursor_pos;

    // Simple calculation: row = total_pos / width, col = total_pos % width
    // This is approximate - for perfect word-wrap cursor tracking,
    // we'd need to match ratatui's exact wrap algorithm
    let row = total_pos / width;
    let col = total_pos % width;

    (row as u16, col as u16)
}

/// Convert markdown to styled spans (existing function, kept as-is)
pub fn markdown_to_spans<'a>(text: &'a str, theme: &'a Theme) -> Vec<Line<'a>> {
    let parser = Parser::new(text);
    let mut lines: Vec<Vec<Span<'a>>> = vec![Vec::new()];
    let mut current_line = 0;

    let base_style = Style::from_crossterm(theme.as_style(Meaning::Base));
    let code_style = Style::from_crossterm(theme.as_style(Meaning::Important));
    let mut style_stack: Vec<Style> = vec![base_style];
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(Tag::Strong) => {
                let bold_style = style_stack
                    .last()
                    .copied()
                    .unwrap_or(base_style)
                    .add_modifier(Modifier::BOLD);
                style_stack.push(bold_style);
            }
            Event::End(TagEnd::Strong) => {
                style_stack.pop();
            }
            Event::Start(Tag::Emphasis) => {
                let underline_style = style_stack
                    .last()
                    .copied()
                    .unwrap_or(base_style)
                    .add_modifier(Modifier::UNDERLINED);
                style_stack.push(underline_style);
            }
            Event::End(TagEnd::Emphasis) => {
                style_stack.pop();
            }
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                // Start new line for code block
                if !lines[current_line].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                // Ensure blank line after code block
                if !lines[current_line].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::Code(code) => {
                lines[current_line].push(Span::styled(format!("`{}`", code), code_style));
            }
            Event::Text(text) => {
                let current_style = if in_code_block {
                    // Use Important style for code block content
                    code_style
                } else {
                    style_stack.last().copied().unwrap_or(base_style)
                };
                let parts: Vec<&str> = text.split('\n').collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        current_line += 1;
                        lines.push(Vec::new());
                    }
                    if !part.is_empty() {
                        lines[current_line].push(Span::styled(part.to_string(), current_style));
                    }
                }
            }
            Event::SoftBreak => {
                let current_style = style_stack.last().copied().unwrap_or(base_style);
                lines[current_line].push(Span::styled(" ", current_style));
            }
            Event::HardBreak => {
                current_line += 1;
                lines.push(Vec::new());
            }
            Event::Start(Tag::Paragraph) => {
                if current_line > 0 || !lines[0].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::End(TagEnd::Paragraph) => {}
            _ => {}
        }
    }

    lines.into_iter().map(Line::from).collect()
}
