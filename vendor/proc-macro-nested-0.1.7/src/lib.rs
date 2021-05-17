//! Support for nested invocations of proc-macro-hack expression macros.
//!
//! By default, macros defined through proc-macro-hack do not support nested
//! invocations, i.e. the code emitted by a proc-macro-hack macro invocation
//! cannot contain recursive calls to the same proc-macro-hack macro nor calls
//! to any other proc-macro-hack macros.
//!
//! This crate provides opt-in support for such nested invocations.
//!
//! To make a macro callable recursively, add a dependency on this crate from
//! your declaration crate and update the `#[proc_macro_hack]` re-export as
//! follows.
//!
//! ```
//! // Before
//! # const IGNORE: &str = stringify! {
//! #[proc_macro_hack]
//! pub use demo_hack_impl::add_one;
//! # };
//! ```
//!
//! ```
//! // After
//! # const IGNORE: &str = stringify! {
//! #[proc_macro_hack(support_nested)]
//! pub use demo_hack_impl::add_one;
//! # };
//! ```
//!
//! No change is required within your definition crate, only to the re-export in
//! the declaration crate.
//!
//! # Limitations
//!
//! - Nested invocations are preprocessed by a TT-muncher, so the caller's crate
//!   will be required to contain `#![recursion_limit = "..."]` if there are
//!   lengthy macro invocations.
//!
//! - Only up to 64 nested invocations are supported.

#![no_std]

include!(concat!(env!("OUT_DIR"), env!("PATH_SEPARATOR"), "count.rs"));

#[doc(hidden)]
#[macro_export]
macro_rules! dispatch {
    (() $($bang:tt)*) => {
        $crate::count!($($bang)*)
    };
    ((($($first:tt)*) $($rest:tt)*) $($bang:tt)*) => {
        $crate::dispatch!(($($first)* $($rest)*) $($bang)*)
    };
    (([$($first:tt)*] $($rest:tt)*) $($bang:tt)*) => {
        $crate::dispatch!(($($first)* $($rest)*) $($bang)*)
    };
    (({$($first:tt)*} $($rest:tt)*) $($bang:tt)*) => {
        $crate::dispatch!(($($first)* $($rest)*) $($bang)*)
    };
    ((! $($rest:tt)*) $($bang:tt)*) => {
        $crate::dispatch!(($($rest)*) $($bang)* !)
    };
    ((!= $($rest:tt)*) $($bang:tt)*) => {
        $crate::dispatch!(($($rest)*) $($bang)* !)
    };
    (($first:tt $($rest:tt)*) $($bang:tt)*) => {
        $crate::dispatch!(($($rest)*) $($bang)*)
    };
}
