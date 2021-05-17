// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Implementation for CloudABI
use crate::Error;
use core::num::NonZeroU32;

extern "C" {
    fn cloudabi_sys_random_get(buf: *mut u8, buf_len: usize) -> u16;
}

pub fn getrandom_inner(dest: &mut [u8]) -> Result<(), Error> {
    let errno = unsafe { cloudabi_sys_random_get(dest.as_mut_ptr(), dest.len()) };
    if let Some(code) = NonZeroU32::new(errno as u32) {
        error!("cloudabi_sys_random_get: failed with {}", errno);
        Err(Error::from(code))
    } else {
        Ok(()) // Zero means success for CloudABI
    }
}
