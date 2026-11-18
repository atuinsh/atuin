//! The bar down the side of a page taller than the terminal, with a handle that drags.

use std::ops::Range;

use ratatui::style::Stylize;
use ratatui::text::{Line, Span};

use super::view::ACCENT;

/// How far down a page taller than the terminal is scrolled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Scroll {
    /// The page's length, in lines.
    lines: usize,
    /// The terminal's height, which the bar runs all of.
    height: usize,
    /// The first line shown.
    top: usize,
}

impl Scroll {
    /// `top` lines down a page `lines` long, or as far as it goes, on a terminal `height` lines
    /// tall. `None` when the page fits.
    pub(super) fn new(lines: usize, height: usize, top: usize) -> Option<Self> {
        (lines > height).then(|| Self {
            lines,
            height,
            top: top.min(lines - height),
        })
    }

    pub(super) const fn top(self) -> usize {
        self.top
    }

    /// How many rows the page shows, which the bar runs down.
    pub(super) const fn height(self) -> usize {
        self.height
    }

    /// The furthest down it scrolls, with the page's last line at the bottom.
    const fn bottom(self) -> usize {
        self.lines - self.height
    }

    /// The handle's length: as much of the bar as the terminal shows of the page.
    fn length(self) -> usize {
        (self.height * self.height / self.lines).max(1)
    }

    /// How far down the bar the handle moves.
    fn travel(self) -> usize {
        self.height.saturating_sub(self.length())
    }

    /// The rows of the bar the handle covers.
    pub(super) fn handle(self) -> Range<usize> {
        let start = (self.top * self.travel() + self.bottom() / 2) / self.bottom();
        start..start + self.length()
    }

    /// Scrolled to put the handle `start` rows down the bar, or as far down as it goes.
    pub(super) fn handle_at(self, start: usize) -> Self {
        let travel = self.travel();
        let top = (start.min(travel) * self.bottom() + travel / 2).checked_div(travel);
        Self {
            top: top.unwrap_or(0),
            ..self
        }
    }

    pub(super) fn down(self, lines: usize) -> Self {
        Self {
            top: (self.top + lines).min(self.bottom()),
            ..self
        }
    }

    pub(super) fn up(self, lines: usize) -> Self {
        Self {
            top: self.top.saturating_sub(lines),
            ..self
        }
    }

    /// The bar, a line for each row, with the handle bright while it's `held`.
    pub(super) fn bar(self, held: bool) -> Vec<Line<'static>> {
        let handle = self.handle();
        let row = |row| match (handle.contains(&row), held) {
            (true, true) => Line::from("█".fg(ACCENT)),
            (true, false) => Line::from(Span::raw("█")),
            (false, _) => Line::from("│".dim()),
        };
        (0..self.height).map(row).collect()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::at_the_top(0, 0..6)]
    #[case::halfway(6, 3..9)]
    #[case::at_the_bottom(12, 6..12)]
    #[case::past_the_bottom(20, 6..12)]
    fn the_handle_is_as_long_as_what_shows(#[case] top: usize, #[case] expected: Range<usize>) {
        assert_eq!(Scroll::new(24, 12, top).map(Scroll::handle), Some(expected));
    }

    #[rstest]
    #[case::fits(12)]
    #[case::shorter(5)]
    fn a_page_that_fits_has_no_bar(#[case] lines: usize) {
        assert_eq!(Scroll::new(lines, 12, 0), None);
    }

    proptest! {
        /// The handle stays on the bar, where it's dragged to.
        #[test]
        fn the_handle_is_where_it_is_dragged(
            height in 1..60usize,
            more in 1..200usize,
            to in 0..80usize,
        ) {
            let scroll = Scroll::new(height + more, height, 0).expect("the page is taller");
            let handle = scroll.handle_at(to).handle();
            prop_assert!(handle.end <= height);
            prop_assert_eq!(handle.start, to.min(height - handle.len()));
        }

        /// Dragged as far as it goes, the handle shows the page's first line, or its last. On a
        /// terminal a row tall, the handle is all of the bar, so it has nowhere to go.
        #[test]
        fn the_handle_reaches_both_ends(
            height in 2..60usize,
            more in 1..200usize,
            top in 0..260usize,
        ) {
            let scroll = Scroll::new(height + more, height, top).expect("the page is taller");
            prop_assert_eq!((scroll.handle_at(0).top, scroll.handle_at(height).top), (0, more));
        }
    }
}
