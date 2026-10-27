//! Numbering the lines of an output whose middle was discarded.

use std::ops::Range;

use easy_cast::Conv;

use super::py_style::PyStyleIdxRange;

/// A sequence whose middle was discarded: `head` kept lines followed by `tail` kept lines.
#[derive(Debug, Clone, Copy)]
pub struct KeptEnds {
    /// Number of kept lines at the start.
    pub head: usize,
    /// Number of kept lines at the end; `0` when nothing was discarded.
    pub tail: usize,
}

impl KeptEnds {
    #[must_use]
    pub fn from_fold(len: usize, tail_from: Option<usize>) -> Self {
        let head = tail_from.unwrap_or(len);
        Self {
            head,
            tail: len.saturating_sub(head),
        }
    }

    #[must_use]
    pub fn number(self, idx: usize) -> i64 {
        if idx < self.head {
            i64::conv(idx)
        } else {
            i64::conv(idx) - i64::conv(self.head + self.tail)
        }
    }

    #[must_use]
    pub fn head_range(self, range: Range<usize>) -> PyStyleIdxRange {
        PyStyleIdxRange::new(i64::conv(range.start), i64::conv(range.end) - 1)
    }

    #[must_use]
    pub fn tail_range(self, range: Range<usize>) -> PyStyleIdxRange {
        let tail = i64::conv(self.tail);
        PyStyleIdxRange::new(i64::conv(range.start) - tail, i64::conv(range.end) - 1 - tail)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::truncated(5, Some(3), 3, 2)]
    #[case::nothing_discarded(5, None, 5, 0)]
    #[case::empty(0, None, 0, 0)]
    fn from_fold_splits_head_and_tail(
        #[case] len: usize,
        #[case] tail_from: Option<usize>,
        #[case] head: usize,
        #[case] tail: usize,
    ) {
        let ends = KeptEnds::from_fold(len, tail_from);
        assert_eq!((ends.head, ends.tail), (head, tail));
    }

    #[rstest]
    // head 3, tail 2 (len 5): first three count up, last two count back from the end.
    #[case(KeptEnds { head: 3, tail: 2 }, 0, 0)]
    #[case(KeptEnds { head: 3, tail: 2 }, 2, 2)]
    #[case(KeptEnds { head: 3, tail: 2 }, 3, -2)]
    #[case(KeptEnds { head: 3, tail: 2 }, 4, -1)]
    // No tail: everything counts up from the start.
    #[case(KeptEnds { head: 5, tail: 0 }, 0, 0)]
    #[case(KeptEnds { head: 5, tail: 0 }, 4, 4)]
    fn number_is_positive_in_head_and_negative_in_tail(
        #[case] ends: KeptEnds,
        #[case] idx: usize,
        #[case] expected: i64,
    ) {
        assert_eq!(ends.number(idx), expected);
    }

    #[rstest]
    #[case(0..2, PyStyleIdxRange::new(0, 1))]
    #[case(2..3, PyStyleIdxRange::new(2, 2))]
    fn head_range_numbers_from_the_start(
        #[case] range: Range<usize>,
        #[case] expected: PyStyleIdxRange,
    ) {
        assert_eq!(KeptEnds { head: 3, tail: 2 }.head_range(range), expected);
    }

    #[rstest]
    // tail 2: indices 0-based into the tail map to the last two lines.
    #[case(0..2, PyStyleIdxRange::new(-2, -1))]
    #[case(1..2, PyStyleIdxRange::new(-1, -1))]
    fn tail_range_numbers_back_from_the_end(
        #[case] range: Range<usize>,
        #[case] expected: PyStyleIdxRange,
    ) {
        assert_eq!(KeptEnds { head: 3, tail: 2 }.tail_range(range), expected);
    }
}
