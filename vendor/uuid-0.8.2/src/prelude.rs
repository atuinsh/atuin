// Copyright 2013-2014 The Rust Project Developers.
// Copyright 2018 The Uuid Project Developers.
//
// See the COPYRIGHT file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The [`uuid`] prelude.
//!
//! This module contains the most important items of the [`uuid`] crate.
//!
//! To use the prelude, include the following in your crate root:
//!
//! ```rust
//! extern crate uuid;
//! ```
//!
//! # Prelude Contents
//!
//! Currently the prelude reexports the following:
//!
//! [`uuid`]`::{`[`Error`], [`Uuid`], [`Variant`], [`Version`],
//! builder::[`Builder`]`}`: The fundamental types used in [`uuid`] crate.
//!
//! [`uuid`]: ../index.html
//! [`Error`]: ../enum.Error.html
//! [`Uuid`]: ../struct.Uuid.html
//! [`Variant`]: ../enum.Variant.html
//! [`Version`]: ../enum.Version.html
//! [`Builder`]: ../builder/struct.Builder.html
//!
#![cfg_attr(feature = "v1",
doc = "
[`uuid::v1`]`::{`[`ClockSequence`],[`Context`]`}`: The types useful for
handling uuid version 1. Requires feature `v1`.

[`uuid::v1`]: ../v1/index.html
[`Context`]: ../v1/struct.Context.html
[`ClockSequence`]: ../v1/trait.ClockSequence.html")]

pub use super::{Builder, Bytes, Error, Uuid, Variant, Version};
#[cfg(feature = "v1")]
pub use crate::v1::{ClockSequence, Context};
