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

use crate::tui::{App, AppMode, BlockKind, BlockState, LegacyBlock as Block};

const SPINNER_FRAMES: [&str; 4] = ["/", "-", "\\", "|"];

pub struct RenderContext<'a> {
    pub theme: &'a Theme,
    pub anchor_col: u16,
}

pub fn render_blocks(frame: &mut Frame, app: &App, ctx: &RenderContext) {
    let area = frame.area();

    // Calculate frame dimensions (64 chars wide max, min 32, accounting for borders)
    let desired_width = 64u16.min(area.width.saturating_sub(2)).max(32);

    // Position the frame at anchor_col
    let max_x = area.x + area.width.saturating_sub(desired_width);
    let preferred_x = area.x + ctx.anchor_col.saturating_sub(2);

    // Calculate height dynamically based on content
    let content_width = usize::from(desired_width.saturating_sub(4)).max(1); // 2 for borders, 2 for padding
    let mut total_height = 0u16;

    // Calculate heights for all blocks
    for block in &app.blocks() {
        let block_height = calculate_block_height(block, content_width);
        total_height = total_height.saturating_add(block_height);
    }

    // Add separator heights (between blocks)
    if app.blocks().len() > 1 {
        total_height = total_height.saturating_add((app.blocks().len() - 1) as u16);
    }

    // Account for current input in Input mode
    if *app.mode() == AppMode::Input {
        // Add separator before current input if there are blocks
        if !app.blocks().is_empty() {
            total_height = total_height.saturating_add(1);
        }
        let input_height = calculate_input_height(&app.input(), content_width);
        total_height = total_height.saturating_add(input_height);
    }

    // Add padding (2 for top/bottom) and borders (2 for top/bottom)
    let desired_height = total_height
        .saturating_add(4) // 2 for padding, 2 for borders
        .min(area.height.max(1))
        .max(5); // Minimum height for title + footer + some content

    let card = Rect {
        x: preferred_x.min(max_x),
        y: area.y,
        width: desired_width,
        height: desired_height,
    };

    // Get keybinds footer based on mode
    let footer = get_footer_text(&*app.mode());

    // Create the outer bordered frame
    let outer_block = RatatuiBlock::default()
        .borders(Borders::ALL)
        .title("Describe the command you'd like to generate:")
        .title_bottom(Line::from(footer).alignment(Alignment::Right))
        .padding(Padding::uniform(1));

    let inner_area = outer_block.inner(card);
    frame.render_widget(outer_block, card);

    // Render blocks inside the frame
    render_block_content(frame, app, ctx, inner_area);
}

