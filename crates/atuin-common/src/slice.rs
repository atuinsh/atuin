/// Reimplementation of the standard library's `slice::partition_dedup`, which is currently
/// unstable.
// TODO: Replace this with the standard library implementation when it's stabilized.
#[must_use]
pub fn partition_dedup<T>(slice: &mut [T]) -> (&mut [T], &mut [T])
where
    T: PartialEq,
{
    if slice.len() <= 1 {
        return (slice, &mut []);
    }

    let mut next_read: usize = 1;
    let mut next_write: usize = 1;

    while next_read < slice.len() {
        if slice[next_read] != slice[next_write - 1] {
            slice.swap(next_read, next_write);
            next_write += 1;
        }
        next_read += 1;
    }
    slice.split_at_mut(next_write)
}

/// Compares a sorted slice that has no duplicates with an iterator, checking whether both contain
/// the same set of items.
///
/// The iterator may have duplicates and its items can be in any order.
///
/// Why is this a struct? We want [`Self::eq`] to take a const generic parameter that controls the
/// size of the internal stack-allocated buffer, but if this were a free function, you would have to
/// specify all the generic parameters. This way, the parameters on the struct can be inferred, and
/// you only have to specify `STACK_SIZE`.
pub struct SortedDedupedSliceComparer<'a, T, I> {
    slice: &'a [T],
    iter: I,
}

impl<'a, T, I, B> SortedDedupedSliceComparer<'a, T, I>
where
    I: IntoIterator<Item = &'a B>,
    B: Ord + ?Sized + 'a,
    T: std::borrow::Borrow<B>,
{
    /// Create a new [`SortedDedupedSliceComparer`].
    ///
    /// `sorted` must be sorted and contain no duplicates.
    pub fn new(sorted: &'a [T], iter: I) -> Self {
        debug_assert!(sorted.is_sorted_by_key(|s| s.borrow()), "`sorted` must be sorted");
        debug_assert!(
            sorted.windows(2).all(|w| w[0].borrow() != w[1].borrow()),
            "`sorted` must not contain duplicates",
        );
        Self {
            slice: sorted,
            iter,
        }
    }

    /// Check whether the elements of the iterator exactly equal the elements of the sorted slice,
    /// without regard to order or duplicates (set equality).
    ///
    /// If the length of the slice is less than or equal to `STACK_SIZE`, this function will not
    /// allocate memory.
    pub fn eq<const STACK_SIZE: usize>(self) -> bool {
        self.eq_with_buffer(&mut [false; STACK_SIZE])
    }

    // This is a separate function from `eq` so most of the code doesn't get monomorphized for every
    // value of `STACK_SIZE`.
    fn eq_with_buffer(self, buffer: &mut [bool]) -> bool {
        let mut seen_heap;
        let seen = if self.slice.len() <= buffer.len() {
            &mut buffer[..self.slice.len()]
        } else {
            seen_heap = vec![false; self.slice.len()];
            seen_heap.as_mut_slice()
        };
        for item in self.iter {
            match self.slice.binary_search_by_key(&item, |s| s.borrow()) {
                Ok(pos) => seen[pos] = true,
                Err(_) => return false,
            }
        }
        seen.iter().all(|b| *b)
    }
}

