//! Task notification.

#[macro_use]
mod poll;

#[doc(hidden)]
pub mod __internal;

#[doc(no_inline)]
pub use core::task::{Context, Poll, Waker, RawWaker, RawWakerVTable};
