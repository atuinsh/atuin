//! Component-oriented rendering primitives for the TUI.
//!
//! Defines the `Component` trait and container types (`VStack`, `SymbolRow`, etc.)
//! that enable declarative, composable UI layout.

use atuin_client::theme::{Meaning, Theme};
use ratatui::{
    Frame, backend::FromCrossterm, layout::Rect, style::Style, text::Span, widgets::Paragraph,
};
use tui_textarea::TextArea;

/// Context passed through the component tree during rendering.
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

/// A renderable component with intrinsic sizing.
pub trait Component {
    /// Calculate the intrinsic height at the given width.
    fn height(&self, width: u16) -> u16;

    /// Render into the given area.
    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext);
}

/// Vertical stack of components.
///
/// Children are laid out top-to-bottom with optional spacing between them.
/// When `scroll_offset > 0`, content is scrolled so that only the visible
/// portion is rendered.
pub struct VStack {
    pub children: Vec<Box<dyn Component>>,
    pub spacing: u16,
    pub scroll_offset: u16,
}

impl VStack {
    pub fn new(children: Vec<Box<dyn Component>>) -> Self {
        Self {
            children,
            spacing: 0,
            scroll_offset: 0,
        }
    }
}

impl Component for VStack {
    fn height(&self, width: u16) -> u16 {
        if self.children.is_empty() {
            return 0;
        }
        let content: u16 = self.children.iter().map(|c| c.height(width)).sum();
        let gaps = (self.children.len() as u16 - 1) * self.spacing;
        content + gaps
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        if self.children.is_empty() {
            return;
        }

        let heights: Vec<u16> = self.children.iter().map(|c| c.height(area.width)).collect();

        let viewport_start = self.scroll_offset;
        let viewport_end = self.scroll_offset + area.height;

        let mut cum: u16 = 0;
        for (i, (child, &h)) in self.children.iter().zip(heights.iter()).enumerate() {
            let child_start = cum;
            let child_end = cum + h;

            // Render if any part of the child is within the viewport
            if child_end > viewport_start && child_start < viewport_end {
                let visible_start = child_start.max(viewport_start);
                let visible_end = child_end.min(viewport_end);

                let child_area = Rect {
                    x: area.x,
                    y: area.y + visible_start - viewport_start,
                    width: area.width,
                    height: visible_end - visible_start,
                };

                child.render(frame, child_area, ctx);
            }

            cum = child_end;
            if i < self.children.len() - 1 {
                cum += self.spacing;
            }
        }
    }
}

/// Fixed-height empty space.
pub struct Spacer(pub u16);

impl Component for Spacer {
    fn height(&self, _width: u16) -> u16 {
        self.0
    }

    fn render(&self, _frame: &mut Frame, _area: Rect, _ctx: &RenderContext) {}
}

/// A row with a symbol in column 0 and content in columns 2+.
///
/// This is the horizontal layout primitive used by all content types that
/// display a prefix symbol (>, $, !, ?, etc.) followed by text.
pub struct SymbolRow {
    pub symbol: String,
    pub symbol_meaning: Meaning,
    pub inner: Box<dyn Component>,
}

impl Component for SymbolRow {
    fn height(&self, width: u16) -> u16 {
        self.inner.height(width.saturating_sub(2))
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        // Render symbol at column 0, first row only
        let style = Style::from_crossterm(ctx.theme.as_style(self.symbol_meaning));
        let symbol_area = Rect {
            x: area.x,
            y: area.y,
            width: 1,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(self.symbol.as_str()).style(style),
            symbol_area,
        );

        // Render inner content at column 2+
        let content_area = Rect {
            x: area.x.saturating_add(2),
            y: area.y,
            width: area.width.saturating_sub(2),
            height: area.height,
        };
        self.inner.render(frame, content_area, ctx);
    }
}

/// Horizontal separator spanning the full card width (├───┤).
///
/// Extends beyond its content area to overlap the card's left and right borders.
pub struct Separator {
    pub card_width: u16,
}

impl Component for Separator {
    fn height(&self, _width: u16) -> u16 {
        1
    }

    fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext) {
        let style = Style::from_crossterm(ctx.theme.as_style(Meaning::Base));
        let inner_width = self.card_width.saturating_sub(2) as usize;
        let separator = format!(
            "\u{251c}{}\u{2524}",           // ├ ... ┤
            "\u{2500}".repeat(inner_width)  // ─
        );

        // Extend left to overlap the card border (content area is inset by border + padding)
        let sep_area = Rect {
            x: area.x.saturating_sub(2),
            y: area.y,
            width: self.card_width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Span::styled(separator, style)), sep_area);
    }
}
