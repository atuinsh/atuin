//! Tools for working with tasks.

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

macro_rules! cfg_target_has_atomic {
    ($($item:item)*) => {$(
        #[cfg_attr(feature = "cfg-target-has-atomic", cfg(target_has_atomic = "ptr"))]
        $item
    )*};
}

mod spawn;
pub use crate::spawn::{Spawn, SpawnError, LocalSpawn};

cfg_target_has_atomic! {
    #[cfg(feature = "alloc")]
    mod arc_wake;
    #[cfg(feature = "alloc")]
    pub use crate::arc_wake::ArcWake;

    #[cfg(feature = "alloc")]
    mod waker;
    #[cfg(feature = "alloc")]
    pub use crate::waker::waker;

    #[cfg(feature = "alloc")]
    mod waker_ref;
    #[cfg(feature = "alloc")]
    pub use crate::waker_ref::{waker_ref, WakerRef};
}

mod future_obj;
pub use crate::future_obj::{FutureObj, LocalFutureObj, UnsafeFutureObj};

mod noop_waker;
pub use crate::noop_waker::noop_waker;
pub use crate::noop_waker::noop_waker_ref;

#[doc(no_inline)]
pub use core::task::{Context, Poll, Waker, RawWaker, RawWakerVTable};
