use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt, TryStreamExt, stream};

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

    pub async fn try_collect(self) -> Result<Vec<T>, E> {
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
    use std::num::NonZeroUsize;

    use futures::StreamExt;
    use futures::executor::block_on;

    use super::{ChunkedStream, Items};

    #[test]
    fn items_walks_every_element_across_chunks() {
        let s = ChunkedStream::from_chunks([vec![1, 2], vec![3], vec![4, 5]]);
        let all: Vec<i32> = block_on(s.items().collect());
        assert_eq!(all, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn items_yields_a_nameable_stream_type() {
        let items: Items<i32> = ChunkedStream::from_chunks([vec![1, 2], vec![3]]).items();
        assert_eq!(block_on(items.collect::<Vec<_>>()), vec![1, 2, 3]);
    }

    #[test]
    fn from_items_chunks_by_size() {
        let s = ChunkedStream::from_items(1..=5, NonZeroUsize::new(2).unwrap());
        let chunks: Vec<Vec<i32>> = block_on(s.collect());
        assert_eq!(chunks, vec![vec![1, 2], vec![3, 4], vec![5]]);
    }

    #[test]
    fn map_transforms_elements_and_keeps_chunk_boundaries() {
        let s = ChunkedStream::from_chunks([vec![1, 2], vec![3]]).map(|x| x * 10);
        let chunks: Vec<Vec<i32>> = block_on(s.collect());
        assert_eq!(chunks, vec![vec![10, 20], vec![30]]);
    }

    #[test]
    fn collects_from_an_iterator_of_chunks() {
        let s: ChunkedStream<i32> = [vec![1, 2], vec![3]].into_iter().collect();
        let chunks: Vec<Vec<i32>> = block_on(s.collect());
        assert_eq!(chunks, vec![vec![1, 2], vec![3]]);
    }

    #[test]
    fn try_collect_gathers_ok_items() {
        let s: ChunkedStream<Result<i32, &str>> =
            ChunkedStream::from_chunks([vec![Ok(1), Ok(2)], vec![Ok(3)]]);
        assert_eq!(block_on(s.try_collect()), Ok(vec![1, 2, 3]));
    }

    #[test]
    fn try_collect_stops_at_first_error() {
        let s: ChunkedStream<Result<i32, &str>> =
            ChunkedStream::from_chunks([vec![Ok(1), Err("boom")], vec![Ok(3)]]);
        assert_eq!(block_on(s.try_collect()), Err("boom"));
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

        let ok: ChunkedStream<Result<i32, &str>> = ChunkedStream::from_fallible_items(Ok(1..=3), two);
        assert_eq!(block_on(ok.try_collect()), Ok(vec![1, 2, 3]));

        let err = ChunkedStream::<Result<i32, &str>>::from_fallible_items::<Vec<i32>>(Err("boom"), two);
        assert_eq!(block_on(err.try_collect()), Err("boom"));
    }
}
