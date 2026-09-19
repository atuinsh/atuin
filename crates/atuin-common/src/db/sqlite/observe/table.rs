use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::mpsc;
use tokio_util::task::AbortOnDropHandle;

use super::ObserveError;

pub struct SqliteTableObserver<E> {
    rx: mpsc::Receiver<Result<E, ObserveError>>,
    _task: Arc<AbortOnDropHandle<()>>,
}

impl<E> SqliteTableObserver<E> {
    pub(super) fn new(
        rx: mpsc::Receiver<Result<E, ObserveError>>,
        task: AbortOnDropHandle<()>,
    ) -> Self {
        Self {
            rx,
            _task: Arc::new(task),
        }
    }
}

impl<E: Send + 'static> Stream for SqliteTableObserver<E> {
    type Item = Result<E, ObserveError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use rstest::rstest;
    use tokio::sync::mpsc;
    use tokio_util::task::AbortOnDropHandle;

    use super::*;
    use crate::db::sqlite::observe::ObserveError;

    #[rstest]
    #[tokio::test]
    async fn handle_forwards_channel_items() {
        let (tx, rx) = mpsc::channel::<Result<u32, ObserveError>>(4);
        let task = tokio::spawn(async move {
            for n in [1u32, 2, 3] {
                tx.send(Ok(n)).await.unwrap();
            }
        });
        let observer = SqliteTableObserver::new(rx, AbortOnDropHandle::new(task));

        let got: Vec<u32> = observer.map(Result::unwrap).collect().await;
        assert_eq!(got, vec![1, 2, 3]);
    }
}
