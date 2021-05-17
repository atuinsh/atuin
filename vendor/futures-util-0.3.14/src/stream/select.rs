use super::assert_stream;
use crate::stream::{StreamExt, Fuse};
use core::pin::Pin;
use futures_core::stream::{FusedStream, Stream};
use futures_core::task::{Context, Poll};
use pin_project_lite::pin_project;

pin_project! {
    /// Stream for the [`select()`] function.
    #[derive(Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct Select<St1, St2> {
        #[pin]
        stream1: Fuse<St1>,
        #[pin]
        stream2: Fuse<St2>,
        flag: bool,
    }
}

/// This function will attempt to pull items from both streams. Each
/// stream will be polled in a round-robin fashion, and whenever a stream is
/// ready to yield an item that item is yielded.
///
/// After one of the two input stream completes, the remaining one will be
/// polled exclusively. The returned stream completes when both input
/// streams have completed.
///
/// Note that this function consumes both streams and returns a wrapped
/// version of them.
pub fn select<St1, St2>(stream1: St1, stream2: St2) -> Select<St1, St2>
    where St1: Stream,
          St2: Stream<Item = St1::Item>
{
    assert_stream::<St1::Item, _>(Select {
        stream1: stream1.fuse(),
        stream2: stream2.fuse(),
        flag: false,
    })
}

impl<St1, St2> Select<St1, St2> {
    /// Acquires a reference to the underlying streams that this combinator is
    /// pulling from.
    pub fn get_ref(&self) -> (&St1, &St2) {
        (self.stream1.get_ref(), self.stream2.get_ref())
    }

    /// Acquires a mutable reference to the underlying streams that this
    /// combinator is pulling from.
    ///
    /// Note that care must be taken to avoid tampering with the state of the
    /// stream which may otherwise confuse this combinator.
    pub fn get_mut(&mut self) -> (&mut St1, &mut St2) {
        (self.stream1.get_mut(), self.stream2.get_mut())
    }

    /// Acquires a pinned mutable reference to the underlying streams that this
    /// combinator is pulling from.
    ///
    /// Note that care must be taken to avoid tampering with the state of the
    /// stream which may otherwise confuse this combinator.
    pub fn get_pin_mut(self: Pin<&mut Self>) -> (Pin<&mut St1>, Pin<&mut St2>) {
        let this = self.project();
        (this.stream1.get_pin_mut(), this.stream2.get_pin_mut())
    }

    /// Consumes this combinator, returning the underlying streams.
    ///
    /// Note that this may discard intermediate state of this combinator, so
    /// care should be taken to avoid losing resources when this is called.
    pub fn into_inner(self) -> (St1, St2) {
        (self.stream1.into_inner(), self.stream2.into_inner())
    }
}

impl<St1, St2> FusedStream for Select<St1, St2>
    where St1: Stream,
          St2: Stream<Item = St1::Item>
{
    fn is_terminated(&self) -> bool {
        self.stream1.is_terminated() && self.stream2.is_terminated()
    }
}

impl<St1, St2> Stream for Select<St1, St2>
    where St1: Stream,
          St2: Stream<Item = St1::Item>
{
    type Item = St1::Item;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<St1::Item>> {
        let this = self.project();
        if !*this.flag {
            poll_inner(this.flag, this.stream1, this.stream2, cx)
        } else {
            poll_inner(this.flag, this.stream2, this.stream1, cx)
        }
    }
}

fn poll_inner<St1, St2>(
    flag: &mut bool,
    a: Pin<&mut St1>,
    b: Pin<&mut St2>,
    cx: &mut Context<'_>
) -> Poll<Option<St1::Item>>
    where St1: Stream, St2: Stream<Item = St1::Item>
{
    let a_done = match a.poll_next(cx) {
        Poll::Ready(Some(item)) => {
            // give the other stream a chance to go first next time
            *flag = !*flag;
            return Poll::Ready(Some(item))
        },
        Poll::Ready(None) => true,
        Poll::Pending => false,
    };

    match b.poll_next(cx) {
        Poll::Ready(Some(item)) => {
            Poll::Ready(Some(item))
        }
        Poll::Ready(None) if a_done => Poll::Ready(None),
        Poll::Ready(None) | Poll::Pending => Poll::Pending,
    }
}
