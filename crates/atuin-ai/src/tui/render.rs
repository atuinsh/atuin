use atuin_client::theme::{Meaning, Theme};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    backend::FromCrossterm,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as RatatuiBlock, Borders, Padding, Paragraph, Wrap},
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

/// Render a single content item using ratatui's native wrapping.
/// Symbol is rendered at column 0, text wraps in columns 2+ (offset area).
fn render_single_content(frame: &mut Frame, content: &Content, area: Rect, ctx: &RenderContext) {
    // Helper to create offset text area (2 chars for symbol column)
    let text_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };

    match content {
        Content::Input { text, .. } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Guidance));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            // Render ">" symbol at column 0
            render_symbol(frame, ">", symbol_style, area);

            // Render text in offset area with native wrapping
            let paragraph = Paragraph::new(text.as_str())
                .style(text_style)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, text_area);
        }

        Content::Command { text, faded } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Important));
            let mut text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
            if *faded {
                text_style = text_style.add_modifier(Modifier::DIM);
            }

            render_symbol(frame, "$", symbol_style, area);

            let paragraph = Paragraph::new(text.as_str())
                .style(text_style)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, text_area);
        }

        Content::Text { markdown } => {
            // No symbol, just indent - render directly in offset area
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            let paragraph = Paragraph::new(markdown.as_str())
                .style(text_style)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, text_area);
        }

        Content::Error { message } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::AlertError));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            render_symbol(frame, "!", symbol_style, area);

            let paragraph = Paragraph::new(message.as_str())
                .style(text_style)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, text_area);
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

            render_symbol(frame, symbol, symbol_style, area);

            let paragraph = Paragraph::new(display_text)
                .style(text_style)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, text_area);
        }

        Content::Spinner {
            frame: spinner_frame,
            status_text,
        } => {
            let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));
            let symbol = SPINNER_FRAMES[*spinner_frame % SPINNER_FRAMES.len()];

            render_symbol(frame, symbol, style, area);

            let paragraph = Paragraph::new(status_text.as_str()).style(style);
            frame.render_widget(paragraph, text_area);
        }

        Content::ToolStatus {
            completed_count,
            current_label,
            frame: spinner_frame,
        } => {
            let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));

            let (symbol, text) = if let Some(label) = current_label {
                let spinner = SPINNER_FRAMES[*spinner_frame % SPINNER_FRAMES.len()];
                let text = if *completed_count > 0 {
                    format!(
                        "{} (used {} tool{})",
                        label,
                        completed_count,
                        if *completed_count == 1 { "" } else { "s" }
                    )
                } else {
                    label.clone()
                };
                (spinner, text)
            } else {
                (
                    "\u{2713}",
                    format!(
                        "Used {} tool{}",
                        completed_count,
                        if *completed_count == 1 { "" } else { "s" }
                    ),
                )
            };

            render_symbol(frame, symbol, style, area);

            let paragraph = Paragraph::new(text).style(style);
            frame.render_widget(paragraph, text_area);
        }
    }
}

/// Render a single-character symbol at the start of an area
fn render_symbol(frame: &mut Frame, symbol: &str, style: Style, area: Rect) {
    let symbol_area = Rect {
        x: area.x,
        y: area.y,
        width: 1,
        height: 1,
    };
    frame.render_widget(Paragraph::new(symbol).style(style), symbol_area);
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

/// Calculate height for a single content item.
/// Uses ratatui's Paragraph::line_count for consistency with rendering.
fn calculate_single_content_height(content: &Content, width: usize) -> u16 {
    // Text area is offset by 2 for symbol column
    let text_width = width.saturating_sub(2);

    match content {
        Content::Input { text, .. } => line_count_wrapped(text, text_width),
        Content::Command { text, .. } => line_count_wrapped(text, text_width),
        Content::Text { markdown } => line_count_wrapped(markdown, text_width),
        Content::Error { message } => line_count_wrapped(message, text_width),
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
            line_count_wrapped(display_text, text_width)
        }
        Content::Spinner { .. } => 1,
        Content::ToolStatus { .. } => 1,
    }
}

/// Count lines when text is wrapped at given width.
/// Simple character-based calculation that approximates ratatui's wrapping.
fn line_count_wrapped(text: &str, width: usize) -> u16 {
    if width == 0 {
        return 1;
    }

    let mut total_lines = 0u16;

    for line in text.split('\n') {
        if line.is_empty() {
            total_lines += 1;
        } else {
            // Ceiling division: how many lines needed for this text at this width
            let char_count = line.chars().count();
            total_lines += char_count.div_ceil(width).max(1) as u16;
        }
    }

    total_lines.max(1)
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
