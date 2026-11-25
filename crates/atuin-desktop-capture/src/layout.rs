//! The geometry of a window's elements: the order in which a person reads them, and whether
//! two pieces of text leave room for a blank between them.

/// Where an element is on the screen, in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    /// The area in square pixels. A rectangle with a negative side has none.
    #[must_use]
    pub fn area(self) -> i64 {
        i64::from(self.width.max(0)) * i64::from(self.height.max(0))
    }

    // Applications report the extents, so the sums saturate instead of overflowing.
    const fn bottom(self) -> i32 {
        self.y.saturating_add(self.height)
    }

    const fn middle(self) -> i32 {
        self.y.saturating_add(self.height / 2)
    }
}

/// Where an element is on the screen, in the platform's screen coordinates, which can fall
/// between pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    /// How far apart two pieces of text may be and still touch. Platforms that place text
    /// between pixels round the frames of adjacent runs differently.
    const TOUCH_TOLERANCE: f64 = 0.5;

    #[must_use]
    pub fn has_area(self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }

    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// Whether `next`, the frame of the text after this one, leaves room between the two.
    ///
    /// Two pieces of text that touch on one line are one word in two styles, such as a bold
    /// first letter. Any other pair is two words.
    #[must_use]
    pub fn leaves_room_for(self, next: Self) -> bool {
        !self.shares_line(next) || next.x > self.right() + Self::TOUCH_TOLERANCE
    }

    /// Whether the two frames overlap vertically, as two pieces of one line of text do.
    #[must_use]
    pub fn shares_line(self, other: Self) -> bool {
        other.y < self.bottom() && self.y < other.bottom()
    }

    /// The frame rounded to whole pixels.
    #[must_use]
    #[expect(clippy::cast_possible_truncation, reason = "screen coordinates fit in an i32")]
    pub fn on_pixels(self) -> Rect {
        Rect {
            x: self.x.round() as i32,
            y: self.y.round() as i32,
            width: self.width.round() as i32,
            height: self.height.round() as i32,
        }
    }

    fn right(self) -> f64 {
        self.x + self.width
    }

    fn bottom(self) -> f64 {
        self.y + self.height
    }
}

impl From<Rect> for Frame {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.x.into(),
            y: rect.y.into(),
            width: rect.width.into(),
            height: rect.height.into(),
        }
    }
}

/// Returns the indices of `rects` in reading order: rows from top to bottom, and each row from
/// left to right.
///
/// An element joins a row when its vertical centre lies above the bottom of that row, so a label
/// and a taller field beside it stay together. Elements without a position keep their relative
/// order and follow the positioned ones.
#[must_use]
pub fn reading_order(rects: &[Option<Rect>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by_key(|&i| rects[i].map_or((1, 0, 0), |r| (0, r.y, r.x)));

    // Walk down the positioned elements and number the rows.
    let mut rows = vec![usize::MAX; rects.len()];
    let mut row = 0;
    let mut row_bottom = i32::MIN;
    for &i in &order {
        let Some(rect) = rects[i] else {
            break;
        };
        if row_bottom != i32::MIN && rect.middle() >= row_bottom {
            row += 1;
            row_bottom = i32::MIN;
        }
        row_bottom = row_bottom.max(rect.bottom());
        rows[i] = row;
    }
    order.sort_by_key(|&i| (rows[i], rects[i].map_or(0, |r| r.x)));
    order
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[expect(
        clippy::unnecessary_wraps,
        reason = "the test tables mix located and unlocated elements"
    )]
    const fn rect(x: i32, y: i32, width: i32, height: i32) -> Option<Rect> {
        Some(Rect {
            x,
            y,
            width,
            height,
        })
    }

    #[rstest]
    #[case::already_ordered(&[rect(0, 0, 100, 20), rect(0, 30, 100, 20)], &[0, 1])]
    #[case::reversed_rows(&[rect(0, 60, 100, 20), rect(0, 30, 100, 20), rect(0, 0, 100, 20)], &[2, 1, 0])]
    // A grid row: the field starts higher than its label, and comes after it.
    #[case::label_beside_a_taller_field(
        &[rect(120, 26, 80, 34), rect(0, 32, 100, 20), rect(0, 0, 100, 20)],
        &[2, 1, 0]
    )]
    #[case::sidebar_beside_rows(
        &[rect(200, 0, 300, 20), rect(200, 30, 300, 20), rect(0, 0, 180, 600)],
        &[2, 0, 1]
    )]
    #[case::unpositioned_keep_their_order_at_the_end(
        &[None, rect(0, 30, 10, 10), None, rect(0, 0, 10, 10)],
        &[3, 1, 0, 2]
    )]
    #[case::nothing(&[], &[])]
    #[case::extents_at_the_limits(
        &[rect(0, i32::MAX, 10, i32::MAX), rect(i32::MAX, 0, i32::MAX, 10), rect(0, i32::MIN, 10, 10)],
        &[2, 1, 0]
    )]
    fn elements_are_ordered_by_rows_then_columns(
        #[case] rects: &[Option<Rect>],
        #[case] expected: &[usize],
    ) {
        assert_eq!(reading_order(rects), expected);
    }

    const fn text_box(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[rstest]
    #[case::touching(text_box(0, 0, 9, 19), text_box(9, 0, 9, 19), false)]
    #[case::overlapping(text_box(0, 0, 9, 19), text_box(8, 0, 9, 19), false)]
    #[case::a_blank_apart(text_box(0, 0, 9, 19), text_box(13, 0, 9, 19), true)]
    #[case::next_line(text_box(300, 0, 9, 19), text_box(0, 22, 9, 19), true)]
    #[case::raised_on_one_line(text_box(0, 4, 9, 15), text_box(9, 0, 6, 10), false)]
    #[case::extents_at_the_limits(
        text_box(10, 10, i32::MAX, i32::MAX),
        text_box(i32::MAX, 20, 1, 1),
        false
    )]
    fn room_between_text_boxes(#[case] before: Rect, #[case] after: Rect, #[case] expected: bool) {
        assert_eq!(Frame::from(before).leaves_room_for(Frame::from(after)), expected);
    }

    const fn frame(x: f64, width: f64) -> Frame {
        Frame {
            x,
            y: 0.0,
            width,
            height: 19.0,
        }
    }

    #[rstest]
    #[case::rounded_apart(frame(0.0, 9.3), frame(9.7, 9.0), false)]
    #[case::at_the_tolerance(frame(0.0, 9.0), frame(9.5, 9.0), false)]
    #[case::beyond_the_tolerance(frame(0.0, 9.0), frame(9.6, 9.0), true)]
    fn runs_between_pixels_touch_within_half_a_pixel(
        #[case] before: Frame,
        #[case] after: Frame,
        #[case] expected: bool,
    ) {
        assert_eq!(before.leaves_room_for(after), expected);
    }

    #[rstest]
    #[case::inside(frame(10.0, 5.0), true)]
    #[case::touching_the_edge(frame(100.0, 5.0), false)]
    #[case::beside(frame(200.0, 5.0), false)]
    fn frames_intersect_only_when_they_overlap(#[case] other: Frame, #[case] expected: bool) {
        assert_eq!(frame(0.0, 100.0).intersects(other), expected);
    }
}
