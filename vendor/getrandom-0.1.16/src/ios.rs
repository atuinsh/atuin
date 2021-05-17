// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Implementation for iOS
use crate::{error::SEC_RANDOM_FAILED, Error};

// TODO: Make extern once extern_types feature is stabilized. See:
//   https://github.com/rust-lang/rust/issues/43467
#[repr(C)]
struct SecRandom([u8; 0]);

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecRandomDefault: *const SecRandom;

    fn SecRandomCopyBytes(rnd: *const SecRandom, count: usize, bytes: *mut u8) -> i32;
}

pub fn getrandom_inner(dest: &mut [u8]) -> Result<(), Error> {
    let ret = unsafe { SecRandomCopyBytes(kSecRandomDefault, dest.len(), dest.as_mut_ptr()) };
    if ret == -1 {
        Err(SEC_RANDOM_FAILED)
    } else {
        Ok(())
    }
}
