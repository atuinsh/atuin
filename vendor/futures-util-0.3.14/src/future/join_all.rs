//! Definition of the `JoinAll` combinator, waiting for all of a list of futures
//! to finish.

use core::fmt;
use core::future::Future;
use core::iter::FromIterator;
use core::mem;
use core::pin::Pin;
use core::task::{Context, Poll};
use alloc::boxed::Box;
use alloc::vec::Vec;

use super::{MaybeDone, assert_future};

fn iter_pin_mut<T>(slice: Pin<&mut [T]>) -> impl Iterator<Item = Pin<&mut T>> {
    // Safety: `std` _could_ make this unsound if it were to decide Pin's
    // invariants aren't required to transmit through slices. Otherwise this has
    // the same safety as a normal field pin projection.
    unsafe { slice.get_unchecked_mut() }
        .iter_mut()
        .map(|t| unsafe { Pin::new_unchecked(t) })
}

/// Future for the [`join_all`] function.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct JoinAll<F>
where
    F: Future,
{
    elems: Pin<Box<[MaybeDone<F>]>>,
}

impl<F> fmt::Debug for JoinAll<F>
where
    F: Future + fmt::Debug,
    F::Output: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinAll")
            .field("elems", &self.elems)
            .finish()
    }
}

/// Creates a future which represents a collection of the outputs of the futures
/// given.
///
/// The returned future will drive execution for all of its underlying futures,
/// collecting the results into a destination `Vec<T>` in the same order as they
/// were provided.
///
/// This function is only available when the `std` or `alloc` feature of this
/// library is activated, and it is activated by default.
///
/// # See Also
///
/// This is purposefully a very simple API for basic use-cases. In a lot of
/// cases you will want to use the more powerful
/// [`FuturesOrdered`][crate::stream::FuturesOrdered] APIs, or, if order does
/// not matter, [`FuturesUnordered`][crate::stream::FuturesUnordered].
///
/// Some examples for additional functionality provided by these are:
///
///  * Adding new futures to the set even after it has been started.
///
///  * Only polling the specific futures that have been woken. In cases where
///    you have a lot of futures this will result in much more efficient polling.
///
/// # Examples
///
/// ```
/// # futures::executor::block_on(async {
/// use futures::future::join_all;
///
/// async fn foo(i: u32) -> u32 { i }
///
/// let futures = vec![foo(1), foo(2), foo(3)];
///
/// assert_eq!(join_all(futures).await, [1, 2, 3]);
/// # });
/// ```
pub fn join_all<I>(i: I) -> JoinAll<I::Item>
where
    I: IntoIterator,
    I::Item: Future,
{
    let elems: Box<[_]> = i.into_iter().map(MaybeDone::Future).collect();
    assert_future::<Vec<<I::Item as Future>::Output>, _>(JoinAll { elems: elems.into() })
}

impl<F> Future for JoinAll<F>
where
    F: Future,
{
    type Output = Vec<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut all_done = true;

        for elem in iter_pin_mut(self.elems.as_mut()) {
            if elem.poll(cx).is_pending() {
                all_done = false;
            }
        }

        if all_done {
            let mut elems = mem::replace(&mut self.elems, Box::pin([]));
            let result = iter_pin_mut(elems.as_mut())
                .map(|e| e.take_output().unwrap())
                .collect();
            Poll::Ready(result)
        } else {
            Poll::Pending
        }
    }
}

impl<F: Future> FromIterator<F> for JoinAll<F> {
    fn from_iter<T: IntoIterator<Item = F>>(iter: T) -> Self {
        join_all(iter)
    }
}
