//! A Python-slice-style index range that resolves into a valid [`Range`] over a slice.

use std::ops::Range;

use easy_cast::Conv;

/// A slice range in Python-slice style: both ends **inclusive**, negatives count from the end.
///
/// Resolve it against a slice (or a length) with [`resolve_for`](Self::resolve_for) to get a plain
/// half-open [`Range<usize>`] that is always valid to index that slice with.
///
/// NOTE: Unlike Python, this is an **inclusive** range!
#[cfg_attr(feature = "proto", derive(prost::Message))]
#[cfg_attr(not(feature = "proto"), derive(Debug))]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PyStyleIdxRange {
    #[cfg_attr(feature = "proto", prost(int64, tag = "1"))]
    pub start: i64,
    #[cfg_attr(feature = "proto", prost(int64, tag = "2"))]
    pub end: i64,
}

impl PyStyleIdxRange {
    /// A range from `start` to `end`, both inclusive. Negative bounds count from the end (`-1` is
    /// the last element); the bounds are only interpreted against a concrete length in
    /// [`resolve_for`](Self::resolve_for).
    #[must_use]
    pub fn new(start: i64, end: i64) -> Self {
        Self { start, end }
    }

    /// Resolve these inclusive, possibly-negative bounds into a half-open [`Range<usize>`] that is
    /// always valid to index `slice` with.
    ///
    /// Negative indices count from the end of `slice`; out-of-range bounds are clamped; a backwards
    /// or empty range yields an empty (but still sliceable) range. In other words,
    /// `&slice[range.resolve_for(slice)]` never panics.
    #[must_use]
    pub fn resolve_for<T>(self, slice: &[T]) -> Range<usize> {
        let len = u64::try_from(slice.len()).unwrap_or(u64::MAX);
        // Normalise a bound to a non-negative index from the front. If a negative `i` would point
        // before the start of the slice, this returns `None`.
        let norm = |i: i64| {
            let abs = i.unsigned_abs();
            if i < 0 {
                len.checked_sub(abs)
            } else {
                Some(abs)
            }
        };

        let start = norm(self.start).unwrap_or_default().clamp(0, len);
        let end = norm(self.end).map_or_default(|n| n.saturating_add(1)).clamp(start, len);
        usize::conv(start)..usize::conv(end)
    }

    /// Resolve this range into concrete indices given a pair of slices that represent the start
    /// and end of a sequence for which the middle was discarded.
    ///
    /// In this case, negative indices exclusively refer to elements in `end`, while positive
    /// indices exclusively refer to elements in `start`.
    ///
    /// Returns a [`SplitRanges`] object; see that type for more information.
    pub fn resolve_for_split<T>(self, start: &[T], end: &[T]) -> SplitRanges {
        if self.start >= 0 && self.end >= 0 {
            let start_range = self.resolve_for(start);
            let expected_unsigned_end =
                usize::try_from(self.end).ok().and_then(|n| n.checked_add(1));

            return SplitRanges {
                truncated: self.start <= self.end
                    && expected_unsigned_end.is_none_or(|end| end > start_range.end),
                start: start_range,
                end: 0..0,
            };
        }

        if self.start < 0 && self.end < 0 {
            let end_range = self.resolve_for(end);
            let expected_unsigned_start = usize::try_from(self.start.unsigned_abs())
                .ok()
                .and_then(|n| end.len().checked_sub(n));

            return SplitRanges {
                truncated: self.start <= self.end
                    && expected_unsigned_start.is_none_or(|start| start < end_range.start),
                start: 0..0,
                end: end_range,
            };
        }

        if self.start < 0 {
            // `self.end >= 0` in this branch.
            return SplitRanges {
                truncated: false,
                start: 0..0,
                end: 0..0,
            };
        }

        // Here `self.start >= 0` and `self.end < 0`.
        SplitRanges {
            truncated: true,
            start: Self {
                start: self.start,
                end: -1,
            }
            .resolve_for(start),
            end: Self {
                start: 0,
                end: self.end,
            }
            .resolve_for(end),
        }
    }
}

