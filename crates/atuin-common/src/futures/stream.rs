use std::num::NonZeroUsize;
use std::pin::Pin;

use futures::{Stream, StreamExt, stream};

/// Group adjacent stream items that share a key into chunks of at most `max` items.
///
/// Like itertools' `chunk_by`, but a run longer than `max` is split into several chunks instead
/// of one unbounded one.
///
/// # Examples
///
/// ```
/// use std::num::NonZeroUsize;
///
/// use atuin_common::futures::stream::chunk_by_bounded;
/// use futures::{StreamExt, executor::block_on, stream};
///
/// // each chunk is paired with its key; `max` is large enough that it never splits a run
/// let chunks: Vec<(i32, Vec<i32>)> = block_on(
///     chunk_by_bounded(stream::iter([1, 1, 1, 2, 2, 3]), NonZeroUsize::new(5).unwrap(), |x| *x)
///         .collect(),
/// );
/// assert_eq!(chunks, vec![(1, vec![1, 1, 1]), (2, vec![2, 2]), (3, vec![3])]);
///
/// // a run longer than `max` is split at the bound, so its key repeats
/// let chunks: Vec<(i32, Vec<i32>)> = block_on(
///     chunk_by_bounded(stream::iter([1, 1, 1, 1]), NonZeroUsize::new(2).unwrap(), |x| *x).collect(),
/// );
/// assert_eq!(chunks, vec![(1, vec![1, 1]), (1, vec![1, 1])]);
/// ```
pub fn chunk_by_bounded<S, K, F>(
    stream: S,
    max: NonZeroUsize,
    key: F,
) -> impl Stream<Item = (K, Vec<S::Item>)>
where
    S: Stream + Unpin,
    K: PartialEq,
    F: FnMut(&S::Item) -> K,
{
    let max = max.get();
    stream::unfold((stream.peekable(), key), move |(mut stream, mut key)| async move {
        let target = key(Pin::new(&mut stream).peek().await?);

        let mut chunk = Vec::new();
        while chunk.len() < max {
            match Pin::new(&mut stream).peek().await {
                Some(item) if key(item) == target => {}
                _ => break,
            }

            // The peek above returned `Some`, so `next` does too.
            chunk.push(stream.next().await.expect("peeked item is present"));
        }

        Some(((target, chunk), (stream, key)))
    })
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use futures::executor::block_on;
    use futures::{StreamExt, stream};
    use proptest::prelude::*;
    use rstest::rstest;

    use super::chunk_by_bounded;

    /// Run the combinator over an in-memory sequence, grouping by value equality, and drop the
    /// per-chunk key so the assertions read as plain chunk shapes.
    fn chunks_of(items: Vec<i32>, max: usize) -> Vec<Vec<i32>> {
        let max = NonZeroUsize::new(max).expect("test max is non-zero");
        block_on(
            chunk_by_bounded(stream::iter(items), max, |x| *x).map(|(_, chunk)| chunk).collect(),
        )
    }

    #[rstest]
    #[case::empty(vec![], 3, vec![])]
    #[case::single(vec![5], 3, vec![vec![5]])]
    #[case::max_one_splits_every_item(vec![1, 1, 2], 1, vec![vec![1], vec![1], vec![2]])]
    #[case::alternating_keys(vec![1, 2, 1], 5, vec![vec![1], vec![2], vec![1]])]
    #[case::run_exactly_max(vec![1, 1], 2, vec![vec![1, 1]])]
    #[case::run_longer_than_max(vec![1, 1, 1, 1, 1], 2, vec![vec![1, 1], vec![1, 1], vec![1]])]
    #[case::mixed(vec![1, 1, 1, 2, 2, 3], 5, vec![vec![1, 1, 1], vec![2, 2], vec![3]])]
    fn chunks_match_expected(
        #[case] items: Vec<i32>,
        #[case] max: usize,
        #[case] expected: Vec<Vec<i32>>,
    ) {
        assert_eq!(chunks_of(items, max), expected);
    }

    #[test]
    fn groups_by_key_not_value() {
        // The key collapses values into residue classes; equal residues chunk together, and each
        // chunk is tagged with that residue.
        let chunks: Vec<(i32, Vec<i32>)> = block_on(
            chunk_by_bounded(stream::iter([2, 4, 3, 6, 5]), NonZeroUsize::new(5).unwrap(), |x| {
                x % 2
            })
            .collect(),
        );

        assert_eq!(chunks, vec![(0, vec![2, 4]), (1, vec![3]), (0, vec![6]), (1, vec![5])]);
    }

    proptest! {
        /// The four properties that fully characterise `chunk_by_bounded`, over arbitrary input.
        /// The small value domain makes runs of equal keys common.
        #[test]
        fn holds_invariants(items in prop::collection::vec(0i32..4, 0..50), max in 1usize..8) {
            let chunks = chunks_of(items.clone(), max);

            // 1. Flattening the chunks restores the original sequence, in order.
            let flat: Vec<i32> = chunks.iter().flatten().copied().collect();
            prop_assert_eq!(&flat, &items);

            for chunk in &chunks {
                // 2. Every chunk is non-empty and within the bound.
                prop_assert!(!chunk.is_empty());
                prop_assert!(chunk.len() <= max);
                // 3. Each chunk is homogeneous (one key; here, one value).
                prop_assert!(chunk.iter().all(|x| x == &chunk[0]));
            }

            // 4. A chunk ends below `max` only because the key changed - never arbitrarily.
            for pair in chunks.windows(2) {
                if pair[0].len() < max {
                    prop_assert_ne!(pair[0].last().unwrap(), pair[1].first().unwrap());
                }
            }
        }
    }
}
