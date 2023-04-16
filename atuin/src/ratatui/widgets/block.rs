use crate::ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    symbols::line,
    text::{Span, Spans},
    widgets::{Borders, Widget},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderType {
    Plain,
    Rounded,
    Double,
    Thick,
}

impl BorderType {
    pub fn line_symbols(border_type: BorderType) -> line::Set {
        match border_type {
            BorderType::Plain => line::NORMAL,
            BorderType::Rounded => line::ROUNDED,
            BorderType::Double => line::DOUBLE,
            BorderType::Thick => line::THICK,
        }
    }
}

/// Base widget to be used with all upper level ones. It may be used to display a box border around
/// the widget and/or add a title.
///
/// # Examples
///
/// ```
/// # use ratatui::widgets::{Block, BorderType, Borders};
/// # use ratatui::style::{Style, Color};
/// Block::default()
///     .title("Block")
///     .borders(Borders::LEFT | Borders::RIGHT)
///     .border_style(Style::default().fg(Color::White))
///     .border_type(BorderType::Rounded)
///     .style(Style::default().bg(Color::Black));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block<'a> {
    /// Optional title place on the upper left of the block
    title: Option<Spans<'a>>,
    /// Title alignment. The default is top left of the block, but one can choose to place
    /// title in the top middle, or top right of the block
    title_alignment: Alignment,
    /// Visible borders
    borders: Borders,
    /// Border style
    border_style: Style,
    /// Type of the border. The default is plain lines but one can choose to have rounded corners
    /// or doubled lines instead.
    border_type: BorderType,
    /// Widget style
    style: Style,
}

impl<'a> Default for Block<'a> {
    fn default() -> Block<'a> {
        Block {
            title: None,
            title_alignment: Alignment::Left,
            borders: Borders::NONE,
            border_style: Default::default(),
            border_type: BorderType::Plain,
            style: Default::default(),
        }
    }
}

impl<'a> Block<'a> {
    pub fn title<T>(mut self, title: T) -> Block<'a>
    where
        T: Into<Spans<'a>>,
    {
        self.title = Some(title.into());
        self
    }

    #[deprecated(
        since = "0.10.0",
        note = "You should use styling capabilities of `text::Spans` given as argument of the `title` method to apply styling to the title."
    )]
    pub fn title_style(mut self, style: Style) -> Block<'a> {
        if let Some(t) = self.title {
            let title = String::from(t);
            self.title = Some(Spans::from(Span::styled(title, style)));
        }
        self
    }

    pub fn title_alignment(mut self, alignment: Alignment) -> Block<'a> {
        self.title_alignment = alignment;
        self
    }

    pub fn border_style(mut self, style: Style) -> Block<'a> {
        self.border_style = style;
        self
    }

    pub fn style(mut self, style: Style) -> Block<'a> {
        self.style = style;
        self
    }

    pub fn borders(mut self, flag: Borders) -> Block<'a> {
        self.borders = flag;
        self
    }

    pub fn border_type(mut self, border_type: BorderType) -> Block<'a> {
        self.border_type = border_type;
        self
    }

    /// Compute the inner area of a block based on its border visibility rules.
    pub fn inner(&self, area: Rect) -> Rect {
        let mut inner = area;
        if self.borders.intersects(Borders::LEFT) {
            inner.x = inner.x.saturating_add(1).min(inner.right());
            inner.width = inner.width.saturating_sub(1);
        }
        if self.borders.intersects(Borders::TOP) || self.title.is_some() {
            inner.y = inner.y.saturating_add(1).min(inner.bottom());
            inner.height = inner.height.saturating_sub(1);
        }
        if self.borders.intersects(Borders::RIGHT) {
            inner.width = inner.width.saturating_sub(1);
        }
        if self.borders.intersects(Borders::BOTTOM) {
            inner.height = inner.height.saturating_sub(1);
        }
        inner
    }
}

