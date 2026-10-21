//! Selecting text with the mouse. The picker has the mouse (for the wheel), so the terminal can't
//! select; a drag selects instead, kept to the pane it started in, and letting go copies it.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use unicode_width::UnicodeWidthStr;

/// A drag, from where it started to where it is, in screen cells, within `area`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub area: Rect,
    pub from: Position,
    pub to: Position,
}

impl Selection {
    /// Start at `at` (on a pane's border, at the nearest of its text).
    pub fn new(area: Rect, at: Position) -> Self {
        let at = clamp(area, at);
        Self {
            area,
            from: at,
            to: at,
        }
    }

    /// Drag to `at`, kept within the area.
    pub fn extend(&mut self, at: Position) {
        self.to = clamp(self.area, at);
    }

    /// Each row selected, with its first and last column: as a terminal selects, from the start
    /// to the end of the area's width, across whole rows between.
    fn rows(&self) -> impl Iterator<Item = (u16, u16, u16)> + '_ {
        let (start, end) = if (self.from.y, self.from.x) <= (self.to.y, self.to.x) {
            (self.from, self.to)
        } else {
            (self.to, self.from)
        };
        let last = self.area.right().saturating_sub(1);
        (start.y..=end.y).map(move |y| {
            let first = if y == start.y {
                start.x
            } else {
                self.area.left()
            };
            (
                y,
                first,
                if y == end.y {
                    end.x
                } else {
                    last
                },
            )
        })
    }

    /// Show the selection in `buf`, reversed.
    pub fn highlight(&self, buf: &mut Buffer) {
        for (y, first, last) in self.rows() {
            for x in first..=last {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.modifier.insert(Modifier::REVERSED);
                }
            }
        }
    }

    /// The text selected in `buf`, each row without trailing blanks, and the rows without the
    /// indent they share (the pane's margin) or blank rows before and after them; `None` for a
    /// click (no drag) or nothing but blanks.
    pub fn text(&self, buf: &Buffer) -> Option<String> {
        if self.from == self.to {
            return None;
        }
        let rows: Vec<String> = self
            .rows()
            .map(|(y, first, last)| {
                let mut row = String::new();
                let mut x = first;
                while x <= last {
                    let Some(cell) = buf.cell((x, y)) else {
                        break;
                    };
                    row.push_str(cell.symbol());
                    // A wide character's cell stands for the ones it covers.
                    x += u16::try_from(cell.symbol().width().max(1)).unwrap_or(1);
                }
                row.trim_end().to_owned()
            })
            .collect();
        let indent = rows
            .iter()
            .filter(|row| !row.is_empty())
            .map(|row| row.len() - row.trim_start_matches(' ').len())
            .min()
            .unwrap_or(0);
        let rows: Vec<&str> =
            rows.iter().map(|row| row.get(indent..).unwrap_or_default()).collect();
        let text = rows.join("\n").trim_matches('\n').to_owned();
        (!text.trim().is_empty()).then_some(text)
    }
}

fn clamp(area: Rect, at: Position) -> Position {
    Position::new(
        at.x.clamp(area.left(), area.right().saturating_sub(1)),
        at.y.clamp(area.top(), area.bottom().saturating_sub(1)),
    )
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget};
    use rstest::rstest;

    use super::*;

    fn screen() -> Buffer {
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        Paragraph::new(vec![Line::from("first line"), Line::from("宽 wide"), Line::from("third")])
            .render(area, &mut buf);
        buf
    }

    fn drag(from: (u16, u16), to: (u16, u16)) -> Selection {
        let mut s = Selection::new(Rect::new(0, 0, 12, 3), from.into());
        s.extend(to.into());
        s
    }

    #[rstest]
    #[case::in_a_row((0, 0), (4, 0), Some("first"))]
    #[case::backwards((4, 0), (0, 0), Some("first"))]
    #[case::across_rows((6, 0), (2, 2), Some("line\n宽 wide\nthi"))]
    #[case::past_the_area((3, 1), (40, 9), Some("wide\nthird"))]
    #[case::a_click((3, 0), (3, 0), None)]
    #[case::blanks((10, 0), (11, 0), None)]
    fn a_drag_selects_text(
        #[case] from: (u16, u16),
        #[case] to: (u16, u16),
        #[case] want: Option<&str>,
    ) {
        assert_eq!(drag(from, to).text(&screen()).as_deref(), want);
    }

    /// The margin and blank rows around the text aren't part of it; indents and box drawing
    /// within it are.
    #[rstest]
    fn the_shared_indent_is_left_out() {
        let area = Rect::new(0, 0, 12, 5);
        let mut buf = Buffer::empty(area);
        let lines = ["", " first", "   • nested", " ├─ │foo│", ""];
        Paragraph::new(lines.map(Line::from).to_vec()).render(area, &mut buf);
        let mut s = Selection::new(area, (0, 0).into());
        s.extend((11, 4).into());
        assert_eq!(
            s.text(&buf).as_deref(),
            Some(
                "first
  • nested
├─ │foo│"
            )
        );
    }

    #[rstest]
    fn the_selection_is_reversed() {
        let mut buf = screen();
        drag((0, 0), (4, 0)).highlight(&mut buf);
        assert!(buf[(4, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(5, 0)].modifier.contains(Modifier::REVERSED));
    }
}
