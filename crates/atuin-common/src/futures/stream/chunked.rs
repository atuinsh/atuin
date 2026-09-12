use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt, TryStreamExt, stream};

/// A chunked stream is a stream that holds [`Vec<T>`] as its items.
///
/// It provides useful helper utilities that allow you to iterate over each element.
///
///   - [`Self::map`], for example, enables you to map over each element of the stream elements, at
///     the scalar level.
///   - [`Self::items`] gives you a new stream, that's a 1-dimensional stream over each element of
///     the stream.
///
/// The convenience this stream enables is to do
///
/// ```
/// let mut stream = ChunkedStream::from_items([[1, 2], [3, 4]])
/// stream.map(|i| i * 2)
///
/// assert_eq(ChunkedStream::from_items([[2, 4], [6, 8]]), stream)
/// ```
#[must_use]
pub struct ChunkedStream<T> {
    inner: Pin<Box<dyn Stream<Item = Vec<T>> + Send>>,
}

impl<T: Send + 'static> ChunkedStream<T> {
    pub fn new(chunks: impl Stream<Item = Vec<T>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(chunks),
        }
    }

    pub fn empty() -> Self {
        Self::from_chunks(std::iter::empty())
    }

    pub fn from_chunks<I>(chunks: I) -> Self
    where
        I: IntoIterator<Item = Vec<T>>,
        I::IntoIter: Send + 'static,
    {
        Self::new(stream::iter(chunks))
    }

    pub fn from_items<I>(items: I, chunk: NonZeroUsize) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: Send + 'static,
    {
        Self::new(stream::iter(items).chunks(chunk.get()))
    }

    pub fn map<U, F>(self, mut f: F) -> ChunkedStream<U>
    where
        U: Send + 'static,
        F: FnMut(T) -> U + Send + 'static,
    {
        ChunkedStream::new(self.inner.map(move |chunk| chunk.into_iter().map(&mut f).collect()))
    }

    pub fn items(self) -> Items<T> {
        Items {
            inner: Box::pin(self.inner.flat_map(stream::iter)),
        }
    }
}

impl<T: Send + 'static, E: Send + 'static> ChunkedStream<Result<T, E>> {
    pub fn from_error(err: E) -> Self {
        Self::from_chunks([vec![Err(err)]])
    }

    pub fn from_fallible_items<I>(items: Result<I, E>, chunk: NonZeroUsize) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: Send + 'static,
    {
        match items {
            Ok(items) => Self::from_items(items.into_iter().map(Ok), chunk),
            Err(err) => Self::from_error(err),
        }
    }

    pub async fn try_collect<C: Default + Extend<T>>(self) -> Result<C, E> {
        self.items().try_collect().await
    }
}

impl<T: Send + 'static> FromIterator<Vec<T>> for ChunkedStream<T> {
    fn from_iter<I: IntoIterator<Item = Vec<T>>>(chunks: I) -> Self {
        #[allow(clippy::needless_collect)]
        Self::from_chunks(chunks.into_iter().collect::<Vec<_>>())
    }
}

impl<T> Stream for ChunkedStream<T> {
    type Item = Vec<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

#[must_use]
pub struct Items<T> {
    inner: Pin<Box<dyn Stream<Item = T> + Send>>,
}

impl<T> Stream for Items<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::num::NonZeroUsize;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::task::{Context, Poll};

    use futures::executor::block_on;
    use futures::task::{ArcWake, waker};
    use futures::{Stream, StreamExt, stream};
    use proptest::prelude::*;
    use rstest::rstest;

    use super::{ChunkedStream, Items};

