//! Common protobuf utilities in the daemon.

use std::future::Future;

use futures::{Stream, TryStreamExt};
use itertools::process_results;
use thiserror::Error;
use tonic::Status;

pub mod pb;

#[derive(Debug, Error)]
#[error("too many items in one request stream: the limit is {0}")]
pub struct TooManyItemsError(pub usize);

/// Why draining a client-streamed, chunked request with [`TryCollectResultsCappedExt`] failed.
#[derive(Debug, Error)]
pub enum CollectCappedError<E> {
    /// The stream carried more than the caller's `max` items.
    #[error(transparent)]
    TooMany(TooManyItemsError),
    /// A chunk yielded an item that failed to parse.
    #[error(transparent)]
    Item(E),
    /// The request stream itself errored (transport, cancellation, ...).
    #[error(transparent)]
    Stream(Status),
}

impl<E> From<CollectCappedError<E>> for Status
where
    E: std::error::Error + Into<Self>,
{
    fn from(err: CollectCappedError<E>) -> Self {
        match err {
            // A caller's fault: the batch is unusable, so reject it whole.
            CollectCappedError::TooMany(err) => Self::invalid_argument(err.to_string()),
            CollectCappedError::Item(e) => e.into(),
            // Already a `Status` from the transport; surface it unchanged.
            CollectCappedError::Stream(status) => status,
        }
    }
}

/// Collect a client-streamed, chunked request into one validated, capped `Vec`.
pub trait TryCollectResultsCappedExt<C, T, E>: Stream<Item = Result<C, Status>> + Sized
where
    C: IntoIterator<Item = Result<T, E>>,
    E: std::error::Error,
{
    /// Drain the whole stream into one `Vec`, limiting the maximum length with `max`.
    fn try_collect_capped(
        self,
        max: usize,
    ) -> impl Future<Output = Result<Vec<T>, CollectCappedError<E>>> + Send
    where
        Self: Send,
        C: Send,
        T: Send,
        E: Send,
    {
        self.map_err(CollectCappedError::Stream).try_fold(
            Vec::new(),
            move |mut acc, chunk| async move {
                let headroom = max - acc.len();

                process_results(chunk, |items| acc.extend(items.take(headroom + 1)))
                    .map_err(CollectCappedError::Item)?;

                if acc.len() > max {
                    return Err(CollectCappedError::TooMany(TooManyItemsError(max)));
                }
                Ok(acc)
            },
        )
    }
}

impl<S, C, T, E> TryCollectResultsCappedExt<C, T, E> for S
where
    S: Stream<Item = Result<C, Status>>,
    C: IntoIterator<Item = Result<T, E>>,
    E: std::error::Error,
{
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use tonic::Code;

    use super::*;

    #[derive(Debug, PartialEq, Eq, Error)]
    #[error("bad item: {0}")]
    struct BadItem(i32);

    /// A chunk is any `IntoIterator<Item = Result<T, E>>` -- a plain `Vec` is the simplest one.
    fn chunk(items: Vec<Result<i32, BadItem>>) -> Vec<Result<i32, BadItem>> {
        items
    }

    #[rstest]
    #[tokio::test]
    async fn collects_every_chunk_in_stream_order() {
        let stream =
            futures::stream::iter(vec![Ok(chunk(vec![Ok(1), Ok(2)])), Ok(chunk(vec![Ok(3)]))]);
        let got = stream.try_collect_capped(100).await.unwrap();
        assert_eq!(got, vec![1, 2, 3]);
    }

    #[rstest]
    #[tokio::test]
    async fn rejects_more_than_max() {
        // Two chunks of two; the fourth item trips the cap of 3.
        let stream = futures::stream::iter(vec![
            Ok(chunk(vec![Ok(1), Ok(2)])),
            Ok(chunk(vec![Ok(3), Ok(4)])),
        ]);
        let err = stream.try_collect_capped(3).await.unwrap_err();
        assert!(matches!(err, CollectCappedError::TooMany(TooManyItemsError(3))));
    }

    #[rstest]
    #[tokio::test]
    async fn propagates_an_item_error() {
        let stream = futures::stream::iter(vec![Ok(chunk(vec![Ok(1), Err(BadItem(2)), Ok(3)]))]);
        let err = stream.try_collect_capped(100).await.unwrap_err();
        assert!(matches!(err, CollectCappedError::Item(BadItem(2))));
    }

    #[rstest]
    #[tokio::test]
    async fn propagates_a_stream_error_verbatim() {
        let stream =
            futures::stream::iter(vec![Ok(chunk(vec![Ok(1)])), Err(Status::internal("boom"))]);
        let err = stream.try_collect_capped(100).await.unwrap_err();
        match err {
            CollectCappedError::Stream(status) => {
                assert_eq!(status.code(), Code::Internal);
                assert_eq!(status.message(), "boom");
            }
            other => panic!("expected a stream error, got {other:?}"),
        }
    }
}