/// The result of [`PyStyleIdxRange::resolve_for_split`].
#[derive(Debug, Clone)]
pub struct SplitRanges {
    /// The range of indices within the `start` slice that the py-style range mapped to. Possibly
    /// empty.
    pub start: Range<usize>,
    /// The range of indices within the `end` slice that the py-style range mapped to. Possibly
    /// empty.
    pub end: Range<usize>,
    /// Whether the py-style range included indices that would map to the area between the start and
    /// end slices (i.e., the truncated part).
    ///
    /// If the start index was positive while the end index was negative, this is always true.
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
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
    // An inclusive end bound landing before the slice selects nothing, rather than clamping
    // forward onto the first element and handing back one the caller never asked for.
    #[case::end_before_the_slice(PyStyleIdxRange::new(0, -100), 0..0)]
    #[case::end_before_the_slice_from_within(PyStyleIdxRange::new(2, -100), 2..2)]
    #[case::both_bounds_before_the_slice(PyStyleIdxRange::new(-100, -100), 0..0)]
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

    /// A sequence whose middle was discarded: `a b c` then a gap then `x y z`. Positive indices
    /// count into the kept start, negative ones back from the kept end.
    const START: [&str; 3] = ["a", "b", "c"];
    const END: [&str; 3] = ["x", "y", "z"];

    /// What `range` picks out of the split sequence: the elements, then whether the request
    /// reached into the discarded middle.
    fn split(range: PyStyleIdxRange) -> (Vec<&'static str>, bool) {
        let ranges = range.resolve_for_split(&START, &END);
        // The invariant, same as `resolve_for`'s: both halves are always sliceable.
        let picked = [&START[ranges.start], &END[ranges.end]].concat();
        (picked, ranges.truncated)
    }

    #[rstest]
    // -- Wholly within the kept start ----------------------------------------
    #[case::first_line(PyStyleIdxRange::new(0, 0), vec!["a"], false)]
    #[case::within_the_start(PyStyleIdxRange::new(1, 2), vec!["b", "c"], false)]
    #[case::exactly_the_start(PyStyleIdxRange::new(0, 2), vec!["a", "b", "c"], false)]
    // Asking for a fourth line reaches past what was kept, into the gap.
    #[case::just_past_the_start(PyStyleIdxRange::new(0, 3), vec!["a", "b", "c"], true)]
    #[case::wholly_inside_the_gap(PyStyleIdxRange::new(5, 9), vec![], true)]
    #[case::backwards_is_empty(PyStyleIdxRange::new(2, 1), vec![], false)]
    // -- Wholly within the kept end ------------------------------------------
    #[case::last_line(PyStyleIdxRange::new(-1, -1), vec!["z"], false)]
    #[case::exactly_the_end(PyStyleIdxRange::new(-3, -1), vec!["x", "y", "z"], false)]
    // One before the kept end is in the gap.
    #[case::just_past_the_end(PyStyleIdxRange::new(-4, -1), vec!["x", "y", "z"], true)]
    #[case::wholly_inside_the_gap_from_behind(PyStyleIdxRange::new(-9, -5), vec![], true)]
    #[case::negative_backwards_is_empty(PyStyleIdxRange::new(-1, -3), vec![], false)]
    // -- Spanning the gap ----------------------------------------------------
    #[case::everything(PyStyleIdxRange::new(0, -1), vec!["a", "b", "c", "x", "y", "z"], true)]
    #[case::across_the_gap(PyStyleIdxRange::new(1, -2), vec!["b", "c", "x", "y"], true)]
    // The far side of the gap is never reached, so the kept end contributes nothing.
    #[case::across_into_the_gap(PyStyleIdxRange::new(1, -9), vec!["b", "c"], true)]
    // -- Running backwards across the gap ------------------------------------
    // A negative start indexes the kept end and a non-negative end indexes the kept start, so
    // these run backwards over the sequence and pick nothing -- including when the end is `0`,
    // which used to fall through to the spanning case and return a nonsensical `c, x`.
    #[case::from_the_end_back_to_the_start(PyStyleIdxRange::new(-2, 1), vec![], false)]
    #[case::from_the_end_back_to_line_zero(PyStyleIdxRange::new(-2, 0), vec![], false)]
    fn resolve_for_split_picks_from_both_halves(
        #[case] range: PyStyleIdxRange,
        #[case] expected: Vec<&str>,
        #[case] truncated: bool,
    ) {
        assert_eq!(split(range), (expected, truncated));
    }

