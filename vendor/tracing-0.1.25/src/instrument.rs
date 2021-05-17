use crate::stdlib::pin::Pin;
use crate::stdlib::task::{Context, Poll};
use crate::stdlib::{future::Future, marker::Sized};
use crate::{dispatcher, span::Span, Dispatch};
use pin_project_lite::pin_project;

/// Attaches spans to a `std::future::Future`.
///
/// Extension trait allowing futures to be
/// instrumented with a `tracing` [span].
///
/// [span]:  ../struct.Span.html
pub trait Instrument: Sized {
    /// Instruments this type with the provided `Span`, returning an
    /// `Instrumented` wrapper.
    ///
    /// The attached `Span` will be [entered] every time the instrumented `Future` is polled.
    ///
    /// # Examples
    ///
    /// Instrumenting a future:
    ///
    /// ```rust
    /// use tracing::Instrument;
    ///
    /// # async fn doc() {
    /// let my_future = async {
    ///     // ...
    /// };
    ///
    /// my_future
    ///     .instrument(tracing::info_span!("my_future"))
    ///     .await
    /// # }
    /// ```
    ///
    /// [entered]: ../struct.Span.html#method.enter
    fn instrument(self, span: Span) -> Instrumented<Self> {
        Instrumented { inner: self, span }
    }

    /// Instruments this type with the [current] `Span`, returning an
    /// `Instrumented` wrapper.
    ///
    /// If the instrumented type is a future, stream, or sink, the attached `Span`
    /// will be [entered] every time it is polled. If the instrumented type
    /// is a future executor, every future spawned on that executor will be
    /// instrumented by the attached `Span`.
    ///
    /// This can be used to propagate the current span when spawning a new future.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use tracing::Instrument;
    ///
    /// # async fn doc() {
    /// let span = tracing::info_span!("my_span");
    /// let _enter = span.enter();
    ///
    /// // ...
    ///
    /// let future = async {
    ///     tracing::debug!("this event will occur inside `my_span`");
    ///     // ...
    /// };
    /// tokio::spawn(future.in_current_span());
    /// # }
    /// ```
    ///
    /// [current]: ../struct.Span.html#method.current
    /// [entered]: ../struct.Span.html#method.enter
    #[inline]
    fn in_current_span(self) -> Instrumented<Self> {
        self.instrument(Span::current())
    }
}

/// Extension trait allowing futures to be instrumented with
/// a `tracing` [`Subscriber`].
///
/// [`Subscriber`]: ../trait.Subscriber.html
pub trait WithSubscriber: Sized {
    /// Attaches the provided [`Subscriber`] to this type, returning a
    /// `WithDispatch` wrapper.
    ///
    /// The attached subscriber will be set as the [default] when the returned `Future` is polled.
    ///
    /// [`Subscriber`]: ../trait.Subscriber.html
    /// [default]: https://docs.rs/tracing/latest/tracing/dispatcher/index.html#setting-the-default-subscriber
    fn with_subscriber<S>(self, subscriber: S) -> WithDispatch<Self>
    where
        S: Into<Dispatch>,
    {
        WithDispatch {
            inner: self,
            dispatch: subscriber.into(),
        }
    }

    /// Attaches the current [default] [`Subscriber`] to this type, returning a
    /// `WithDispatch` wrapper.
    ///
    /// When the wrapped type is a future, stream, or sink, the attached
    /// subscriber will be set as the [default] while it is being polled.
    /// When the wrapped type is an executor, the subscriber will be set as the
    /// default for any futures spawned on that executor.
    ///
    /// This can be used to propagate the current dispatcher context when
    /// spawning a new future.
    ///
    /// [`Subscriber`]: ../trait.Subscriber.html
    /// [default]: https://docs.rs/tracing/latest/tracing/dispatcher/index.html#setting-the-default-subscriber
    #[inline]
    fn with_current_subscriber(self) -> WithDispatch<Self> {
        WithDispatch {
            inner: self,
            dispatch: dispatcher::get_default(|default| default.clone()),
        }
    }
}

pin_project! {
    /// A future that has been instrumented with a `tracing` subscriber.
    #[derive(Clone, Debug)]
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct WithDispatch<T> {
        #[pin]
        inner: T,
        dispatch: Dispatch,
    }
}

pin_project! {
    /// A future that has been instrumented with a `tracing` span.
    #[derive(Debug, Clone)]
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct Instrumented<T> {
        #[pin]
        inner: T,
        span: Span,
    }
}

impl<T: Future> Future for Instrumented<T> {
    type Output = T::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _enter = this.span.enter();
        this.inner.poll(cx)
    }
}

impl<T: Sized> Instrument for T {}

impl<T> Instrumented<T> {
    /// Borrows the `Span` that this type is instrumented by.
    pub fn span(&self) -> &Span {
        &self.span
    }

    /// Mutably borrows the `Span` that this type is instrumented by.
    pub fn span_mut(&mut self) -> &mut Span {
        &mut self.span
    }

    /// Borrows the wrapped type.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Mutably borrows the wrapped type.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Get a pinned reference to the wrapped type.
    pub fn inner_pin_ref(self: Pin<&Self>) -> Pin<&T> {
        self.project_ref().inner
    }

    /// Get a pinned mutable reference to the wrapped type.
    pub fn inner_pin_mut(self: Pin<&mut Self>) -> Pin<&mut T> {
        self.project().inner
    }

    /// Consumes the `Instrumented`, returning the wrapped type.
    ///
    /// Note that this drops the span.
    pub fn into_inner(self) -> T {
        self.inner
    }
}
