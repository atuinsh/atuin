use std::cmp::Ordering;
use std::pin::pin;

use async_stream::try_stream;
use futures::{Stream, TryStreamExt};
pub use itertools::EitherOrBoth;

/// Merge two ascending fallible streams into their [`EitherOrBoth`] outer join - the async
/// streaming analog of [`itertools::merge_join_by`].
///
/// # Examples
///
/// ```
/// use atuin_common::futures::stream::{EitherOrBoth, try_merge_join_by};
/// use futures::{StreamExt, TryStreamExt, executor::block_on, stream};
///
/// let left = stream::iter([1, 3, 4]).map(Ok::<_, ()>);
/// let right = stream::iter([2, 3]).map(Ok::<_, ()>);
///
/// let merged: Vec<EitherOrBoth<i32, i32>> =
///     block_on(try_merge_join_by(left, right, i32::cmp).try_collect()).unwrap();
///
/// assert_eq!(
///     merged,
///     vec![
///         EitherOrBoth::Left(1),
///         EitherOrBoth::Right(2),
///         EitherOrBoth::Both(3, 3),
///         EitherOrBoth::Left(4),
///     ],
/// );
/// ```
pub fn try_merge_join_by<A, B, L, R, E, F>(
    left: A,
    right: B,
    mut cmp: F,
) -> impl Stream<Item = Result<EitherOrBoth<L, R>, E>>
where
    A: Stream<Item = Result<L, E>>,
    B: Stream<Item = Result<R, E>>,
    F: FnMut(&L, &R) -> Ordering,
{
    try_stream! {
        let mut left = pin!(left);
        let mut right = pin!(right);

        let mut l = left.try_next().await?;
        let mut r = right.try_next().await?;

        loop {
            let ordering = match (l.as_ref(), r.as_ref()) {
                (Some(a), Some(b)) => cmp(a, b),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => break,
            };

            match ordering {
                Ordering::Less => {
                    yield EitherOrBoth::Left(l.take().expect("left present on Less"));
                    l = left.try_next().await?;
                }
                Ordering::Greater => {
                    yield EitherOrBoth::Right(r.take().expect("right present on Greater"));
                    r = right.try_next().await?;
                }
                Ordering::Equal => {
                    let a = l.take().expect("left present on Equal");
                    let b = r.take().expect("right present on Equal");
                    yield EitherOrBoth::Both(a, b);
                    l = left.try_next().await?;
                    r = right.try_next().await?;
                }
            }
        }
    }
}

