use atuin_client::theme::{Meaning, Theme};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    backend::FromCrossterm,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block as RatatuiBlock, Borders, Padding},
};

use super::component::Component;
pub use super::component::RenderContext;
use super::components::build_component_tree;
use super::spinner::active_frame;
use super::state::AppState;
use super::view_model::Blocks;

/// Fixed card width for the TUI
pub(crate) const CARD_WIDTH: u16 = 64;

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
    let content_width = w.saturating_sub(4).max(1);

    let items: Vec<_> = view.items.iter().collect();
    let tree = build_component_tree(&items, w);

    // Add borders (2) + top padding (1), minimum 5
    tree.height(content_width).saturating_add(3).max(5)
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

    // Build component tree from view model
    let mut tree = build_component_tree(&items, desired_width);
    let content_width = desired_width.saturating_sub(4).max(1);

    let desired_height = tree.height(content_width).saturating_add(3).max(5);

    // Cap card height at viewport height to prevent overflow
    let actual_height = desired_height.min(area.height);

    // Calculate scroll offset to keep the active content visible when overflowing.
    // When render_above=false (popup below cursor), items are reversed so the active
    // content (input/spinner) is at the top — scroll_offset stays 0 to show the top.
    // Otherwise, scroll to show the bottom where the active content lives.
    tree.scroll_offset = if ctx.popup_mode && !ctx.render_above {
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
        .title_top(Line::from("atuin").alignment(Alignment::Right))
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

    // Render the component tree
    tree.render(frame, inner_area, ctx);
}

/// Convert markdown to styled spans
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
