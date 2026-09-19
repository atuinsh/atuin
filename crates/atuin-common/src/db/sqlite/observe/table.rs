use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;

use super::ObserveError;

pub struct SqliteTableObserver<E> {
    inner: Pin<Box<dyn Stream<Item = Result<E, ObserveError>> + Send>>,
}

impl<E> SqliteTableObserver<E> {
    pub(super) fn new(inner: impl Stream<Item = Result<E, ObserveError>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(inner),
        }
    }
}

impl<E> Stream for SqliteTableObserver<E> {
    type Item = Result<E, ObserveError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use rstest::rstest;

    use super::*;
    use crate::db::sqlite::observe::ObserveError;

    #[rstest]
    #[tokio::test]
    async fn handle_forwards_stream_items() {
        let inner = futures::stream::iter(vec![Ok::<u32, ObserveError>(1), Ok(2), Ok(3)]);
        let observer = SqliteTableObserver::new(inner);

        let got: Vec<u32> = observer.map(Result::unwrap).collect().await;
        assert_eq!(got, vec![1, 2, 3]);
    }
}
