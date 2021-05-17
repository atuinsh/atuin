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

use crate::prelude::*;

pub const fn new() -> Uuid {
    Uuid::from_bytes([
        0xF9, 0x16, 0x8C, 0x5E, 0xCE, 0xB2, 0x4F, 0xAA, 0xB6, 0xBF, 0x32, 0x9B,
        0xF3, 0x9F, 0xA1, 0xE4,
    ])
}

pub const fn new2() -> Uuid {
    Uuid::from_bytes([
        0xF9, 0x16, 0x8C, 0x5E, 0xCE, 0xB2, 0x4F, 0xAB, 0xB6, 0xBF, 0x32, 0x9B,
        0xF3, 0x9F, 0xA1, 0xE4,
    ])
}
