//! This crate provides type-level numbers evaluated at compile time. It depends only on libcore.
//!
//! The traits defined or used in this crate are used in a typical manner. They can be divided into
//! two categories: **marker traits** and **type operators**.
//!
//! Many of the marker traits have functions defined, but they all do essentially the same thing:
//! convert a type into its runtime counterpart, and are really just there for debugging. For
//! example,
//!
//! ```rust
//! use typenum::{Integer, N4};
//!
//! assert_eq!(N4::to_i32(), -4);
//! ```
//!
//! **Type operators** are traits that behave as functions at the type level. These are the meat of
//! this library. Where possible, traits defined in libcore have been used, but their attached
//! functions have not been implemented.
//!
//! For example, the `Add` trait is implemented for both unsigned and signed integers, but the
//! `add` function is not. As there are never any objects of the types defined here, it wouldn't
//! make sense to implement it. What is important is its associated type `Output`, which is where
//! the addition happens.
//!
//! ```rust
//! use std::ops::Add;
//! use typenum::{Integer, P3, P4};
//!
//! type X = <P3 as Add<P4>>::Output;
//! assert_eq!(<X as Integer>::to_i32(), 7);
//! ```
//!
//! In addition, helper aliases are defined for type operators. For example, the above snippet
//! could be replaced with
//!
//! ```rust
//! use typenum::{Integer, Sum, P3, P4};
//!
//! type X = Sum<P3, P4>;
//! assert_eq!(<X as Integer>::to_i32(), 7);
//! ```
//!
//! Documented in each module is the full list of type operators implemented.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(feature = "strict", deny(missing_docs))]
#![cfg_attr(feature = "strict", deny(warnings))]
#![cfg_attr(
    feature = "cargo-clippy",
    allow(
        clippy::len_without_is_empty,
        clippy::many_single_char_names,
        clippy::new_without_default,
        clippy::suspicious_arithmetic_impl,
        clippy::type_complexity,
        clippy::wrong_self_convention,
    )
)]
#![cfg_attr(feature = "cargo-clippy", deny(clippy::missing_inline_in_public_items))]

// For debugging macros:
// #![feature(trace_macros)]
// trace_macros!(true);

use core::cmp::Ordering;

#[cfg(feature = "force_unix_path_separator")]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/op.rs"));
    include!(concat!(env!("OUT_DIR"), "/consts.rs"));
}

#[cfg(not(feature = "force_unix_path_separator"))]
mod generated {
    include!(env!("TYPENUM_BUILD_OP"));
    include!(env!("TYPENUM_BUILD_CONSTS"));
}

pub mod bit;
pub mod int;
pub mod marker_traits;
pub mod operator_aliases;
pub mod private;
pub mod type_operators;
pub mod uint;

pub mod array;

pub use crate::{
    array::{ATerm, TArr},
    consts::*,
    generated::consts,
    int::{NInt, PInt},
    marker_traits::*,
    operator_aliases::*,
    type_operators::*,
    uint::{UInt, UTerm},
};

/// A potential output from `Cmp`, this is the type equivalent to the enum variant
/// `core::cmp::Ordering::Greater`.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct Greater;

/// A potential output from `Cmp`, this is the type equivalent to the enum variant
/// `core::cmp::Ordering::Less`.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct Less;

/// A potential output from `Cmp`, this is the type equivalent to the enum variant
/// `core::cmp::Ordering::Equal`.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct Equal;

/// Returns `core::cmp::Ordering::Greater`
impl Ord for Greater {
    #[inline]
    fn to_ordering() -> Ordering {
        Ordering::Greater
    }
}

/// Returns `core::cmp::Ordering::Less`
impl Ord for Less {
    #[inline]
    fn to_ordering() -> Ordering {
        Ordering::Less
    }
}

/// Returns `core::cmp::Ordering::Equal`
impl Ord for Equal {
    #[inline]
    fn to_ordering() -> Ordering {
        Ordering::Equal
    }
}

/// Asserts that two types are the same.
#[macro_export]
macro_rules! assert_type_eq {
    ($a:ty, $b:ty) => {
        const _: core::marker::PhantomData<<$a as $crate::Same<$b>>::Output> =
            core::marker::PhantomData;
    };
}

/// Asserts that a type is `True`, aka `B1`.
#[macro_export]
macro_rules! assert_type {
    ($a:ty) => {
        const _: core::marker::PhantomData<<$a as $crate::Same<True>>::Output> =
            core::marker::PhantomData;
    };
}
