use std::num::NonZeroUsize;
use std::pin::Pin;

use futures::{Stream, StreamExt, stream};

mod chunked;

pub use chunked::ChunkedStream;

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
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;
    use std::task::{Context, Poll};

    use futures::executor::block_on;
    use futures::{Stream, StreamExt, stream};
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

    fn poll_now<S: Stream + Unpin>(s: &mut S) -> Poll<Option<S::Item>> {
        let w = futures::task::noop_waker();
        let mut cx = Context::from_waker(&w);
        Pin::new(s).poll_next(&mut cx)
    }

    fn scheduled_items(
        items: Vec<i32>,
        schedule: Vec<bool>,
    ) -> impl Stream<Item = i32> + Send + Unpin {
        enum Step {
            Pending,
            Yield(i32),
        }
        let mut sched = schedule.into_iter();
        let mut steps = std::collections::VecDeque::new();
        for item in items {
            if sched.next().unwrap_or(false) {
                steps.push_back(Step::Pending);
            }
            steps.push_back(Step::Yield(item));
        }
        stream::poll_fn(move |cx| match steps.pop_front() {
            Some(Step::Pending) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Some(Step::Yield(v)) => Poll::Ready(Some(v)),
            None => Poll::Ready(None),
        })
    }

    #[rstest]
    #[case::empty(vec![], 3, vec![])]
    #[case::single(vec![5], 3, vec![vec![5]])]
    #[case::max_one_splits_every_item(vec![1, 1, 2], 1, vec![vec![1], vec![1], vec![2]])]
    #[case::alternating_keys(vec![1, 2, 1], 5, vec![vec![1], vec![2], vec![1]])]
    #[case::run_exactly_max(vec![1, 1], 2, vec![vec![1, 1]])]
    #[case::run_longer_than_max(vec![1, 1, 1, 1, 1], 2, vec![vec![1, 1], vec![1, 1], vec![1]])]
    #[case::mixed(vec![1, 1, 1, 2, 2, 3], 5, vec![vec![1, 1, 1], vec![2, 2], vec![3]])]
    #[case::split_run_then_key_change(vec![1, 1, 1, 2], 2, vec![vec![1, 1], vec![1], vec![2]])]
    #[case::huge_max(vec![1, 1, 1], usize::MAX, vec![vec![1, 1, 1]])]
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

    #[test]
    fn split_run_stays_grouped_by_key() {
        let chunks: Vec<(i32, Vec<i32>)> = block_on(
            chunk_by_bounded(stream::iter([2, 4, 6]), NonZeroUsize::new(2).unwrap(), |x| x % 2)
                .collect(),
        );
        assert_eq!(chunks, vec![(0, vec![2, 4]), (0, vec![6])]);
    }

    #[rstest]
    #[case(
        vec!["a", "b", "cc", "dd", "e"],
        5,
        vec![(1, vec!["a", "b"]), (2, vec!["cc", "dd"]), (1, vec!["e"])],
    )]
    #[case(vec!["aa", "bb", "cc"], 2, vec![(2, vec!["aa", "bb"]), (2, vec!["cc"])])]
    fn groups_owned_items_by_projected_key(
        #[case] items: Vec<&str>,
        #[case] max: usize,
        #[case] expected: Vec<(usize, Vec<&str>)>,
    ) {
        let owned: Vec<String> = items.into_iter().map(String::from).collect();
        let chunks: Vec<(usize, Vec<String>)> = block_on(
            chunk_by_bounded(stream::iter(owned), NonZeroUsize::new(max).unwrap(), |s: &String| {
                s.len()
            })
            .collect(),
        );
        let expected: Vec<(usize, Vec<String>)> = expected
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().map(String::from).collect()))
            .collect();
        assert_eq!(chunks, expected);
    }

    #[test]
    fn sub_max_run_withheld_until_a_differing_key_is_peeked() {
        let (tx, rx) = futures::channel::mpsc::unbounded::<i32>();
        let mut s = Box::pin(chunk_by_bounded(rx, NonZeroUsize::new(3).unwrap(), |x| *x));

        tx.unbounded_send(1).unwrap();
        tx.unbounded_send(1).unwrap();
        assert!(poll_now(&mut s).is_pending());

        tx.unbounded_send(2).unwrap();
        assert_eq!(poll_now(&mut s), Poll::Ready(Some((1, vec![1, 1]))));

        assert!(poll_now(&mut s).is_pending());
    }

    #[test]
    fn full_chunk_emits_without_peeking_past_the_bound() {
        let (tx, rx) = futures::channel::mpsc::unbounded::<i32>();
        let mut s = Box::pin(chunk_by_bounded(rx, NonZeroUsize::new(2).unwrap(), |x| *x));

        tx.unbounded_send(1).unwrap();
        tx.unbounded_send(1).unwrap();
        assert_eq!(poll_now(&mut s), Poll::Ready(Some((1, vec![1, 1]))));
    }

    #[test]
    fn pending_then_closed_empty_source_yields_nothing() {
        let stage = AtomicUsize::new(0);
        let src = stream::poll_fn(move |cx| {
            if stage.swap(1, SeqCst) == 0 {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(None)
            }
        });
        let chunks: Vec<(i32, Vec<i32>)> =
            block_on(chunk_by_bounded(src, NonZeroUsize::new(3).unwrap(), |x| *x).collect());
        assert_eq!(chunks, Vec::<(i32, Vec<i32>)>::new());
    }

    #[test]
    fn key_panic_surfaces_on_the_poll_that_peeks_the_offending_item() {
        let mut s = Box::pin(chunk_by_bounded(
            stream::iter([1, 2, 3]),
            NonZeroUsize::new(5).unwrap(),
            |x| {
                if *x == 3 {
                    panic!("boom")
                } else {
                    *x
                }
            },
        ));
        assert_eq!(poll_now(&mut s), Poll::Ready(Some((1, vec![1]))));
        assert!(catch_unwind(AssertUnwindSafe(|| poll_now(&mut s))).is_err());
    }

    #[rstest]
    #[case(vec![1], 5, 2)]
    #[case(vec![1, 1], 5, 3)]
    #[case(vec![1, 2], 5, 5)]
    #[case(vec![1, 1], 1, 4)]
    fn key_is_invoked_per_peek_and_per_chunk_target(
        #[case] items: Vec<i32>,
        #[case] max: usize,
        #[case] expected_calls: usize,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        block_on(
            chunk_by_bounded(
                stream::iter(items),
                NonZeroUsize::new(max).unwrap(),
                move |x: &i32| {
                    c.fetch_add(1, SeqCst);
                    *x
                },
            )
            .collect::<Vec<(i32, Vec<i32>)>>(),
        );
        assert_eq!(calls.load(SeqCst), expected_calls);
    }

    #[test]
    fn items_are_borrowed_for_keying_and_moved_into_chunks_never_cloned() {
        #[derive(Debug, PartialEq)]
        struct Loud(u8);
        impl Clone for Loud {
            fn clone(&self) -> Self {
                panic!("must not clone")
            }
        }
        let chunks: Vec<(u8, Vec<Loud>)> = block_on(
            chunk_by_bounded(
                stream::iter([Loud(1), Loud(1), Loud(2)]),
                NonZeroUsize::new(5).unwrap(),
                |l: &Loud| l.0,
            )
            .collect(),
        );
        assert_eq!(chunks, vec![(1, vec![Loud(1), Loud(1)]), (2, vec![Loud(2)])]);
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

    proptest! {
        #[test]
        fn key_tagged_invariants(
            items in prop::collection::vec((0u8..3, any::<u8>()), 0..50),
            max in 1usize..8,
        ) {
            let size = NonZeroUsize::new(max).expect("proptest max is non-zero");
            let chunks: Vec<(u8, Vec<(u8, u8)>)> =
                block_on(chunk_by_bounded(stream::iter(items.clone()), size, |&(t, _)| t).collect());

            let flat: Vec<(u8, u8)> = chunks.iter().flat_map(|(_, c)| c.iter().copied()).collect();
            prop_assert_eq!(&flat, &items);

            for (k, c) in &chunks {
                prop_assert!(!c.is_empty());
                prop_assert!(c.len() <= max);
                prop_assert!(c.iter().all(|(t, _)| t == k));
            }

            for pair in chunks.windows(2) {
                if pair[0].1.len() < max {
                    prop_assert_ne!(pair[0].0, pair[1].0);
                }
            }
        }

        #[test]
        fn grouping_is_invariant_to_upstream_readiness(
            items in prop::collection::vec(0i32..4, 0..50),
            max in 1usize..8,
            schedule in prop::collection::vec(any::<bool>(), 0..60),
        ) {
            let size = NonZeroUsize::new(max).expect("proptest max is non-zero");
            let scheduled: Vec<Vec<i32>> = block_on(
                chunk_by_bounded(scheduled_items(items.clone(), schedule), size, |x| *x)
                    .map(|(_, c)| c)
                    .collect(),
            );
            prop_assert_eq!(scheduled, chunks_of(items, max));
        }
    }
}
