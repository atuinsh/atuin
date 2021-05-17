// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Implementation for Windows UWP targets. After deprecation of Windows XP
//! and Vista, this can supersede the `RtlGenRandom`-based implementation.
use crate::Error;
use core::{ffi::c_void, num::NonZeroU32, ptr};

const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x00000002;

extern "system" {
    fn BCryptGenRandom(
        hAlgorithm: *mut c_void,
        pBuffer: *mut u8,
        cbBuffer: u32,
        dwFlags: u32,
    ) -> u32;
}

pub fn getrandom_inner(dest: &mut [u8]) -> Result<(), Error> {
    // Prevent overflow of u32
    for chunk in dest.chunks_mut(u32::max_value() as usize) {
        let ret = unsafe {
            BCryptGenRandom(
                ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        // NTSTATUS codes use two highest bits for severity status
        match ret >> 30 {
            0b01 => {
                info!("BCryptGenRandom: information code 0x{:08X}", ret);
            }
            0b10 => {
                warn!("BCryptGenRandom: warning code 0x{:08X}", ret);
            }
            0b11 => {
                error!("BCryptGenRandom: failed with 0x{:08X}", ret);
                // We zeroize the highest bit, so the error code will reside
                // inside the range of designated for OS codes.
                let code = ret ^ (1 << 31);
                // SAFETY: the second highest bit is always equal to one,
                // so it's impossible to get zero. Unfortunately compiler
                // is not smart enough to figure out it yet.
                let code = unsafe { NonZeroU32::new_unchecked(code) };
                return Err(Error::from(code));
            }
            _ => (),
        }
    }
    Ok(())
}
