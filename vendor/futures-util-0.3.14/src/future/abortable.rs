use super::assert_future;
use crate::task::AtomicWaker;
use futures_core::future::Future;
use futures_core::task::{Context, Poll};
use core::fmt;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use alloc::sync::Arc;
use pin_project_lite::pin_project;

pin_project! {
    /// A future which can be remotely short-circuited using an `AbortHandle`.
    #[derive(Debug, Clone)]
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct Abortable<Fut> {
        #[pin]
        future: Fut,
        inner: Arc<AbortInner>,
    }
}

impl<Fut> Abortable<Fut> where Fut: Future {
    /// Creates a new `Abortable` future using an existing `AbortRegistration`.
    /// `AbortRegistration`s can be acquired through `AbortHandle::new`.
    ///
    /// When `abort` is called on the handle tied to `reg` or if `abort` has
    /// already been called, the future will complete immediately without making
    /// any further progress.
    ///
    /// Example:
    ///
    /// ```
    /// # futures::executor::block_on(async {
    /// use futures::future::{Abortable, AbortHandle, Aborted};
    ///
    /// let (abort_handle, abort_registration) = AbortHandle::new_pair();
    /// let future = Abortable::new(async { 2 }, abort_registration);
    /// abort_handle.abort();
    /// assert_eq!(future.await, Err(Aborted));
    /// # });
    /// ```
    pub fn new(future: Fut, reg: AbortRegistration) -> Self {
        assert_future::<Result<Fut::Output, Aborted>, _>(Self {
            future,
            inner: reg.inner,
        })
    }
}

/// A registration handle for a `Abortable` future.
/// Values of this type can be acquired from `AbortHandle::new` and are used
/// in calls to `Abortable::new`.
#[derive(Debug)]
pub struct AbortRegistration {
    inner: Arc<AbortInner>,
}

/// A handle to a `Abortable` future.
#[derive(Debug, Clone)]
pub struct AbortHandle {
    inner: Arc<AbortInner>,
}

impl AbortHandle {
    /// Creates an (`AbortHandle`, `AbortRegistration`) pair which can be used
    /// to abort a running future.
    ///
    /// This function is usually paired with a call to `Abortable::new`.
    ///
    /// Example:
    ///
    /// ```
    /// # futures::executor::block_on(async {
    /// use futures::future::{Abortable, AbortHandle, Aborted};
    ///
    /// let (abort_handle, abort_registration) = AbortHandle::new_pair();
    /// let future = Abortable::new(async { 2 }, abort_registration);
    /// abort_handle.abort();
    /// assert_eq!(future.await, Err(Aborted));
    /// # });
    /// ```
    pub fn new_pair() -> (Self, AbortRegistration) {
        let inner = Arc::new(AbortInner {
            waker: AtomicWaker::new(),
            cancel: AtomicBool::new(false),
        });

        (
            Self {
                inner: inner.clone(),
            },
            AbortRegistration {
                inner,
            },
        )
    }
}

// Inner type storing the waker to awaken and a bool indicating that it
// should be cancelled.
#[derive(Debug)]
struct AbortInner {
    waker: AtomicWaker,
    cancel: AtomicBool,
}

/// Creates a new `Abortable` future and a `AbortHandle` which can be used to stop it.
///
/// This function is a convenient (but less flexible) alternative to calling
/// `AbortHandle::new` and `Abortable::new` manually.
///
/// This function is only available when the `std` or `alloc` feature of this
/// library is activated, and it is activated by default.
pub fn abortable<Fut>(future: Fut) -> (Abortable<Fut>, AbortHandle)
    where Fut: Future
{
    let (handle, reg) = AbortHandle::new_pair();
    (
        Abortable::new(future, reg),
        handle,
    )
}

/// Indicator that the `Abortable` future was aborted.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Aborted;

impl fmt::Display for Aborted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`Abortable` future has been aborted")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Aborted {}

impl<Fut> Future for Abortable<Fut> where Fut: Future {
    type Output = Result<Fut::Output, Aborted>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Check if the future has been aborted
        if self.inner.cancel.load(Ordering::Relaxed) {
            return Poll::Ready(Err(Aborted))
        }

        // attempt to complete the future
        if let Poll::Ready(x) = self.as_mut().project().future.poll(cx) {
            return Poll::Ready(Ok(x))
        }

        // Register to receive a wakeup if the future is aborted in the... future
        self.inner.waker.register(cx.waker());

        // Check to see if the future was aborted between the first check and
        // registration.
        // Checking with `Relaxed` is sufficient because `register` introduces an
        // `AcqRel` barrier.
        if self.inner.cancel.load(Ordering::Relaxed) {
            return Poll::Ready(Err(Aborted))
        }

        Poll::Pending
    }
}

impl AbortHandle {
    /// Abort the `Abortable` future associated with this handle.
    ///
    /// Notifies the Abortable future associated with this handle that it
    /// should abort. Note that if the future is currently being polled on
    /// another thread, it will not immediately stop running. Instead, it will
    /// continue to run until its poll method returns.
    pub fn abort(&self) {
        self.inner.cancel.store(true, Ordering::Relaxed);
        self.inner.waker.wake();
    }
}
