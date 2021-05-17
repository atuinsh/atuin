// Copyright 2015 The Servo Project Developers. See the
// COPYRIGHT file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Accessor for `Bidi_Class` property from Unicode Character Database (UCD)

mod tables;

pub use self::tables::{BidiClass, UNICODE_VERSION};

use std::cmp::Ordering::{Equal, Less, Greater};
use std::char;

use self::tables::bidi_class_table;
use crate::BidiClass::*;

/// Find the `BidiClass` of a single char.
pub fn bidi_class(c: char) -> BidiClass {
    bsearch_range_value_table(c, bidi_class_table)
}

pub fn is_rtl(bidi_class: BidiClass) -> bool {
    match bidi_class {
        RLE | RLO | RLI => true,
        _ => false,
    }
}

fn bsearch_range_value_table(c: char, r: &'static [(char, char, BidiClass)]) -> BidiClass {
    match r.binary_search_by(|&(lo, hi, _)| if lo <= c && c <= hi {
        Equal
    } else if hi < c {
        Less
    } else {
        Greater
    }) {
        Ok(idx) => {
            let (_, _, cat) = r[idx];
            cat
        }
        // UCD/extracted/DerivedBidiClass.txt: "All code points not explicitly listed
        // for Bidi_Class have the value Left_To_Right (L)."
        Err(_) => L,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii() {
        assert_eq!(bidi_class('\u{0000}'), BN);
        assert_eq!(bidi_class('\u{0040}'), ON);
        assert_eq!(bidi_class('\u{0041}'), L);
        assert_eq!(bidi_class('\u{0062}'), L);
        assert_eq!(bidi_class('\u{007F}'), BN);
    }

    #[test]
    fn test_bmp() {
        // Hebrew
        assert_eq!(bidi_class('\u{0590}'), R);
        assert_eq!(bidi_class('\u{05D0}'), R);
        assert_eq!(bidi_class('\u{05D1}'), R);
        assert_eq!(bidi_class('\u{05FF}'), R);

        // Arabic
        assert_eq!(bidi_class('\u{0600}'), AN);
        assert_eq!(bidi_class('\u{0627}'), AL);
        assert_eq!(bidi_class('\u{07BF}'), AL);

        // Default R + Arabic Extras
        assert_eq!(bidi_class('\u{07C0}'), R);
        assert_eq!(bidi_class('\u{085F}'), R);
        assert_eq!(bidi_class('\u{0860}'), AL);
        assert_eq!(bidi_class('\u{0870}'), R);
        assert_eq!(bidi_class('\u{089F}'), R);
        assert_eq!(bidi_class('\u{08A0}'), AL);
        assert_eq!(bidi_class('\u{089F}'), R);
        assert_eq!(bidi_class('\u{08FF}'), NSM);

        // Default ET
        assert_eq!(bidi_class('\u{20A0}'), ET);
        assert_eq!(bidi_class('\u{20CF}'), ET);

        // Arabic Presentation Forms
        assert_eq!(bidi_class('\u{FB1D}'), R);
        assert_eq!(bidi_class('\u{FB4F}'), R);
        assert_eq!(bidi_class('\u{FB50}'), AL);
        assert_eq!(bidi_class('\u{FDCF}'), AL);
        assert_eq!(bidi_class('\u{FDF0}'), AL);
        assert_eq!(bidi_class('\u{FDFF}'), AL);
        assert_eq!(bidi_class('\u{FE70}'), AL);
        assert_eq!(bidi_class('\u{FEFE}'), AL);
        assert_eq!(bidi_class('\u{FEFF}'), BN);

        // noncharacters
        assert_eq!(bidi_class('\u{FDD0}'), L);
        assert_eq!(bidi_class('\u{FDD1}'), L);
        assert_eq!(bidi_class('\u{FDEE}'), L);
        assert_eq!(bidi_class('\u{FDEF}'), L);
        assert_eq!(bidi_class('\u{FFFE}'), L);
        assert_eq!(bidi_class('\u{FFFF}'), L);
    }

    #[test]
    fn test_smp() {
        // Default AL + R
        assert_eq!(bidi_class('\u{10800}'), R);
        assert_eq!(bidi_class('\u{10FFF}'), R);
        assert_eq!(bidi_class('\u{1E800}'), R);
        assert_eq!(bidi_class('\u{1EDFF}'), R);
        assert_eq!(bidi_class('\u{1EE00}'), AL);
        assert_eq!(bidi_class('\u{1EEFF}'), AL);
        assert_eq!(bidi_class('\u{1EF00}'), R);
        assert_eq!(bidi_class('\u{1EFFF}'), R);
    }

    #[test]
    fn test_unassigned_planes() {
        assert_eq!(bidi_class('\u{30000}'), L);
        assert_eq!(bidi_class('\u{40000}'), L);
        assert_eq!(bidi_class('\u{50000}'), L);
        assert_eq!(bidi_class('\u{60000}'), L);
        assert_eq!(bidi_class('\u{70000}'), L);
        assert_eq!(bidi_class('\u{80000}'), L);
        assert_eq!(bidi_class('\u{90000}'), L);
        assert_eq!(bidi_class('\u{a0000}'), L);
    }
}
