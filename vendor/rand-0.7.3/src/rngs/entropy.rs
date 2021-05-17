// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Entropy generator, or wrapper around external generators

#![allow(deprecated)] // whole module is deprecated

use crate::rngs::OsRng;
use rand_core::{CryptoRng, Error, RngCore};

/// An interface returning random data from external source(s), provided
/// specifically for securely seeding algorithmic generators (PRNGs).
///
/// This is deprecated. It is suggested you use [`rngs::OsRng`] instead.
///
/// [`rngs::OsRng`]: crate::rngs::OsRng
#[derive(Debug)]
#[deprecated(since = "0.7.0", note = "use rngs::OsRng instead")]
pub struct EntropyRng {
    source: OsRng,
}

impl EntropyRng {
    /// Create a new `EntropyRng`.
    ///
    /// This method will do no system calls or other initialization routines,
    /// those are done on first use. This is done to make `new` infallible,
    /// and `try_fill_bytes` the only place to report errors.
    pub fn new() -> Self {
        EntropyRng { source: OsRng }
    }
}

impl Default for EntropyRng {
    fn default() -> Self {
        EntropyRng::new()
    }
}

impl RngCore for EntropyRng {
    fn next_u32(&mut self) -> u32 {
        self.source.next_u32()
    }

    fn next_u64(&mut self) -> u64 {
        self.source.next_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.source.fill_bytes(dest)
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
        self.source.try_fill_bytes(dest)
    }
}

impl CryptoRng for EntropyRng {}


#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_entropy() {
        let mut rng = EntropyRng::new();
        let n = (rng.next_u32() ^ rng.next_u32()).count_ones();
        assert!(n >= 2); // p(failure) approx 1e-7
    }
}