/// [`try_merge_join_by`] over a shared item type, using its natural [`Ord`] order.
pub fn try_merge_join<A, B, T, E>(
    left: A,
    right: B,
) -> impl Stream<Item = Result<EitherOrBoth<T, T>, E>>
where
    A: Stream<Item = Result<T, E>>,
    B: Stream<Item = Result<T, E>>,
    T: Ord,
{
    try_merge_join_by(left, right, T::cmp)
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use futures::{Stream, StreamExt, TryStreamExt, stream};
    use proptest::prelude::*;
    use rstest::rstest;

    use super::EitherOrBoth::{Both, Left, Right};
    use super::{EitherOrBoth, try_merge_join, try_merge_join_by};

    fn ok_stream(items: Vec<i32>) -> impl Stream<Item = Result<i32, ()>> {
        stream::iter(items).map(Ok)
    }

    fn merged(a: Vec<i32>, b: Vec<i32>) -> Vec<EitherOrBoth<i32, i32>> {
        block_on(try_merge_join(ok_stream(a), ok_stream(b)).try_collect::<Vec<_>>())
            .expect("no error")
    }

    #[rstest]
    #[case(vec![], vec![], vec![])]
    #[case(vec![1, 2, 3], vec![], vec![Left(1), Left(2), Left(3)])]
    #[case(vec![], vec![1, 2], vec![Right(1), Right(2)])]
    #[case(vec![1, 3], vec![2, 4], vec![Left(1), Right(2), Left(3), Right(4)])]
    #[case(vec![1, 2], vec![2, 3], vec![Left(1), Both(2, 2), Right(3)])]
    #[case(vec![1, 1, 2], vec![1, 2, 2], vec![Both(1, 1), Left(1), Both(2, 2), Right(2)])]
    fn merges_two_sorted_streams(
        #[case] a: Vec<i32>,
        #[case] b: Vec<i32>,
        #[case] expected: Vec<EitherOrBoth<i32, i32>>,
    ) {
        assert_eq!(merged(a, b), expected);
    }

    // Each case carries exactly one error, so the merge must surface it whichever side and
    // position it sits at.
    #[rstest]
    #[case::left_first(vec![Err("e")], vec![Ok(1)])]
    #[case::left_mid(vec![Ok(1), Err("e"), Ok(9)], vec![Ok(2)])]
    #[case::right_first(vec![Ok(1)], vec![Err("e")])]
    #[case::right_mid(vec![Ok(1)], vec![Ok(2), Err("e"), Ok(9)])]
    fn surfaces_the_first_error(
        #[case] left: Vec<Result<i32, &'static str>>,
        #[case] right: Vec<Result<i32, &'static str>>,
    ) {
        let out: Result<Vec<_>, &str> =
            block_on(try_merge_join(stream::iter(left), stream::iter(right)).try_collect());
        assert_eq!(out, Err("e"));
    }

    #[rstest]
    fn stops_reading_after_the_first_error() {
        // A tail that panics if polled proves the merge short-circuits at the first error rather
        // than draining the rest of the side.
        let left = stream::iter(vec![Ok::<i32, &str>(1), Err("boom")])
            .chain(stream::poll_fn(|_| panic!("polled past the first error")));
        let right = stream::iter(vec![Ok::<i32, &str>(2), Ok(3)]);
        let out: Result<Vec<_>, &str> = block_on(try_merge_join(left, right).try_collect());
        assert_eq!(out, Err("boom"));
    }

    #[rstest]
    fn joins_by_key_across_differing_item_types() {
        // The `_by` variant allows `L != R`: left carries a tag, right is a bare key, joined on key.
        let left = stream::iter(vec![Ok::<_, ()>((1, "a")), Ok((3, "c"))]);
        let right = stream::iter(vec![Ok::<_, ()>(2), Ok(3)]);
        let out: Vec<EitherOrBoth<(i32, &str), i32>> =
            block_on(try_merge_join_by(left, right, |l, r| l.0.cmp(r)).try_collect())
                .expect("no error");
        assert_eq!(out, vec![Left((1, "a")), Right(2), Both((3, "c"), 3)]);
    }

    proptest! {
        #[test]
        fn matches_itertools_merge_join_by(
            a in prop::collection::vec(0i32..6, 0..25),
            b in prop::collection::vec(0i32..6, 0..25),
        ) {
            // Same greedy algorithm as itertools, so we match it on any input, sorted or not.
            let expected: Vec<EitherOrBoth<i32, i32>> =
                itertools::merge_join_by(a.clone(), b.clone(), i32::cmp).collect();
            prop_assert_eq!(merged(a, b), expected);
        }

        #[test]
        fn by_variant_matches_itertools_under_a_custom_order(
            a in prop::collection::vec(0i32..6, 0..25),
            b in prop::collection::vec(0i32..6, 0..25),
        ) {
            // A non-natural (descending) comparator exercises the `_by` path itself; same greedy
            // algorithm as itertools, so we match it on any input.
            let rev = |x: &i32, y: &i32| y.cmp(x);
            let expected: Vec<EitherOrBoth<i32, i32>> =
                itertools::merge_join_by(a.clone(), b.clone(), rev).collect();
            let got: Vec<EitherOrBoth<i32, i32>> =
                block_on(try_merge_join_by(ok_stream(a), ok_stream(b), rev).try_collect())
                    .expect("no error");
            prop_assert_eq!(got, expected);
        }
    }
}
