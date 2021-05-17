use crate::Body;
use bytes::Buf;
use std::{
    fmt,
    pin::Pin,
    task::{Context, Poll},
};

/// A boxed [`Body`] trait object.
pub struct BoxBody<D, E> {
    inner: Pin<Box<dyn Body<Data = D, Error = E> + Send + Sync + 'static>>,
}

impl<D, E> BoxBody<D, E> {
    /// Create a new `BoxBody`.
    pub fn new<B>(body: B) -> Self
    where
        B: Body<Data = D, Error = E> + Send + Sync + 'static,
        D: Buf,
    {
        Self {
            inner: Box::pin(body),
        }
    }
}

impl<D, E> fmt::Debug for BoxBody<D, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxBody").finish()
    }
}

impl<D, E> Body for BoxBody<D, E>
where
    D: Buf,
{
    type Data = D;
    type Error = E;

    fn poll_data(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        self.inner.as_mut().poll_data(cx)
    }

    fn poll_trailers(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        self.inner.as_mut().poll_trailers(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> crate::SizeHint {
        self.inner.size_hint()
    }
}
