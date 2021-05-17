// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Determine if a `char` is a valid identifier for a parser and/or lexer according to
//! [Unicode Standard Annex #31](http://www.unicode.org/reports/tr31/) rules.
//!
//! ```rust
//! extern crate unicode_xid;
//!
//! use unicode_xid::UnicodeXID;
//!
//! fn main() {
//!     let ch = 'a';
//!     println!("Is {} a valid start of an identifier? {}", ch, UnicodeXID::is_xid_start(ch));
//! }
//! ```
//!
//! # features
//!
//! unicode-xid supports a `no_std` feature. This eliminates dependence
//! on std, and instead uses equivalent functions from core.
//!

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc(
    html_logo_url = "https://unicode-rs.github.io/unicode-rs_sm.png",
    html_favicon_url = "https://unicode-rs.github.io/unicode-rs_sm.png"
)]
#![no_std]
#![cfg_attr(feature = "bench", feature(test, unicode_internals))]

#[cfg(test)]
#[macro_use]
extern crate std;

#[cfg(feature = "bench")]
extern crate test;

use tables::derived_property;
pub use tables::UNICODE_VERSION;

mod tables;

#[cfg(test)]
mod tests;

/// Methods for determining if a character is a valid identifier character.
pub trait UnicodeXID {
    /// Returns whether the specified character satisfies the 'XID_Start'
    /// Unicode property.
    ///
    /// 'XID_Start' is a Unicode Derived Property specified in
    /// [UAX #31](http://unicode.org/reports/tr31/#NFKC_Modifications),
    /// mostly similar to ID_Start but modified for closure under NFKx.
    fn is_xid_start(self) -> bool;

    /// Returns whether the specified `char` satisfies the 'XID_Continue'
    /// Unicode property.
    ///
    /// 'XID_Continue' is a Unicode Derived Property specified in
    /// [UAX #31](http://unicode.org/reports/tr31/#NFKC_Modifications),
    /// mostly similar to 'ID_Continue' but modified for closure under NFKx.
    fn is_xid_continue(self) -> bool;
}

impl UnicodeXID for char {
    #[inline]
    fn is_xid_start(self) -> bool {
        derived_property::XID_Start(self)
    }

    #[inline]
    fn is_xid_continue(self) -> bool {
        derived_property::XID_Continue(self)
    }
}
