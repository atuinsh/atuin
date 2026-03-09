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
use tui_textarea::TextArea;

use super::spinner::active_frame;
use super::state::AppState;
use super::view_model::{Blocks, Content, WarningKind};

/// Fixed card width for the TUI
pub(crate) const CARD_WIDTH: u16 = 64;

pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub anchor_col: u16,
    pub textarea: Option<&'a TextArea<'static>>,
    /// Maximum viewport height (for scroll calculations)
    pub max_height: u16,
    /// When true, the viewport is a fixed rect already positioned for the card.
    /// The card fills the entire viewport instead of positioning via anchor_col.
    pub popup_mode: bool,
    /// When true, blocks are rendered in reverse order so that the input field
    /// appears at the bottom of the card (close to the prompt when the popup
    /// is above the cursor).
    pub render_above: bool,
}

/// Calculate the height needed to render the current state.
/// Used to dynamically resize the viewport before rendering.
/// `card_width` is the outer card width (including borders); pass 0 to use CARD_WIDTH default.
pub fn calculate_needed_height(state: &AppState, card_width: u16) -> u16 {
    let view = Blocks::from_state(state);
    let w = if card_width > 0 {
        card_width
    } else {
        CARD_WIDTH
    };
    let content_width = usize::from(w.saturating_sub(4)).max(1);

    let mut total_height = 0u16;
    for (idx, block) in view.items.iter().enumerate() {
        if idx > 0 {
            total_height = total_height.saturating_add(1); // separator
            total_height = total_height.saturating_add(1); // leading blank after separator
        }
        total_height =
            total_height.saturating_add(calculate_block_height(&block.content, content_width));
    }

    // Add borders (2) + top padding (1), minimum 5
    total_height.saturating_add(3).max(5)
}

/// Main render function: derives view model from state, then renders it
pub fn render(frame: &mut Frame, state: &AppState, ctx: &RenderContext) {
    // PURE DERIVATION: view model is always rebuilt from state
    let view = Blocks::from_state(state);

    // Render the derived view model
    render_view(frame, &view, ctx);
}

fn render_view(frame: &mut Frame, view: &Blocks, ctx: &RenderContext) {
    let full_area = frame.area();

    // In popup mode, the viewport is already positioned and sized for the card.
    // Clear it to prevent background bleed-through, then inset by margin for the card.
    let (area, card_x, desired_width) = if ctx.popup_mode {
        #[cfg(unix)]
        use super::popup::POPUP_MARGIN;
        #[cfg(not(unix))]
        const POPUP_MARGIN: u16 = 0;
        frame.render_widget(ratatui::widgets::Clear, full_area);
        let inset = full_area.inner(ratatui::layout::Margin {
            horizontal: POPUP_MARGIN,
            vertical: POPUP_MARGIN,
        });
        (inset, inset.x, inset.width)
    } else {
        let dw = CARD_WIDTH.min(full_area.width.saturating_sub(2)).max(32);
        let max_x = full_area.x + full_area.width.saturating_sub(dw);
        let preferred_x = full_area.x + ctx.anchor_col.saturating_sub(2);
        (full_area, preferred_x.min(max_x), dw)
    };
    let content_width = usize::from(desired_width.saturating_sub(4)).max(1);

    // Build ordered items list — the active content (input/LLM response)
    // should always be closest to the cursor/prompt:
    //   - Popup below cursor (render_above=false): reverse so active is at top
    //   - Popup above cursor (render_above=true): normal order, active is at bottom
    //   - Inline mode: normal order (no reversal)
    let items: Vec<&super::view_model::Block> = if ctx.popup_mode && !ctx.render_above {
        view.items.iter().rev().collect()
    } else {
        view.items.iter().collect()
    };

    // Calculate height from view model
    let mut total_height = 0u16;
    for (idx, block) in items.iter().enumerate() {
        if idx > 0 {
            total_height = total_height.saturating_add(1); // separator
            total_height = total_height.saturating_add(1); // leading blank after separator
        }
        total_height =
            total_height.saturating_add(calculate_block_height(&block.content, content_width));
    }

    let desired_height = total_height
        .saturating_add(3) // borders (2) + top padding (1), no bottom padding
        .max(5);

    // Cap card height at viewport height to prevent overflow
    let actual_height = desired_height.min(area.height);

    // Calculate scroll offset to keep the active content visible when overflowing.
    // When render_above=false (popup below cursor), items are reversed so the active
    // content (input/spinner) is at the top — scroll_offset stays 0 to show the top.
    // Otherwise, scroll to show the bottom where the active content lives.
    let scroll_offset = if ctx.popup_mode && !ctx.render_above {
        0
    } else {
        desired_height.saturating_sub(actual_height)
    };

    let card = Rect {
        x: card_x,
        y: area.y,
        width: desired_width,
        height: actual_height,
    };

    // Get title from first block in ORIGINAL order (always the input block)
    let title = view
        .items
        .first()
        .and_then(|b| b.title.as_deref())
        .unwrap_or("Describe the command you'd like to generate:");

    // Create bordered frame
    // Padding: left=1, right=1, top=1, bottom=0 (blocks have trailing blanks)
    let mut outer_block = RatatuiBlock::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(Line::from(view.footer).alignment(Alignment::Right))
        .padding(Padding::new(1, 1, 1, 0));

    // Status bar: transient status on the bottom border, left-aligned
    if let Some(ref sb) = view.status_bar {
        let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));
        let spinner = active_frame(sb.frame);
        let status_text = format!(" {} {} ", spinner, sb.text);
        outer_block = outer_block
            .title_bottom(Line::from(Span::styled(status_text, style)).alignment(Alignment::Left));
    }

    let inner_area = outer_block.inner(card);
    frame.render_widget(outer_block, card);

    // Render blocks (with scroll offset for overflowing content)
    render_blocks_content(frame, &items, ctx, inner_area, card.width, scroll_offset);
}

