//! Miscellaneous tools for concurrent programming.
//!
//! ## Atomics
//!
//! * [`AtomicCell`], a thread-safe mutable memory location.
//! * [`AtomicConsume`], for reading from primitive atomic types with "consume" ordering.
//!
//! ## Thread synchronization
//!
//! * [`Parker`], a thread parking primitive.
//! * [`ShardedLock`], a sharded reader-writer lock with fast concurrent reads.
//! * [`WaitGroup`], for synchronizing the beginning or end of some computation.
//!
//! ## Utilities
//!
//! * [`Backoff`], for exponential backoff in spin loops.
//! * [`CachePadded`], for padding and aligning a value to the length of a cache line.
//! * [`scope`], for spawning threads that borrow local variables from the stack.
//!
//! [`AtomicCell`]: atomic::AtomicCell
//! [`AtomicConsume`]: atomic::AtomicConsume
//! [`Parker`]: sync::Parker
//! [`ShardedLock`]: sync::ShardedLock
//! [`WaitGroup`]: sync::WaitGroup
//! [`scope`]: thread::scope

#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms),
        allow(dead_code, unused_assignments, unused_variables)
    )
))]
#![warn(
    missing_docs,
    missing_debug_implementations,
    rust_2018_idioms,
    unreachable_pub
)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]

#[cfg(crossbeam_loom)]
#[allow(unused_imports)]
mod primitive {
    pub(crate) mod sync {
        pub(crate) mod atomic {
            pub(crate) use loom::sync::atomic::spin_loop_hint;
            pub(crate) use loom::sync::atomic::{
                AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, AtomicUsize,
            };

            // FIXME: loom does not support compiler_fence at the moment.
            // https://github.com/tokio-rs/loom/issues/117
            // we use fence as a stand-in for compiler_fence for the time being.
            // this may miss some races since fence is stronger than compiler_fence,
            // but it's the best we can do for the time being.
            pub(crate) use loom::sync::atomic::fence as compiler_fence;
        }
        pub(crate) use loom::sync::{Arc, Condvar, Mutex};
    }
}
#[cfg(not(crossbeam_loom))]
#[allow(unused_imports)]
mod primitive {
    pub(crate) mod sync {
        pub(crate) mod atomic {
            pub(crate) use core::sync::atomic::compiler_fence;
            // TODO(taiki-e): once we bump the minimum required Rust version to 1.49+,
            // use [`core::hint::spin_loop`] instead.
            #[allow(deprecated)]
            pub(crate) use core::sync::atomic::spin_loop_hint;
            pub(crate) use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize};
            #[cfg(has_atomic_u16)]
            pub(crate) use core::sync::atomic::{AtomicI16, AtomicU16};
            #[cfg(has_atomic_u32)]
            pub(crate) use core::sync::atomic::{AtomicI32, AtomicU32};
            #[cfg(has_atomic_u64)]
            pub(crate) use core::sync::atomic::{AtomicI64, AtomicU64};
            #[cfg(has_atomic_u8)]
            pub(crate) use core::sync::atomic::{AtomicI8, AtomicU8};
        }

        #[cfg(feature = "std")]
        pub(crate) use std::sync::{Arc, Condvar, Mutex};
    }
}

cfg_if! {
    if #[cfg(feature = "alloc")] {
        extern crate alloc;
    } else if #[cfg(feature = "std")] {
        extern crate std as alloc;
    }
}

#[cfg_attr(feature = "nightly", cfg(target_has_atomic = "ptr"))]
pub mod atomic;

mod cache_padded;
pub use crate::cache_padded::CachePadded;

mod backoff;
pub use crate::backoff::Backoff;

use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(feature = "std")] {
        pub mod sync;

        #[cfg(not(crossbeam_loom))]
        pub mod thread;
    }
}