/// The indices and elements to show around every hit in `items`.
///
/// A hit is an element for which `is_hit` holds. With `radius` `None`, every element is shown; with
/// `Some(r)`, only each hit's neighbourhood of `r` elements on either side, overlapping
/// neighbourhoods merged (see [`context_windows`](crate::range::context_windows)). The yielded
/// indices are ascending and never repeat.
pub fn excerpt<T>(
    items: &[T],
    is_hit: impl Fn(&T) -> bool,
    radius: Option<usize>,
) -> impl Iterator<Item = (usize, &T)> {
    use crate::range::context_windows;

    let everything = 0..items.len();
    let windows = match radius {
        None => vec![everything],
        Some(radius) => {
            let hits = items.iter().enumerate().filter(|(_, item)| is_hit(item)).map(|(i, _)| i);
            context_windows(items.len(), hits, radius)
        }
    };
    windows.into_iter().flatten().map(move |i| (i, &items[i]))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn excerpted(items: &[u32], hit: u32, radius: Option<usize>) -> Vec<(usize, u32)> {
        excerpt(items, |x| *x == hit, radius).map(|(i, x)| (i, *x)).collect()
    }

    #[rstest]
    #[case::none_shows_everything(&[10, 20, 30], 99, None, vec![(0, 10), (1, 20), (2, 30)])]
    #[case::none_on_empty(&[], 99, None, vec![])]
    #[case::some_without_hits_is_empty(&[1, 2, 3], 9, Some(1), vec![])]
    #[case::some_windows_the_hit(&[0, 0, 1, 0, 0], 1, Some(1), vec![(1, 0), (2, 1), (3, 0)])]
    #[case::some_merges_adjacent(&[9, 0, 9, 0, 0], 9, Some(1), vec![(0, 9), (1, 0), (2, 9), (3, 0)])]
    #[case::radius_zero_is_just_the_hits(&[9, 0, 9], 9, Some(0), vec![(0, 9), (2, 9)])]
    fn excerpt_selects_context_around_hits(
        #[case] items: &[u32],
        #[case] hit: u32,
        #[case] radius: Option<usize>,
        #[case] expected: Vec<(usize, u32)>,
    ) {
        assert_eq!(excerpted(items, hit, radius), expected);
    }

    #[rstest]
    #[case(&[], &[], &[])]
    #[case(&[1], &[1], &[])]
    #[case(&[1, 2], &[1, 2], &[])]
    #[case(&[1, 1], &[1], &[1])]
    #[case(&[1, 1, 1], &[1], &[1, 1])]
    #[case(&[1, 1, 2], &[1, 2], &[1])]
    #[case(&[1, 2, 2], &[1, 2], &[2])]
    #[case(&[1, 1, 2, 2, 3], &[1, 2, 3], &[1, 2])]
    #[case(&[1, 2, 3, 3, 3, 3], &[1, 2, 3], &[3, 3, 3])]
    // Only *consecutive* duplicates are removed.
    #[case(&[1, 2, 1], &[1, 2, 1], &[])]
    #[case(&[7, 7, 0, 7], &[7, 0, 7], &[7])]
    fn test_partition_dedup(
        #[case] input: &[u32],
        #[case] expected_dedup: &[u32],
        #[case] expected_rest: &[u32],
    ) {
        let mut input = input.to_vec();
        let (dedup, rest) = partition_dedup(&mut input);
        assert_eq!(dedup, expected_dedup);
        // The order of the duplicates is unspecified, so compare them as a multiset.
        let mut rest = rest.to_vec();
        rest.sort_unstable();
        assert_eq!(rest, expected_rest);
    }

    #[rstest]
    #[case(&[], &[], true)]
    #[case(&[], &[1], false)]
    #[case(&[1], &[], false)]
    #[case(&[1], &[1], true)]
    #[case(&[1], &[2], false)]
    #[case(&[1, 1], &[1], true)]
    #[case(&[1, 2], &[1], false)]
    #[case(&[1], &[1, 2], false)]
    #[case(&[2, 1], &[1, 2], true)]
    #[case(&[2, 1, 1, 2, 2], &[1, 2], true)]
    #[case(&[2, 3, 1, 2], &[1, 2, 3], true)]
    #[case(&[2, 3, 1, 2], &[1, 2], false)]
    #[case(&[2, 3, 1, 2], &[1, 2, 3, 4], false)]
    #[allow(
        clippy::used_underscore_binding,
        reason = "unused _stack_size is used to drive generic N within rstest"
    )]
    fn test_sorted_deduped_slice_comparer<const STACK_SIZE: usize>(
        #[case] iter: &[u32],
        #[case] sorted: &[u32],
        #[case] expected: bool,
        #[values(
            [(); 0],
            [(); 1],
            [(); 2],
            [(); 3],
            [(); 4],
            [(); 5],
        )]
        _stack_size: [(); STACK_SIZE],
    ) {
        assert_eq!(SortedDedupedSliceComparer::new(sorted, iter).eq::<STACK_SIZE>(), expected,);
    }

    fn sorted_deduped_slice() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(0u8..16, 0..16).prop_map(|mut items| {
            items.sort_unstable();
            items.dedup();
            items
        })
    }

    proptest! {
        #[rstest]
        fn partition_dedup_matches_vec_dedup(input in prop::collection::vec(0u8..4, 0..32)) {
            let mut actual = input.clone();
            let (dedup, _) = partition_dedup(&mut actual);
            let mut expected = input;
            expected.dedup();
            prop_assert_eq!(dedup.to_vec(), expected);
        }

        #[rstest]
        fn partition_dedup_leaves_no_consecutive_duplicates(
            input in prop::collection::vec(0u8..4, 0..32),
        ) {
            let mut input = input;
            let (dedup, _) = partition_dedup(&mut input);
            prop_assert!(dedup.windows(2).all(|w| w[0] != w[1]));
        }

        /// Nothing is added or removed: the two partitions together are a permutation of the input.
        #[rstest]
        fn partition_dedup_permutes_the_input(input in prop::collection::vec(0u8..4, 0..32)) {
            let mut actual = input.clone();
            {
                let (dedup, rest) = partition_dedup(&mut actual);
                prop_assert_eq!(dedup.len() + rest.len(), input.len());
            }
            actual.sort_unstable();
            let mut expected = input;
            expected.sort_unstable();
            prop_assert_eq!(actual, expected);
        }

        #[rstest]
        fn comparer_matches_set_equality(
            slice in sorted_deduped_slice(),
            iter in prop::collection::vec(0u8..16, 0..16),
        ) {
            let expected = iter.iter().copied().collect::<BTreeSet<_>>()
                == slice.iter().copied().collect::<BTreeSet<_>>();
            prop_assert_eq!(
                SortedDedupedSliceComparer::new(&slice, &iter).eq::<8>(),
                expected,
            );
        }

        #[rstest]
        fn comparer_ignores_the_stack_size(
            slice in sorted_deduped_slice(),
            iter in prop::collection::vec(0u8..16, 0..16),
        ) {
            let expected = SortedDedupedSliceComparer::new(&slice, &iter).eq::<8>();
            prop_assert_eq!(SortedDedupedSliceComparer::new(&slice, &iter).eq::<0>(), expected);
            prop_assert_eq!(SortedDedupedSliceComparer::new(&slice, &iter).eq::<1>(), expected);
            prop_assert_eq!(SortedDedupedSliceComparer::new(&slice, &iter).eq::<64>(), expected);
        }

        #[rstest]
        fn comparer_ignores_order_and_duplicates(
            slice in sorted_deduped_slice(),
            iter in prop::collection::vec(0u8..16, 0..16),
        ) {
            let expected = SortedDedupedSliceComparer::new(&slice, &iter).eq::<8>();
            let mut shuffled = iter.clone();
            shuffled.reverse();
            shuffled.extend(iter);
            prop_assert_eq!(
                SortedDedupedSliceComparer::new(&slice, &shuffled).eq::<8>(),
                expected,
            );
        }

        #[rstest]
        fn comparer_is_reflexive(slice in sorted_deduped_slice()) {
            prop_assert!(SortedDedupedSliceComparer::new(&slice, &slice).eq::<8>());

            let mut extra = slice.clone();
            extra.push(u8::MAX);
            prop_assert!(!SortedDedupedSliceComparer::new(&slice, &extra).eq::<8>());
        }
    }
}