fn render_blocks_content(
    frame: &mut Frame,
    items: &[&super::view_model::Block],
    ctx: &RenderContext,
    area: Rect,
    card_width: u16,
    scroll_offset: u16,
) {
    let content_width = usize::from(area.width).max(1);

    // Build layout constraints for full content
    let mut constraints = Vec::new();
    let mut block_heights = Vec::new();
    for (idx, block) in items.iter().enumerate() {
        if idx > 0 {
            constraints.push(Constraint::Length(1)); // separator
            constraints.push(Constraint::Length(1)); // leading blank after separator
            block_heights.push(1);
            block_heights.push(1);
        }
        let height = calculate_block_height(&block.content, content_width);
        constraints.push(Constraint::Length(height));
        block_heights.push(height);
    }

    if constraints.is_empty() {
        return;
    }

    // Calculate cumulative heights to find which blocks are visible after scrolling
    let mut cumulative: Vec<u16> = Vec::with_capacity(block_heights.len() + 1);
    cumulative.push(0);
    for h in &block_heights {
        cumulative.push(cumulative.last().unwrap() + h);
    }

    // Render each chunk, offsetting by scroll_offset and clipping to visible area
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut chunk_idx = 0;
    for (idx, block) in items.iter().enumerate() {
        if idx > 0 {
            // Check if separator is visible (its position minus scroll_offset)
            let sep_start = cumulative[chunk_idx];
            if sep_start >= scroll_offset && sep_start < scroll_offset + area.height {
                let adjusted_chunk = Rect {
                    y: area.y + sep_start - scroll_offset,
                    ..chunks[chunk_idx]
                };
                render_separator(frame, adjusted_chunk, ctx, card_width);
            }
            chunk_idx += 1;
            chunk_idx += 1; // skip leading blank
        }

        // Check if this block is at least partially visible
        let block_start = cumulative[chunk_idx];
        let block_end = cumulative[chunk_idx + 1];

        // Block is visible if it starts before viewport end and ends after viewport start
        if block_start < scroll_offset + area.height && block_end > scroll_offset {
            // Calculate visible portion
            let visible_start = block_start.max(scroll_offset);
            let visible_end = block_end.min(scroll_offset + area.height);

            let adjusted_chunk = Rect {
                x: area.x,
                y: area.y + visible_start - scroll_offset,
                width: area.width,
                height: visible_end - visible_start,
            };

            render_block_content(frame, &block.content, adjusted_chunk, ctx);
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
        Content::Input { text, active, .. } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Guidance));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            // Render ">" symbol at column 0
            render_symbol(frame, ">", symbol_style, area);

            if *active {
                // Active input: render TextArea widget (handles cursor display)
                if let Some(textarea) = ctx.textarea {
                    frame.render_widget(textarea, text_area);
                }
            } else {
                // Inactive input: render as plain paragraph
                let paragraph = Paragraph::new(text.as_str())
                    .style(text_style)
                    .wrap(Wrap { trim: false });
                frame.render_widget(paragraph, text_area);
            }
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
            let symbol = active_frame(*spinner_frame);

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
                let spinner = active_frame(*spinner_frame);
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
        // Input uses word wrapping (WrapMode::Word) in TextArea, which can produce
        // more lines than character wrapping since it won't break words mid-word
        Content::Input { text, active, .. } => {
            if *active {
                // For active input, use word-wrap line counting to match TextArea behavior
                let (lines, last_line_width) =
                    word_wrap_line_count_with_last_width(text, text_width);
                // Only add extra line for cursor if the last line is full
                if last_line_width >= text_width {
                    lines.saturating_add(1)
                } else {
                    lines
                }
            } else {
                line_count_wrapped(text, text_width)
            }
        }
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
/// Uses ratatui's Paragraph::line_count for accurate wrapping calculation.
fn line_count_wrapped(text: &str, width: usize) -> u16 {
    if width == 0 {
        return 1;
    }

    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    paragraph.line_count(width as u16).max(1) as u16
}

/// Count lines using word-wrap algorithm (matches TextArea's WrapMode::Word).
/// Words won't be broken mid-word, so this may produce more lines than character wrapping.
/// Returns (line_count, last_line_width) so caller can determine if cursor needs extra space.
fn word_wrap_line_count_with_last_width(text: &str, width: usize) -> (u16, usize) {
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
                // First word on line
                if word_width > width {
                    // Word is longer than width, it will be split by character
                    // Count how many lines it takes
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
                // Subsequent word - need space before it
                let needed = current_line_width + 1 + word_width;
                if needed > width {
                    // Word doesn't fit, start new line
                    line_count += 1;
                    if word_width > width {
                        // Word itself is too long, will be split
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

        // Count the last line of this logical line
        if line_started {
            line_count += 1;
        }
    }

    // Handle case where text has no lines() output (empty or just whitespace)
    if line_count == 0 {
        line_count = 1;
        current_line_width = 0;
    }

    (line_count, current_line_width)
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
