use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_channel::mpsc;
use futures_core::future::BoxFuture;
use futures_core::stream::Stream;
use futures_util::{pin_mut, FutureExt, SinkExt};

use crate::error::Error;

pub struct TryAsyncStream<'a, T> {
    receiver: mpsc::Receiver<Result<T, Error>>,
    future: BoxFuture<'a, Result<(), Error>>,
}

impl<'a, T> TryAsyncStream<'a, T> {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: FnOnce(mpsc::Sender<Result<T, Error>>) -> Fut + Send,
        Fut: 'a + Future<Output = Result<(), Error>> + Send,
        T: 'a + Send,
    {
        let (mut sender, receiver) = mpsc::channel(0);

        let future = f(sender.clone());
        let future = async move {
            if let Err(error) = future.await {
                let _ = sender.send(Err(error)).await;
            }

            Ok(())
        }
        .fuse()
        .boxed();

        Self { future, receiver }
    }
}

impl<'a, T> Stream for TryAsyncStream<'a, T> {
    type Item = Result<T, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let future = &mut self.future;
        pin_mut!(future);

        // the future is fused so its safe to call forever
        // the future advances our "stream"
        // the future should be polled in tandem with the stream receiver
        let _ = future.poll(cx);

        let receiver = &mut self.receiver;
        pin_mut!(receiver);

        // then we check to see if we have anything to return
        receiver.poll_next(cx)
    }
}

macro_rules! try_stream {
    ($($block:tt)*) => {
        crate::ext::async_stream::TryAsyncStream::new(move |mut sender| async move {
            macro_rules! r#yield {
                ($v:expr) => {
                    let _ = futures_util::sink::SinkExt::send(&mut sender, Ok($v)).await;
                }
            }

            $($block)*
        })
    }
}