    #[rstest]
    #[case::everything(PyStyleIdxRange::new(0, -1))]
    #[case::positive(PyStyleIdxRange::new(0, 5))]
    #[case::negative(PyStyleIdxRange::new(-5, -1))]
    #[case::spanning(PyStyleIdxRange::new(2, -2))]
    #[case::extremes(PyStyleIdxRange::new(i64::MIN, i64::MAX))]
    #[case::reversed_extremes(PyStyleIdxRange::new(i64::MAX, i64::MIN))]
    fn resolve_for_split_empty_halves_never_panic(#[case] range: PyStyleIdxRange) {
        let empty: [&str; 0] = [];
        for (start, end) in [(&empty[..], &END[..]), (&START[..], &empty[..]), (&empty, &empty)] {
            let ranges = range.resolve_for_split(start, end);
            let _ = (&start[ranges.start], &end[ranges.end]);
        }
    }

    #[rstest]
    fn resolve_for_split_matches_resolve_for_when_nothing_is_missing() {
        // With an empty tail there is no gap on the right, so every non-negative request must
        // agree with the unsplit resolution over the same elements.
        let empty: [&str; 0] = [];
        for start in 0..4i64 {
            for end in 0..4i64 {
                let range = PyStyleIdxRange::new(start, end);
                let ranges = range.resolve_for_split(&START, &empty);
                assert_eq!(ranges.start, range.resolve_for(&START), "{start}..={end}");
                assert!(ranges.end.is_empty(), "{start}..={end}");
            }
        }
    }

    proptest! {
        /// The same invariant `resolve_for` upholds, across the pair: whatever bounds and whatever
        /// the two halves look like, both ranges can index their slice without panicking.
        #[test]
        fn resolve_for_split_is_always_sliceable(
            start_bound in -20i64..20,
            end_bound in -20i64..20,
            start_len in 0usize..6,
            end_len in 0usize..6,
        ) {
            let start = vec![(); start_len];
            let end = vec![(); end_len];
            let ranges = PyStyleIdxRange::new(start_bound, end_bound).resolve_for_split(&start, &end);
            let _ = &start[ranges.start.clone()];
            let _ = &end[ranges.end.clone()];
            prop_assert!(ranges.start.end <= start_len);
            prop_assert!(ranges.end.end <= end_len);
        }

        /// A request that picks something out of the kept tail must never also claim that tail was
        /// unreachable, and one that reaches past a kept half must always report the gap.
        #[test]
        fn resolve_for_split_reports_the_gap_when_a_bound_overshoots(
            start_bound in -20i64..20,
            end_bound in -20i64..20,
        ) {
            let start = vec![(); 3];
            let end = vec![(); 3];
            let range = PyStyleIdxRange::new(start_bound, end_bound);
            let ranges = range.resolve_for_split(&start, &end);

            // A non-negative range asking beyond the kept start has run into the gap.
            if start_bound >= 0 && end_bound >= 0 && start_bound <= end_bound {
                prop_assert_eq!(ranges.truncated, end_bound + 1 > 3);
            }
            // So has a negative one asking beyond the kept tail.
            if start_bound < 0 && end_bound < 0 && start_bound <= end_bound {
                prop_assert_eq!(ranges.truncated, start_bound < -3);
            }
        }
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