impl<'a> Widget for Block<'a> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.area() == 0 {
            return;
        }
        buf.set_style(area, self.style);
        let symbols = BorderType::line_symbols(self.border_type);

        // Sides
        if self.borders.intersects(Borders::LEFT) {
            for y in area.top()..area.bottom() {
                buf.get_mut(area.left(), y)
                    .set_symbol(symbols.vertical)
                    .set_style(self.border_style);
            }
        }
        if self.borders.intersects(Borders::TOP) {
            for x in area.left()..area.right() {
                buf.get_mut(x, area.top())
                    .set_symbol(symbols.horizontal)
                    .set_style(self.border_style);
            }
        }
        if self.borders.intersects(Borders::RIGHT) {
            let x = area.right() - 1;
            for y in area.top()..area.bottom() {
                buf.get_mut(x, y)
                    .set_symbol(symbols.vertical)
                    .set_style(self.border_style);
            }
        }
        if self.borders.intersects(Borders::BOTTOM) {
            let y = area.bottom() - 1;
            for x in area.left()..area.right() {
                buf.get_mut(x, y)
                    .set_symbol(symbols.horizontal)
                    .set_style(self.border_style);
            }
        }

        // Corners
        if self.borders.contains(Borders::RIGHT | Borders::BOTTOM) {
            buf.get_mut(area.right() - 1, area.bottom() - 1)
                .set_symbol(symbols.bottom_right)
                .set_style(self.border_style);
        }
        if self.borders.contains(Borders::RIGHT | Borders::TOP) {
            buf.get_mut(area.right() - 1, area.top())
                .set_symbol(symbols.top_right)
                .set_style(self.border_style);
        }
        if self.borders.contains(Borders::LEFT | Borders::BOTTOM) {
            buf.get_mut(area.left(), area.bottom() - 1)
                .set_symbol(symbols.bottom_left)
                .set_style(self.border_style);
        }
        if self.borders.contains(Borders::LEFT | Borders::TOP) {
            buf.get_mut(area.left(), area.top())
                .set_symbol(symbols.top_left)
                .set_style(self.border_style);
        }

        // Title
        if let Some(title) = self.title.as_ref() {
            let left_border_dx = if self.borders.intersects(Borders::LEFT) {
                1
            } else {
                0
            };

            let right_border_dx = if self.borders.intersects(Borders::RIGHT) {
                1
            } else {
                0
            };

            let title_area_width = area
                .width
                .saturating_sub(left_border_dx)
                .saturating_sub(right_border_dx);

            let title_dx = match self.title_alignment {
                Alignment::Left => left_border_dx,
                Alignment::Center => area.width.saturating_sub(title.width() as u16) / 2,
                Alignment::Right => area
                    .width
                    .saturating_sub(title.width() as u16)
                    .saturating_sub(right_border_dx),
            };

            let title_x = area.left() + title_dx;
            let title_y = area.top();

            buf.set_spans(title_x, title_y, &title, title_area_width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratatui::layout::Rect;

    #[test]
    fn inner_takes_into_account_the_borders() {
        // No borders
        assert_eq!(
            Block::default().inner(Rect::default()),
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0
            },
            "no borders, width=0, height=0"
        );
        assert_eq!(
            Block::default().inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            },
            "no borders, width=1, height=1"
        );

        // Left border
        assert_eq!(
            Block::default().borders(Borders::LEFT).inner(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1
            }),
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1
            },
            "left, width=0"
        );
        assert_eq!(
            Block::default().borders(Borders::LEFT).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            Rect {
                x: 1,
                y: 0,
                width: 0,
                height: 1
            },
            "left, width=1"
        );
        assert_eq!(
            Block::default().borders(Borders::LEFT).inner(Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1
            }),
            Rect {
                x: 1,
                y: 0,
                width: 1,
                height: 1
            },
            "left, width=2"
        );

        // Top border
        assert_eq!(
            Block::default().borders(Borders::TOP).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 0
            }),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 0
            },
            "top, height=0"
        );
        assert_eq!(
            Block::default().borders(Borders::TOP).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            Rect {
                x: 0,
                y: 1,
                width: 1,
                height: 0
            },
            "top, height=1"
        );
        assert_eq!(
            Block::default().borders(Borders::TOP).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 2
            }),
            Rect {
                x: 0,
                y: 1,
                width: 1,
                height: 1
            },
            "top, height=2"
        );

        // Right border
        assert_eq!(
            Block::default().borders(Borders::RIGHT).inner(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1
            }),
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1
            },
            "right, width=0"
        );
        assert_eq!(
            Block::default().borders(Borders::RIGHT).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1
            },
            "right, width=1"
        );
        assert_eq!(
            Block::default().borders(Borders::RIGHT).inner(Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1
            }),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            },
            "right, width=2"
        );

        // Bottom border
        assert_eq!(
            Block::default().borders(Borders::BOTTOM).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 0
            }),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 0
            },
            "bottom, height=0"
        );
        assert_eq!(
            Block::default().borders(Borders::BOTTOM).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 0
            },
            "bottom, height=1"
        );
        assert_eq!(
            Block::default().borders(Borders::BOTTOM).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 2
            }),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            },
            "bottom, height=2"
        );

        // All borders
        assert_eq!(
            Block::default()
                .borders(Borders::ALL)
                .inner(Rect::default()),
            Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0
            },
            "all borders, width=0, height=0"
        );
        assert_eq!(
            Block::default().borders(Borders::ALL).inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            Rect {
                x: 1,
                y: 1,
                width: 0,
                height: 0,
            },
            "all borders, width=1, height=1"
        );
        assert_eq!(
            Block::default().borders(Borders::ALL).inner(Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            }),
            Rect {
                x: 1,
                y: 1,
                width: 0,
                height: 0,
            },
            "all borders, width=2, height=2"
        );
        assert_eq!(
            Block::default().borders(Borders::ALL).inner(Rect {
                x: 0,
                y: 0,
                width: 3,
                height: 3,
            }),
            Rect {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            },
            "all borders, width=3, height=3"
        );
    }

    #[test]
    fn inner_takes_into_account_the_title() {
        assert_eq!(
            Block::default().title("Test").inner(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            }),
            Rect {
                x: 0,
                y: 1,
                width: 0,
                height: 0,
            },
        );
        assert_eq!(
            Block::default()
                .title("Test")
                .title_alignment(Alignment::Center)
                .inner(Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 1,
                }),
            Rect {
                x: 0,
                y: 1,
                width: 0,
                height: 0,
            },
        );
        assert_eq!(
            Block::default()
                .title("Test")
                .title_alignment(Alignment::Right)
                .inner(Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 1,
                }),
            Rect {
                x: 0,
                y: 1,
                width: 0,
                height: 0,
            },
        );
    }
}
