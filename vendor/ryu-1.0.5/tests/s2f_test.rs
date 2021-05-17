// Translated from C to Rust. The original C code can be found at
// https://github.com/ulfjack/ryu and carries the following license:
//
// Copyright 2018 Ulf Adams
//
// The contents of this file may be used under the terms of the Apache License,
// Version 2.0.
//
//    (See accompanying file LICENSE-Apache or copy at
//     http://www.apache.org/licenses/LICENSE-2.0)
//
// Alternatively, the contents of this file may be used under the terms of
// the Boost Software License, Version 1.0.
//    (See accompanying file LICENSE-Boost or copy at
//     https://www.boost.org/LICENSE_1_0.txt)
//
// Unless required by applicable law or agreed to in writing, this software
// is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.

#![allow(dead_code)]

#[path = "../src/common.rs"]
mod common;

#[path = "../src/d2s_full_table.rs"]
mod d2s_full_table;

#[path = "../src/d2s_intrinsics.rs"]
mod d2s_intrinsics;

#[path = "../src/d2s.rs"]
mod d2s;

#[path = "../src/f2s_intrinsics.rs"]
mod f2s_intrinsics;

#[path = "../src/f2s.rs"]
mod f2s;

#[path = "../src/s2f.rs"]
mod s2f;

#[path = "../src/parse.rs"]
mod parse;

use crate::parse::Error;
use crate::s2f::s2f;

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        *self as u8 == *other as u8
    }
}

#[test]
fn test_basic() {
    assert_eq!(0.0, s2f(b"0").unwrap());
    assert_eq!(-0.0, s2f(b"-0").unwrap());
    assert_eq!(1.0, s2f(b"1").unwrap());
    assert_eq!(-1.0, s2f(b"-1").unwrap());
    assert_eq!(123456792.0, s2f(b"123456789").unwrap());
    assert_eq!(299792448.0, s2f(b"299792458").unwrap());
}

#[test]
fn test_min_max() {
    assert_eq!(1e-45, s2f(b"1e-45").unwrap());
    assert_eq!(f32::MIN_POSITIVE, s2f(b"1.1754944e-38").unwrap());
    assert_eq!(f32::MAX, s2f(b"3.4028235e+38").unwrap());
}

#[test]
fn test_mantissa_rounding_overflow() {
    assert_eq!(1.0, s2f(b"0.999999999").unwrap());
    assert_eq!(f32::INFINITY, s2f(b"3.4028236e+38").unwrap());
}
