//! The futures-rs procedural macro implementations.

#![recursion_limit = "128"]
#![warn(rust_2018_idioms, unreachable_pub)]
// It cannot be included in the published code because this lints have false positives in the minimum required version.
#![cfg_attr(test, warn(single_use_lifetimes))]
#![warn(clippy::all)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_assignments, unused_variables))))]

// Since https://github.com/rust-lang/cargo/pull/7700 `proc_macro` is part of the prelude for
// proc-macro crates, but to support older compilers we still need this explicit `extern crate`.
#[allow(unused_extern_crates)]
extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro_hack::proc_macro_hack;

mod join;
mod select;

/// The `join!` macro.
#[proc_macro_hack]
pub fn join_internal(input: TokenStream) -> TokenStream {
    crate::join::join(input)
}

/// The `try_join!` macro.
#[proc_macro_hack]
pub fn try_join_internal(input: TokenStream) -> TokenStream {
    crate::join::try_join(input)
}

/// The `select!` macro.
#[proc_macro_hack]
pub fn select_internal(input: TokenStream) -> TokenStream {
    crate::select::select(input)
}

/// The `select_biased!` macro.
#[proc_macro_hack]
pub fn select_biased_internal(input: TokenStream) -> TokenStream {
    crate::select::select_biased(input)
}