fn render_block_content(frame: &mut Frame, app: &App, ctx: &RenderContext, area: Rect) {
    let content_width = usize::from(area.width).max(1);

    // Build constraints for layout
    let mut constraints = Vec::new();
    let mut block_count = 0;

    for (idx, block) in app.blocks().iter().enumerate() {
        // Add separator constraint if not first block
        if idx > 0 {
            constraints.push(Constraint::Length(1));
        }

        let height = calculate_block_height(block, content_width);
        constraints.push(Constraint::Length(height));
        block_count += 1;
    }

    // Add current input in Input mode
    if *app.mode() == AppMode::Input {
        if block_count > 0 {
            constraints.push(Constraint::Length(1)); // Separator
        }
        let input_height = calculate_input_height(&app.input(), content_width);
        constraints.push(Constraint::Length(input_height));
    }

    // If no constraints, nothing to render
    if constraints.is_empty() {
        return;
    }

    // Create layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // Render each block
    let mut chunk_idx = 0;
    for (idx, block) in app.blocks().iter().enumerate() {
        // Render separator if not first block
        if idx > 0 {
            render_separator(frame, chunks[chunk_idx], ctx);
            chunk_idx += 1;
        }

        // Render the block
        render_single_block(frame, block, chunks[chunk_idx], ctx);
        chunk_idx += 1;
    }

    // Render current input in Input mode
    if *app.mode() == AppMode::Input {
        if block_count > 0 {
            render_separator(frame, chunks[chunk_idx], ctx);
            chunk_idx += 1;
        }

        render_input_block(frame, &app.input(), chunks[chunk_idx], ctx);

        // Set cursor position
        let (cursor_row, cursor_col) = calculate_cursor_position(&app.input(), content_width);
        let cursor_x = chunks[chunk_idx].x.saturating_add(cursor_col);
        let cursor_y = chunks[chunk_idx].y.saturating_add(cursor_row);
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_single_block(frame: &mut Frame, block: &Block, area: Rect, ctx: &RenderContext) {
    match block.kind {
        BlockKind::Input => {
            render_input_block(frame, &block.content, area, ctx);
        }
        BlockKind::Command => {
            render_command_block(frame, &block.content, area, ctx);
        }
        BlockKind::Spinner => {
            render_spinner_block(frame, block, area, ctx);
        }
        BlockKind::Text => {
            render_text_block(frame, &block.content, area, ctx);
        }
        BlockKind::Error => {
            render_error_block(frame, &block.content, area, ctx);
        }
    }
}

fn render_input_block(frame: &mut Frame, input: &str, area: Rect, ctx: &RenderContext) {
    let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Guidance));
    let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

    let prompt = if input.is_empty() {
        vec![Span::styled("> ", symbol_style)]
    } else {
        vec![
            Span::styled("> ", symbol_style),
            Span::styled(input, text_style),
        ]
    };

    let paragraph = Paragraph::new(Line::from(prompt)).wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn render_command_block(frame: &mut Frame, command: &str, area: Rect, ctx: &RenderContext) {
    let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Important));
    let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

    let content = vec![
        Span::styled("$ ", symbol_style),
        Span::styled(command, text_style),
    ];

    let paragraph = Paragraph::new(Line::from(content)).wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn render_spinner_block(frame: &mut Frame, block: &Block, area: Rect, ctx: &RenderContext) {
    let spinner_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));

    let spinner_frame = if let BlockState::Building { spinner_idx } = block.state {
        SPINNER_FRAMES[spinner_idx % SPINNER_FRAMES.len()]
    } else {
        SPINNER_FRAMES[0]
    };

    let content = format!("{} Generating...", spinner_frame);
    let paragraph = Paragraph::new(Span::styled(content, spinner_style));

    frame.render_widget(paragraph, area);
}

fn render_text_block(frame: &mut Frame, text: &str, area: Rect, ctx: &RenderContext) {
    let lines = markdown_to_spans(text, ctx.theme);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_error_block(frame: &mut Frame, message: &str, area: Rect, ctx: &RenderContext) {
    let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::AlertError));
    let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

    let content = vec![
        Span::styled("! ", symbol_style),
        Span::styled(message, text_style),
    ];

    let paragraph = Paragraph::new(Line::from(content)).wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn render_separator(frame: &mut Frame, area: Rect, ctx: &RenderContext) {
    let separator_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Muted));
    let width = usize::from(area.width).max(1);
    let separator = "─".repeat(width);

    let paragraph = Paragraph::new(Span::styled(separator, separator_style));
    frame.render_widget(paragraph, area);
}

fn get_footer_text(mode: &AppMode) -> &'static str {
    match mode {
        AppMode::Input => "[Enter]: Accept  [Esc]: Cancel",
        AppMode::Generating => "[Esc]: Cancel",
        AppMode::Streaming => "[Esc]: Cancel",
        AppMode::Review => "[Enter]: Run  [Tab]: Insert  [e]: Edit  [Esc]: Cancel",
        AppMode::Error => "[Enter]/[r]: Retry  [Esc]: Cancel",
    }
}