    const _: fn() = || {
        fn assert_send_unpin<T: Send + Unpin>() {}
        fn assert_send<T: Send>() {}
        assert_send_unpin::<Items<i32>>();
        assert_send::<ChunkedStream<i32>>();
    };

    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test chunk size is non-zero")
    }

    fn poll_now<S: Stream + Unpin>(s: &mut S) -> Poll<Option<S::Item>> {
        let w = futures::task::noop_waker();
        let mut cx = Context::from_waker(&w);
        Pin::new(s).poll_next(&mut cx)
    }

    struct CountWaker(AtomicUsize);

    impl ArcWake for CountWaker {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.0.fetch_add(1, SeqCst);
        }
    }

    fn scheduled(
        chunks: Vec<Vec<i32>>,
        schedule: Vec<bool>,
    ) -> impl Stream<Item = Vec<i32>> + Send {
        enum Step {
            Pending,
            Yield(Vec<i32>),
        }
        let mut sched = schedule.into_iter();
        let mut steps = VecDeque::new();
        for chunk in chunks {
            if sched.next().unwrap_or(false) {
                steps.push_back(Step::Pending);
            }
            steps.push_back(Step::Yield(chunk));
        }
        stream::poll_fn(move |cx| match steps.pop_front() {
            Some(Step::Pending) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Some(Step::Yield(c)) => Poll::Ready(Some(c)),
            None => Poll::Ready(None),
        })
    }

    #[test]
    fn empty_yields_no_chunks_not_one_empty_chunk() {
        assert_eq!(
            block_on(ChunkedStream::<i32>::empty().collect::<Vec<Vec<i32>>>()),
            Vec::<Vec<i32>>::new()
        );
        assert_eq!(
            block_on(ChunkedStream::<i32>::empty().items().collect::<Vec<i32>>()),
            Vec::<i32>::new()
        );
    }

    #[rstest]
    #[case(vec![vec![1, 2], vec![3], vec![4, 5]], vec![1, 2, 3, 4, 5])]
    #[case(vec![vec![1], vec![], vec![2]], vec![1, 2])]
    #[case(vec![vec![], vec![1]], vec![1])]
    #[case(vec![vec![1], vec![]], vec![1])]
    #[case(vec![vec![], vec![]], vec![])]
    fn stream_keeps_empty_chunks_items_flattens_them(
        #[case] chunks: Vec<Vec<i32>>,
        #[case] expected_items: Vec<i32>,
    ) {
        assert_eq!(
            block_on(ChunkedStream::from_chunks(chunks.clone()).collect::<Vec<_>>()),
            chunks
        );
        assert_eq!(
            block_on(ChunkedStream::from_chunks(chunks).items().collect::<Vec<_>>()),
            expected_items
        );
    }

    #[test]
    fn items_yields_a_nameable_stream_type() {
        let items: Items<i32> = ChunkedStream::from_chunks([vec![1, 2], vec![3]]).items();
        assert_eq!(block_on(items.collect::<Vec<_>>()), vec![1, 2, 3]);
    }

    #[rstest]
    #[case(vec![vec![1, 2], vec![3]], vec![vec![10, 20], vec![30]])]
    #[case(vec![vec![], vec![1, 2], vec![]], vec![vec![], vec![10, 20], vec![]])]
    fn map_transforms_per_item_and_keeps_chunk_boundaries(
        #[case] input: Vec<Vec<i32>>,
        #[case] expected: Vec<Vec<i32>>,
    ) {
        let s = ChunkedStream::from_chunks(input).map(|x| x * 10);
        assert_eq!(block_on(s.collect::<Vec<_>>()), expected);
    }

    #[rstest]
    #[case(vec![1, 2, 3, 4, 5], 2, vec![vec![1, 2], vec![3, 4], vec![5]])]
    #[case(vec![1, 2, 3], 1, vec![vec![1], vec![2], vec![3]])]
    #[case(vec![1, 2, 3], 3, vec![vec![1, 2, 3]])]
    #[case(vec![1, 2, 3], 100, vec![vec![1, 2, 3]])]
    #[case(vec![1, 2, 3, 4], 2, vec![vec![1, 2], vec![3, 4]])]
    #[case(vec![], 3, vec![])]
    fn from_items_chunks_by_size(
        #[case] items: Vec<i32>,
        #[case] size: usize,
        #[case] expected: Vec<Vec<i32>>,
    ) {
        let s = ChunkedStream::from_items(items, nz(size));
        assert_eq!(block_on(s.collect::<Vec<_>>()), expected);
    }

    #[rstest]
    #[case(5, 2, vec![2, 2, 1])]
    #[case(4, 2, vec![2, 2])]
    #[case(3, 5, vec![3])]
    fn from_items_chunks_zsts_by_count(
        #[case] n: usize,
        #[case] size: usize,
        #[case] lens: Vec<usize>,
    ) {
        let chunks: Vec<Vec<()>> =
            block_on(ChunkedStream::from_items(std::iter::repeat_n((), n), nz(size)).collect());
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), lens);

        let count = block_on(
            ChunkedStream::from_items(std::iter::repeat_n((), n), nz(size)).items().count(),
        );
        assert_eq!(count, n);
    }

    #[test]
    fn collects_from_an_iterator_of_chunks() {
        let s: ChunkedStream<i32> = [vec![1, 2], vec![3]].into_iter().collect();
        let chunks: Vec<Vec<i32>> = block_on(s.collect());
        assert_eq!(chunks, vec![vec![1, 2], vec![3]]);
    }

    #[rstest]
    #[case(vec![], Ok(vec![]))]
    #[case(vec![vec![Ok(1), Ok(2)], vec![Ok(3)]], Ok(vec![1, 2, 3]))]
    #[case(vec![vec![Ok(1), Err("boom")], vec![Ok(3)]], Err("boom"))]
    #[case(vec![vec![Err("e"), Ok(2)]], Err("e"))]
    #[case(vec![vec![Ok(1), Err("e"), Ok(3)]], Err("e"))]
    #[case(vec![vec![Ok(1), Ok(2)], vec![Ok(3), Err("e")]], Err("e"))]
    #[case(vec![vec![Ok(1), Ok(2)], vec![Ok(3), Err("boom")], vec![Ok(4)]], Err("boom"))]
    fn try_collect_gathers_ok_or_short_circuits_first_error(
        #[case] chunks: Vec<Vec<Result<i32, &'static str>>>,
        #[case] expected: Result<Vec<i32>, &'static str>,
    ) {
        let s: ChunkedStream<Result<i32, &str>> = ChunkedStream::from_chunks(chunks);
        assert_eq!(block_on(s.try_collect()), expected);
    }

    #[test]
    fn try_collect_returns_the_first_error_instance_when_several_exist() {
        #[derive(Debug, PartialEq)]
        enum E {
            First,
            Second,
        }
        let s: ChunkedStream<Result<i32, E>> =
            ChunkedStream::from_chunks([vec![Ok(1), Err(E::First)], vec![Err(E::Second)]]);
        assert_eq!(block_on(s.try_collect::<Vec<_>>()), Err(E::First));
    }

    #[test]
    fn from_error_is_one_err_chunk() {
        let s: ChunkedStream<Result<i32, &str>> = ChunkedStream::from_error("boom");
        let chunks: Vec<Vec<Result<i32, &str>>> = block_on(s.collect());
        assert_eq!(chunks, vec![vec![Err("boom")]]);
    }

    #[test]
    fn from_fallible_items_folds_ok_and_err() {
        let two = NonZeroUsize::new(2).unwrap();

        let ok: ChunkedStream<Result<i32, &str>> =
            ChunkedStream::from_fallible_items(Ok(1..=3), two);
        assert_eq!(block_on(ok.try_collect()), Ok(vec![1, 2, 3]));

        let err =
            ChunkedStream::<Result<i32, &str>>::from_fallible_items::<Vec<i32>>(Err("boom"), two);
        assert_eq!(block_on(err.try_collect::<Vec<_>>()), Err("boom"));
    }

    #[rstest]
    #[case(vec![1, 2, 3, 4, 5], 2, vec![vec![Ok(1), Ok(2)], vec![Ok(3), Ok(4)], vec![Ok(5)]])]
    #[case(vec![1, 2, 3, 4], 2, vec![vec![Ok(1), Ok(2)], vec![Ok(3), Ok(4)]])]
    #[case(vec![1], 3, vec![vec![Ok(1)]])]
    #[case(vec![], 1, vec![])]
    #[case(vec![], 100, vec![])]
    fn from_fallible_items_ok_path_chunks_like_from_items(
        #[case] items: Vec<i32>,
        #[case] size: usize,
        #[case] expected: Vec<Vec<Result<i32, &'static str>>>,
    ) {
        let s = ChunkedStream::<Result<i32, &str>>::from_fallible_items(Ok(items), nz(size));
        assert_eq!(block_on(s.collect::<Vec<_>>()), expected);
    }

    #[rstest]
    #[case(1)]
    #[case(2)]
    #[case(7)]
    fn from_fallible_items_err_path_is_one_chunk_regardless_of_size(#[case] size: usize) {
        let s =
            ChunkedStream::<Result<i32, &str>>::from_fallible_items::<Vec<i32>>(Err("e"), nz(size));
        assert_eq!(block_on(s.collect::<Vec<_>>()), vec![vec![Err("e")]]);
    }

    #[test]
    fn constructors_pull_lazily() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let counted = (0..10).inspect(move |_| {
            c.fetch_add(1, SeqCst);
        });
        let mut s = ChunkedStream::from_items(counted, nz(2));
        assert_eq!(count.load(SeqCst), 0);
        assert_eq!(block_on(s.next()), Some(vec![0, 1]));
        assert_eq!(count.load(SeqCst), 2);

        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let counted = (0..5).map(move |x| {
            c.fetch_add(1, SeqCst);
            vec![x]
        });
        let mut s = ChunkedStream::from_chunks(counted);
        assert_eq!(count.load(SeqCst), 0);
        assert_eq!(block_on(s.next()), Some(vec![0]));
        assert_eq!(count.load(SeqCst), 1);
    }

    #[test]
    fn map_runs_closure_lazily_once_per_item() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut s = ChunkedStream::from_chunks([vec![1, 2], vec![3]]).map(move |x| {
            c.fetch_add(1, SeqCst);
            x * 10
        });
        assert_eq!(calls.load(SeqCst), 0);
        assert_eq!(block_on(s.next()), Some(vec![10, 20]));
        assert_eq!(calls.load(SeqCst), 2);
    }

    #[test]
    fn map_closure_panic_surfaces_at_the_offending_chunks_poll() {
        let mut s = ChunkedStream::from_chunks([vec![1, 2], vec![3]]).map(|x| {
            if x == 3 {
                panic!("boom")
            } else {
                x * 10
            }
        });
        assert_eq!(block_on(s.next()), Some(vec![10, 20]));
        assert!(catch_unwind(AssertUnwindSafe(|| block_on(s.next()))).is_err());
    }

    #[test]
    fn dropping_midstream_drops_every_item_exactly_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        struct DropCounter(Arc<AtomicUsize>);
        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.fetch_add(1, SeqCst);
            }
        }
        let items: Vec<DropCounter> = (0..5).map(|_| DropCounter(drops.clone())).collect();
        let mut s = ChunkedStream::from_items(items, nz(2));
        let first = block_on(s.next());
        assert_eq!(first.as_ref().map(Vec::len), Some(2));
        drop(first);
        drop(s);
        assert_eq!(drops.load(SeqCst), 5);
    }

    #[test]
    fn map_and_items_move_owned_elements_without_cloning() {
        #[derive(Debug)]
        struct Loud(i32);
        impl Clone for Loud {
            fn clone(&self) -> Self {
                panic!("must not clone")
            }
        }
        let out: Vec<i32> = block_on(
            ChunkedStream::from_items([Loud(1), Loud(2), Loud(3)], nz(2))
                .map(|l| l.0)
                .items()
                .collect(),
        );
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn forwards_pending_and_the_upstream_wake() {
        let armed = Arc::new(AtomicBool::new(false));
        let aw = Arc::new(futures::task::AtomicWaker::new());

        let a = armed.clone();
        let w = aw.clone();
        let emitted = AtomicBool::new(false);
        let src = stream::poll_fn(move |cx| {
            if a.load(SeqCst) {
                if emitted.swap(true, SeqCst) {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(vec![1, 2]))
                }
            } else {
                w.register(cx.waker());
                Poll::Pending
            }
        });
        let mut cs = ChunkedStream::new(src);

        let cw = Arc::new(CountWaker(AtomicUsize::new(0)));
        let wk = waker(cw.clone());
        let mut cx = Context::from_waker(&wk);

        assert!(Pin::new(&mut cs).poll_next(&mut cx).is_pending());
        assert_eq!(cw.0.load(SeqCst), 0);

        armed.store(true, SeqCst);
        aw.wake();
        assert_eq!(cw.0.load(SeqCst), 1);

        assert_eq!(Pin::new(&mut cs).poll_next(&mut cx), Poll::Ready(Some(vec![1, 2])));
        assert_eq!(Pin::new(&mut cs).poll_next(&mut cx), Poll::Ready(None));
    }

    #[test]
    fn items_resumes_across_a_pending_chunk_boundary() {
        let resume = Arc::new(AtomicBool::new(false));
        let rs = resume.clone();
        let stage = AtomicUsize::new(0);
        let outer = stream::poll_fn(move |cx| match stage.load(SeqCst) {
            0 => {
                stage.store(1, SeqCst);
                Poll::Ready(Some(vec![1]))
            }
            1 => {
                if rs.load(SeqCst) {
                    stage.store(2, SeqCst);
                    Poll::Ready(Some(vec![2]))
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            _ => Poll::Ready(None),
        });
        let mut items = ChunkedStream::new(outer).items();

        assert_eq!(poll_now(&mut items), Poll::Ready(Some(1)));
        assert!(poll_now(&mut items).is_pending());

        resume.store(true, SeqCst);
        assert_eq!(poll_now(&mut items), Poll::Ready(Some(2)));
        assert_eq!(poll_now(&mut items), Poll::Ready(None));
    }

    #[test]
    fn try_collect_short_circuits_before_a_pending_tail() {
        let tail = stream::poll_fn(|_| -> Poll<Option<Vec<Result<i32, &str>>>> {
            panic!("polled past the first error")
        });
        let src = stream::iter([vec![Ok(1), Err("boom")]]).chain(tail);
        let s = ChunkedStream::new(src);
        assert_eq!(block_on(s.try_collect::<Vec<_>>()), Err("boom"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn streams_are_send_and_drive_by_reference() {
        let cs = ChunkedStream::from_items(0..6, nz(2));
        let flat: Vec<i32> =
            tokio::spawn(async move { cs.items().collect().await }).await.expect("task panicked");
        assert_eq!(flat, vec![0, 1, 2, 3, 4, 5]);

        let mut items = ChunkedStream::from_chunks([vec![1], vec![2, 3]]).items();
        let mut seen = Vec::new();
        while let Some(x) = items.next().await {
            seen.push(x);
        }
        assert_eq!(seen, vec![1, 2, 3]);
    }

    proptest! {
        #[test]
        fn from_items_chunk_arithmetic(xs in prop::collection::vec(0i32..1000, 0..50), n in 1usize..8) {
            let size = nz(n);
            let chunks: Vec<Vec<i32>> = block_on(ChunkedStream::from_items(xs.clone(), size).collect());

            let flat: Vec<i32> = chunks.iter().flatten().copied().collect();
            prop_assert_eq!(&flat, &xs);

            let items: Vec<i32> =
                block_on(ChunkedStream::from_items(xs.clone(), size).items().collect());
            prop_assert_eq!(&items, &xs);

            if xs.is_empty() {
                prop_assert!(chunks.is_empty());
            } else {
                prop_assert_eq!(chunks.len(), xs.len().div_ceil(n));
                let (last, rest) = chunks.split_last().expect("non-empty");
                for c in rest {
                    prop_assert_eq!(c.len(), n);
                }
                prop_assert!(!last.is_empty() && last.len() <= n);
            }
        }

        #[test]
        fn chunk_views_agree(
            chunks in prop::collection::vec(prop::collection::vec(any::<i32>(), 0..5), 0..8),
        ) {
            let round: Vec<Vec<i32>> = block_on(ChunkedStream::from_chunks(chunks.clone()).collect());
            prop_assert_eq!(&round, &chunks);

            let via_from_iter: Vec<Vec<i32>> =
                block_on(chunks.clone().into_iter().collect::<ChunkedStream<i32>>().collect());
            prop_assert_eq!(&via_from_iter, &chunks);

            let items: Vec<i32> =
                block_on(ChunkedStream::from_chunks(chunks.clone()).items().collect());
            let expected: Vec<i32> = chunks.iter().flatten().copied().collect();
            prop_assert_eq!(&items, &expected);
        }

        #[test]
        fn map_commutes_with_items(
            chunks in prop::collection::vec(prop::collection::vec(any::<i32>(), 0..5), 0..8),
        ) {
            let f = |x: i32| x.to_string();

            let left: Vec<String> =
                block_on(ChunkedStream::from_chunks(chunks.clone()).map(f).items().collect());
            let right: Vec<String> =
                block_on(ChunkedStream::from_chunks(chunks.clone()).items().map(f).collect());
            let flat: Vec<String> = chunks.iter().flatten().map(|x| f(*x)).collect();
            prop_assert_eq!(&left, &right);
            prop_assert_eq!(&left, &flat);

            let mapped: Vec<Vec<String>> =
                block_on(ChunkedStream::from_chunks(chunks.clone()).map(f).collect());
            let in_lens: Vec<usize> = chunks.iter().map(Vec::len).collect();
            let out_lens: Vec<usize> = mapped.iter().map(Vec::len).collect();
            prop_assert_eq!(out_lens, in_lens);
        }

        #[test]
        fn try_collect_matches_reference(
            chunks in prop::collection::vec(
                prop::collection::vec(
                    prop_oneof![(0i32..100).prop_map(Ok::<i32, i32>), (0i32..100).prop_map(Err::<i32, i32>)],
                    0..5,
                ),
                0..6,
            ),
        ) {
            let expected: Result<Vec<i32>, i32> = chunks.iter().flatten().copied().collect();
            let got = block_on(ChunkedStream::from_chunks(chunks).try_collect());
            prop_assert_eq!(got, expected);
        }

        #[test]
        fn from_fallible_items_ok_delegates_to_from_items(
            xs in prop::collection::vec(0i32..100, 0..50),
            n in 1usize..8,
        ) {
            let size = nz(n);
            let lhs: Vec<Vec<Result<i32, i32>>> = block_on(
                ChunkedStream::<Result<i32, i32>>::from_fallible_items(Ok(xs.clone()), size).collect(),
            );
            let rhs: Vec<Vec<Result<i32, i32>>> = block_on(
                ChunkedStream::<Result<i32, i32>>::from_items(xs.into_iter().map(Ok), size).collect(),
            );
            prop_assert_eq!(lhs, rhs);
        }

        #[test]
        fn from_fallible_items_err_equals_from_error(e in any::<i32>(), n in 1usize..8) {
            let got: Vec<Vec<Result<i32, i32>>> = block_on(
                ChunkedStream::<Result<i32, i32>>::from_fallible_items::<Vec<i32>>(Err(e), nz(n))
                    .collect(),
            );
            prop_assert_eq!(&got, &vec![vec![Err(e)]]);

            let via_from_error: Vec<Vec<Result<i32, i32>>> =
                block_on(ChunkedStream::<Result<i32, i32>>::from_error(e).collect());
            prop_assert_eq!(got, via_from_error);
        }

        #[test]
        fn items_flattens_under_arbitrary_pending(
            chunks in prop::collection::vec(prop::collection::vec(any::<i32>(), 0..6), 0..20),
            schedule in prop::collection::vec(any::<bool>(), 0..40),
        ) {
            let items: Vec<i32> = block_on(
                ChunkedStream::new(scheduled(chunks.clone(), schedule.clone())).items().collect(),
            );
            let expected: Vec<i32> = chunks.iter().flatten().copied().collect();
            prop_assert_eq!(items, expected);

            let round: Vec<Vec<i32>> =
                block_on(ChunkedStream::new(scheduled(chunks.clone(), schedule)).collect());
            prop_assert_eq!(round, chunks);
        }
    }
}
