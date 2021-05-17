// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Implementation for Windows
use crate::{error::RTL_GEN_RANDOM_FAILED, Error};

extern "system" {
    #[link_name = "SystemFunction036"]
    fn RtlGenRandom(RandomBuffer: *mut u8, RandomBufferLength: u32) -> u8;
}

pub fn getrandom_inner(dest: &mut [u8]) -> Result<(), Error> {
    // Prevent overflow of u32
    for chunk in dest.chunks_mut(u32::max_value() as usize) {
        let ret = unsafe { RtlGenRandom(chunk.as_mut_ptr(), chunk.len() as u32) };
        if ret == 0 {
            return Err(RTL_GEN_RANDOM_FAILED);
        }
    }
    Ok(())
}