/// Convert markdown text to styled ratatui Lines
/// Supports: **bold**, *italics* (rendered as underline), `inline code`
pub fn markdown_to_spans<'a>(text: &'a str, theme: &'a Theme) -> Vec<Line<'a>> {
    let parser = Parser::new(text);
    let mut lines: Vec<Vec<Span<'a>>> = vec![Vec::new()];
    let mut current_line = 0;

    // Style stack for nested formatting
    let base_style = Style::from_crossterm(theme.as_style(Meaning::Base));
    let code_style = Style::from_crossterm(theme.as_style(Meaning::Important));
    let mut style_stack: Vec<Style> = vec![base_style];

    for event in parser {
        match event {
            Event::Start(Tag::Strong) => {
                // Bold
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
                // Italics -> underline per CONV-06 requirement
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
            Event::Code(code) => {
                // Inline code with Important styling (includes backticks visually)
                lines[current_line].push(Span::styled(format!("`{}`", code), code_style));
            }
            Event::Text(text) => {
                let current_style = style_stack.last().copied().unwrap_or(base_style);
                // Handle text that might contain newlines
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
                // Soft break = space
                let current_style = style_stack.last().copied().unwrap_or(base_style);
                lines[current_line].push(Span::styled(" ", current_style));
            }
            Event::HardBreak => {
                // Hard break = new line
                current_line += 1;
                lines.push(Vec::new());
            }
            Event::Start(Tag::Paragraph) => {
                // Start new paragraph - add blank line if not at start
                if current_line > 0 || !lines[0].is_empty() {
                    current_line += 1;
                    lines.push(Vec::new());
                }
            }
            Event::End(TagEnd::Paragraph) => {
                // End paragraph - will naturally flow to next
            }
            _ => {
                // Ignore other events (headings, lists, etc.) - just render as plain text
            }
        }
    }

    // Convert to Lines, filtering empty trailing lines
    lines.into_iter().map(Line::from).collect()
}

fn calculate_block_height(block: &Block, width: usize) -> u16 {
    match block.kind {
        BlockKind::Input => calculate_input_height(&block.content, width),
        BlockKind::Command => {
            let command_line = format!("$ {}", block.content);
            wrapped_line_count(&command_line, width) as u16
        }
        BlockKind::Spinner => 1, // Single line
        BlockKind::Text => wrapped_line_count(&block.content, width) as u16,
        BlockKind::Error => {
            let error_line = format!("! {}", block.content);
            wrapped_line_count(&error_line, width) as u16
        }
    }
}

fn calculate_input_height(input: &str, width: usize) -> u16 {
    let prompt_line = if input.is_empty() {
        "> ".to_string()
    } else {
        format!("> {}", input)
    };
    wrapped_line_count(&prompt_line, width) as u16
}

fn wrapped_line_count(text: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }

    text.split('\n')
        .map(|line| {
            let len = line.chars().count();
            len.max(1).div_ceil(width)
        })
        .sum::<usize>()
        .max(1)
}

fn calculate_cursor_position(input: &str, width: usize) -> (u16, u16) {
    if width == 0 {
        return (0, 0);
    }

    // The visible prompt line is always `> {input}`.
    // We mimic word-wrapping so cursor tracking matches visual layout.
    let mut row = 0usize;
    let mut col = 2usize; // "> "

    let mut saw_any_word = false;
    for word in input.split_whitespace() {
        let word_len = word.chars().count();
        if !saw_any_word {
            saw_any_word = true;
            if col + word_len <= width {
                col += word_len;
            } else if word_len >= width {
                let used = width.saturating_sub(col);
                let remaining = word_len.saturating_sub(used);
                row += 1 + (remaining / width);
                col = remaining % width;
            } else {
                row += 1;
                col = word_len;
            }
            continue;
        }

        if col + 1 + word_len <= width {
            col += 1 + word_len;
        } else if word_len >= width {
            row += 1 + (word_len / width);
            col = word_len % width;
        } else {
            row += 1;
            col = word_len;
        }
    }

    // Keep trailing spaces user typed.
    let trailing_spaces = input.chars().rev().take_while(|c| *c == ' ').count();
    for _ in 0..trailing_spaces {
        if col >= width {
            row += 1;
            col = 0;
        }
        col += 1;
    }

    (row as u16, col as u16)
}
