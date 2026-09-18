//! Coalescing ranges and building context windows around match positions.
//!
//! [`context_windows`] is the `grep -C` core: given where the matches are, it produces the merged
//! spans of lines to show. [`merge_ranges`] is the interval-union primitive it is built on.

use std::ops::Range;

/// Coalesce ascending ranges that overlap or touch into a minimal set of disjoint ranges.
///
/// The input must be sorted by `start`. Half-open ranges that merely touch (`a..b` then `b..c`)
/// fuse into `a..c`, and a range wholly contained in the previous one is absorbed. An empty input
/// yields an empty `Vec`.
pub fn merge_ranges<T: Ord + Copy>(ranges: impl IntoIterator<Item = Range<T>>) -> Vec<Range<T>> {
    ranges.into_iter().fold(Vec::new(), |mut merged, r| {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
        merged
    })
}

/// The merged neighbourhoods of `radius` elements on each side of every hit, clamped to `0..len`.
///
/// `hits` must be ascending and each `< len`. Hit `i` contributes `i - radius ..= i + radius`
/// (saturating at both ends), then windows that overlap or touch are merged. With no hits the
/// result is empty.
pub fn context_windows(
    len: usize,
    hits: impl IntoIterator<Item = usize>,
    radius: usize,
) -> Vec<Range<usize>> {
    merge_ranges(
        hits.into_iter()
            .map(|i| i.saturating_sub(radius)..i.saturating_add(radius).saturating_add(1).min(len)),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::empty(vec![], vec![])]
    #[case::single(vec![2..5], vec![2..5])]
    #[case::disjoint_kept(vec![0..2, 4..6], vec![0..2, 4..6])]
    #[case::touching_fuse(vec![0..2, 2..4], vec![0..4])]
    #[case::overlapping_fuse(vec![0..3, 2..5], vec![0..5])]
    #[case::nested_absorbed(vec![0..6, 2..4], vec![0..6])]
    #[case::chain(vec![0..2, 2..4, 4..6, 8..9], vec![0..6, 8..9])]
    fn merge_ranges_coalesces_overlap_and_touch(
        #[case] input: Vec<Range<usize>>,
        #[case] expected: Vec<Range<usize>>,
    ) {
        assert_eq!(merge_ranges(input), expected);
    }

    #[rstest]
    #[case::no_hits(5, vec![], 1, vec![])]
    #[case::middle(5, vec![2], 1, vec![1..4])]
    #[case::clamps_at_start(3, vec![0], 1, vec![0..2])]
    #[case::clamps_at_end(3, vec![2], 1, vec![1..3])]
    #[case::radius_zero_is_the_hit_line(5, vec![2], 0, vec![2..3])]
    #[case::adjacent_windows_merge(5, vec![0, 2], 1, vec![0..4])]
    #[case::distant_windows_leave_a_gap(6, vec![0, 4], 1, vec![0..2, 3..6])]
    fn context_windows_are_merged_clamped_neighbourhoods(
        #[case] len: usize,
        #[case] hits: Vec<usize>,
        #[case] radius: usize,
        #[case] expected: Vec<Range<usize>>,
    ) {
        assert_eq!(context_windows(len, hits, radius), expected);
    }

    proptest! {
        /// The union primitive's contract: whatever ranges go in (sorted by start), the output
        /// covers exactly the same integer points, in ascending, disjoint, non-touching runs.
        #[test]
        fn merge_ranges_preserves_the_covered_points_as_disjoint_runs(
            input in prop::collection::vec((0u32..20, 0u32..20), 0..12),
        ) {
            let ranges: Vec<Range<u32>> = {
                let mut ranges: Vec<Range<u32>> =
                    input.into_iter().map(|(a, b)| a.min(b)..a.max(b)).collect();
                ranges.sort_unstable_by_key(|r| r.start);
                ranges
            };
            let covered: BTreeSet<u32> = ranges.iter().flat_map(|r| r.clone()).collect();

            let merged = merge_ranges(ranges);

            let recovered: BTreeSet<u32> = merged.iter().flat_map(|r| r.clone()).collect();
            prop_assert_eq!(recovered, covered);
            for pair in merged.windows(2) {
                // Ascending and with a real gap between runs -- touching runs would have fused.
                prop_assert!(pair[0].end < pair[1].start);
            }
        }
    }
}
