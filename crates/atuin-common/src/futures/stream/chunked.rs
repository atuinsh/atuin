use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt, stream};

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

    pub fn items(self) -> impl Stream<Item = T> {
        self.inner.flat_map(stream::iter)
    }
}

impl<T: Send + 'static> FromIterator<Vec<T>> for ChunkedStream<T> {
    fn from_iter<I: IntoIterator<Item = Vec<T>>>(chunks: I) -> Self {
        Self::from_chunks(chunks.into_iter().collect::<Vec<_>>())
    }
}

impl<T> Stream for ChunkedStream<T> {
    type Item = Vec<T>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use futures::StreamExt;
    use futures::executor::block_on;

    use super::ChunkedStream;

    #[test]
    fn items_walks_every_element_across_chunks() {
        let s = ChunkedStream::from_chunks([vec![1, 2], vec![3], vec![4, 5]]);
        let all: Vec<i32> = block_on(s.items().collect());
        assert_eq!(all, vec![1, 2, 3, 4, 5]);
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
}
