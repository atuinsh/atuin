//! Core traits and types for asynchronous operations in Rust.

#![cfg_attr(feature = "cfg-target-has-atomic", feature(cfg_target_has_atomic))]

#![cfg_attr(not(feature = "std"), no_std)]

#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms, unreachable_pub)]
// It cannot be included in the published code because this lints have false positives in the minimum required version.
#![cfg_attr(test, warn(single_use_lifetimes))]
#![warn(clippy::all)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_assignments, unused_variables))))]

#[cfg(all(feature = "cfg-target-has-atomic", not(feature = "unstable")))]
compile_error!("The `cfg-target-has-atomic` feature requires the `unstable` feature as an explicit opt-in to unstable features");

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod future;
#[doc(hidden)] pub use self::future::{Future, FusedFuture, TryFuture};

pub mod stream;
#[doc(hidden)] pub use self::stream::{Stream, FusedStream, TryStream};

#[macro_use]
pub mod task;

// Not public API.
#[doc(hidden)]
pub mod __private {
    pub use core::task::Poll;
}
