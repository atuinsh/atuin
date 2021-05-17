//! Asynchronous channels.
//!
//! Like threads, concurrent tasks sometimes need to communicate with each
//! other. This module contains two basic abstractions for doing so:
//!
//! - [oneshot], a way of sending a single value from one task to another.
//! - [mpsc], a multi-producer, single-consumer channel for sending values
//!   between tasks, analogous to the similarly-named structure in the standard
//!   library.
//!
//! All items are only available when the `std` or `alloc` feature of this
//! library is activated, and it is activated by default.

#![cfg_attr(feature = "cfg-target-has-atomic", feature(cfg_target_has_atomic))]

#![cfg_attr(not(feature = "std"), no_std)]

#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms, unreachable_pub)]
// It cannot be included in the published code because this lints have false positives in the minimum required version.
#![cfg_attr(test, warn(single_use_lifetimes))]
#![warn(clippy::all)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_assignments, unused_variables))))]

#[cfg(all(feature = "cfg-target-has-atomic", not(feature = "unstable")))]
compile_error!("The `cfg-target-has-atomic` feature requires the `unstable` feature as an explicit opt-in to unstable features");

macro_rules! cfg_target_has_atomic {
    ($($item:item)*) => {$(
        #[cfg_attr(feature = "cfg-target-has-atomic", cfg(target_has_atomic = "ptr"))]
        $item
    )*};
}

cfg_target_has_atomic! {
    #[cfg(feature = "alloc")]
    extern crate alloc;

    #[cfg(feature = "alloc")]
    mod lock;
    #[cfg(feature = "std")]
    pub mod mpsc;
    #[cfg(feature = "alloc")]
    pub mod oneshot;
}
