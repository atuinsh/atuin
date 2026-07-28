/// Reimplementation of the standard library's `slice::partition_dedup`, which is currently
/// unstable.
// TODO: Replace this with the standard library implementation when it's stabilized.
pub fn partition_dedup<'a, T>(slice: &'a mut [T]) -> (&'a mut [T], &'a mut [T])
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
        debug_assert!(
            sorted.is_sorted_by_key(|s| s.borrow()),
            "`sorted` must be sorted",
        );
        debug_assert_eq!(
            {
                let mut vec = sorted.iter().collect::<Vec<_>>();
                vec.dedup_by_key(|s| s.borrow());
                vec.len()
            },
            sorted.len(),
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
        let seen;
        if self.slice.len() <= buffer.len() {
            seen = &mut buffer[..self.slice.len()];
        } else {
            seen_heap = vec![false; self.slice.len()];
            seen = seen_heap.as_mut_slice();
        }
        for item in self.iter {
            match self.slice.binary_search_by_key(&item, |s| s.borrow()) {
                Ok(pos) => seen[pos] = true,
                Err(_) => return false,
            }
        }
        seen.iter().all(|b| *b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

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
        assert_eq!(
            SortedDedupedSliceComparer::new(sorted, iter).eq::<STACK_SIZE>(),
            expected,
        );
    }
}
