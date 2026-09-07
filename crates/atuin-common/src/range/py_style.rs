//! A Python-slice-style index range that resolves into a valid [`Range`] over a slice.

use std::ops::Range;

use easy_cast::Conv;

/// A slice range in Python-slice style: both ends **inclusive**, negatives count from the end.
///
/// Resolve it against a slice (or a length) with [`resolve_for`](Self::resolve_for) to get a plain
/// half-open [`Range<usize>`] that is always valid to index that slice with.
#[cfg_attr(feature = "proto", derive(prost::Message))]
#[cfg_attr(not(feature = "proto"), derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct PyStyleIdxRange {
    #[cfg_attr(feature = "proto", prost(int64, tag = "1"))]
    start: i64,
    #[cfg_attr(feature = "proto", prost(int64, tag = "2"))]
    end: i64,
}

impl PyStyleIdxRange {
    /// A range from `start` to `end`, both inclusive. Negative bounds count from the end (`-1` is
    /// the last element); the bounds are only interpreted against a concrete length in
    /// [`resolve_for`](Self::resolve_for).
    #[must_use]
    pub fn new(start: i64, end: i64) -> Self {
        Self { start, end }
    }

    /// The (possibly negative) inclusive start bound, as given.
    #[must_use]
    pub fn start(&self) -> i64 {
        self.start
    }

    /// The (possibly negative) inclusive end bound, as given.
    #[must_use]
    pub fn end(&self) -> i64 {
        self.end
    }

    /// Resolve these inclusive, possibly-negative bounds into a half-open [`Range<usize>`] that is
    /// always valid to index `slice` with.
    ///
    /// Negative indices count from the end of `slice`; out-of-range bounds are clamped; a backwards
    /// or empty range yields an empty (but still sliceable) range. In other words,
    /// `&slice[range.resolve_for(slice)]` never panics.
    #[must_use]
    pub fn resolve_for<T>(&self, slice: &[T]) -> Range<usize> {
        let len = i64::try_from(slice.len()).unwrap_or(i64::MAX);
        // Normalise a bound to a non-negative index from the front, saturating rather than wrapping
        // so pathological inputs stay well-behaved.
        let norm = |i: i64| {
            if i < 0 {
                len.saturating_add(i)
            } else {
                i
            }
        };

        let start = norm(self.start).clamp(0, len);
        let end = norm(self.end).saturating_add(1).clamp(0, len).max(start);

        usize::conv(start)..usize::conv(end)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::in_bounds(PyStyleIdxRange::new(1, 3), 1..4)]
    #[case::single_line(PyStyleIdxRange::new(2, 2), 2..3)]
    #[case::last_line_no_sentinel(PyStyleIdxRange::new(-1, -1), 4..5)]
    #[case::whole_via_negative_end(PyStyleIdxRange::new(0, -1), 0..5)]
    #[case::negative_from_end(PyStyleIdxRange::new(-2, -1), 3..5)]
    #[case::more_than_available_clamps(PyStyleIdxRange::new(-100, -1), 0..5)]
    #[case::clamped_past_end(PyStyleIdxRange::new(10, 20), 5..5)]
    #[case::backwards_resolves_empty(PyStyleIdxRange::new(3, 1), 3..3)]
    #[case::negative_backwards_resolves_empty(PyStyleIdxRange::new(-1, -5), 4..4)]
    fn resolve_for_is_always_sliceable(
        #[case] range: PyStyleIdxRange,
        #[case] expected: Range<usize>,
    ) {
        let slice = [(); 5];
        let resolved = range.resolve_for(&slice);
        assert_eq!(resolved, expected);
        // The invariant: whatever comes back can index the slice without panicking.
        let _ = &slice[resolved];
    }

    #[rstest]
    #[case::everything(PyStyleIdxRange::new(0, -1), 0..0)]
    #[case::last_line(PyStyleIdxRange::new(-1, -1), 0..0)]
    #[case::positive(PyStyleIdxRange::new(0, 5), 0..0)]
    fn resolve_for_empty_slice_never_panics(
        #[case] range: PyStyleIdxRange,
        #[case] expected: Range<usize>,
    ) {
        let slice: [(); 0] = [];
        let resolved = range.resolve_for(&slice);
        assert_eq!(resolved, expected);
        let _ = &slice[resolved];
    }
}
