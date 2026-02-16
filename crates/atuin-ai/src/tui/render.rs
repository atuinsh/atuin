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
use super::view_model::{Blocks, Content};

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
        .saturating_add(4) // padding + borders
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
    let outer_block = RatatuiBlock::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(Line::from(view.footer).alignment(Alignment::Right))
        .padding(Padding::uniform(1));

    let inner_area = outer_block.inner(card);
    frame.render_widget(outer_block, card);

    // Render blocks
    render_blocks_content(frame, view, ctx, inner_area);
}

fn render_blocks_content(frame: &mut Frame, view: &Blocks, ctx: &RenderContext, area: Rect) {
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
            render_separator(frame, chunks[chunk_idx], ctx);
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

    // Build layout constraints for each content item
    let constraints: Vec<Constraint> = content
        .iter()
        .map(|c| Constraint::Length(calculate_single_content_height(c, content_width)))
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (idx, item) in content.iter().enumerate() {
        render_single_content(frame, item, chunks[idx], ctx);
    }
}

/// Render a single content item
fn render_single_content(frame: &mut Frame, content: &Content, area: Rect, ctx: &RenderContext) {
    match content {
        Content::Input { text, .. } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Guidance));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            let spans = if text.is_empty() {
                vec![Span::styled("> ", symbol_style)]
            } else {
                vec![
                    Span::styled("> ", symbol_style),
                    Span::styled(text.as_str(), text_style),
                ]
            };

            let paragraph = Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        }

        Content::Command { text, faded } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Important));
            let mut text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
            if *faded {
                text_style = text_style.add_modifier(Modifier::DIM);
            }

            let spans = vec![
                Span::styled("$ ", symbol_style),
                Span::styled(text.as_str(), text_style),
            ];

            let paragraph = Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        }

        Content::Text { markdown } => {
            let lines = markdown_to_spans(markdown, ctx.theme);
            let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        }

        Content::Error { message } => {
            let symbol_style = Style::from_crossterm(ctx.theme.as_style(Meaning::AlertError));
            let text_style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));

            let spans = vec![
                Span::styled("! ", symbol_style),
                Span::styled(message.as_str(), text_style),
            ];

            let paragraph = Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        }

        Content::Spinner {
            frame: spinner_frame,
        } => {
            let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Annotation));
            let symbol = SPINNER_FRAMES[*spinner_frame % SPINNER_FRAMES.len()];
            let text = format!("{} Generating...", symbol);

            let paragraph = Paragraph::new(Span::styled(text, style));
            frame.render_widget(paragraph, area);
        }
    }
}

fn render_separator(frame: &mut Frame, area: Rect, ctx: &RenderContext) {
    let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Muted));
    let width = usize::from(area.width).max(1);
    let separator = "\u{2500}".repeat(width); // box drawing horizontal

    let paragraph = Paragraph::new(Span::styled(separator, style));
    frame.render_widget(paragraph, area);
}

/// Calculate total height for all content items in a block
fn calculate_block_height(content: &[Content], width: usize) -> u16 {
    content
        .iter()
        .map(|c| calculate_single_content_height(c, width))
        .sum()
}

/// Calculate height for a single content item
fn calculate_single_content_height(content: &Content, width: usize) -> u16 {
    match content {
        Content::Input { text, .. } => {
            let line = if text.is_empty() {
                "> ".to_string()
            } else {
                format!("> {}", text)
            };
            wrapped_line_count(&line, width) as u16
        }
        Content::Command { text, .. } => {
            let line = format!("$ {}", text);
            wrapped_line_count(&line, width) as u16
        }
        Content::Text { markdown } => wrapped_line_count(markdown, width) as u16,
        Content::Error { message } => {
            let line = format!("! {}", message);
            wrapped_line_count(&line, width) as u16
        }
        Content::Spinner { .. } => 1,
    }
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
            Event::Code(code) => {
                lines[current_line].push(Span::styled(format!("`{}`", code), code_style));
            }
            Event::Text(text) => {
                let current_style = style_stack.last().copied().unwrap_or(base_style);
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
