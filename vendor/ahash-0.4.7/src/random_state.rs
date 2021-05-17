use crate::convert::Convert;
use crate::AHasher;
use core::fmt;
use core::hash::BuildHasher;
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering;

use crate::operations::folded_multiply;
#[cfg(all(feature = "compile-time-rng", not(test)))]
use const_random::const_random;

///This constant come from Kunth's prng
pub(crate) const MULTIPLE: u64 = 6364136223846793005;
pub(crate) const INCREMENT: u64 = 1442695040888963407;

// Const random provides randomized starting key with no runtime cost.
#[cfg(all(feature = "compile-time-rng", not(test)))]
pub(crate) const INIT_SEED: [u64; 2] = [const_random!(u64), const_random!(u64)];

#[cfg(any(not(feature = "compile-time-rng"), test))]
pub(crate) const INIT_SEED: [u64; 2] = [0x2360_ED05_1FC6_5DA4, 0x4385_DF64_9FCC_F645]; //From PCG-64

#[cfg(all(feature = "compile-time-rng", not(test)))]
static SEED: AtomicUsize = AtomicUsize::new(const_random!(u64) as usize);

#[cfg(any(not(feature = "compile-time-rng"), test))]
static SEED: AtomicUsize = AtomicUsize::new(INCREMENT as usize);

/// Provides a [Hasher] factory. This is typically used (e.g. by [HashMap]) to create
/// [AHasher]s in order to hash the keys of the map. See `build_hasher` below.
///
/// [build_hasher]: ahash::
/// [Hasher]: std::hash::Hasher
/// [BuildHasher]: std::hash::BuildHasher
/// [HashMap]: std::collections::HashMap
#[derive(Clone)]
pub struct RandomState {
    pub(crate) k0: u64,
    pub(crate) k1: u64,
    pub(crate) k2: u64,
    pub(crate) k3: u64,
}

impl fmt::Debug for RandomState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("RandomState { .. }")
    }
}

impl RandomState {
    #[inline]
    pub fn new() -> RandomState {
        //Using a self pointer. When running with ASLR this is a random value.
        let previous = SEED.load(Ordering::Relaxed) as u64;
        let stack_mem_loc = &previous as *const _ as u64;
        //This is similar to the update function in the fallback.
        //only one multiply is needed because memory locations are not under an attackers control.
        let current_seed = previous
            .wrapping_add(stack_mem_loc)
            .wrapping_mul(MULTIPLE)
            .rotate_right(31);
        SEED.store(current_seed as usize, Ordering::Relaxed);
        let (k0, k1, k2, k3) = scramble_keys(&SEED as *const _ as u64, current_seed);
        RandomState { k0, k1, k2, k3 }
    }

    /// Allows for explicitly setting the seeds to used.
    pub const fn with_seeds(k0: u64, k1: u64) -> RandomState {
        let (k0, k1, k2, k3) = scramble_keys(k0, k1);
        RandomState { k0, k1, k2, k3 }
    }
}

/// This is based on the fallback hasher
#[inline]
pub(crate) const fn scramble_keys(a: u64, b: u64) -> (u64, u64, u64, u64) {
    let k1 = folded_multiply(INIT_SEED[0] ^ a, MULTIPLE).wrapping_add(b);
    let k2 = folded_multiply(INIT_SEED[0] ^ b, MULTIPLE).wrapping_add(a);
    let k3 = folded_multiply(INIT_SEED[1] ^ a, MULTIPLE).wrapping_add(b);
    let k4 = folded_multiply(INIT_SEED[1] ^ b, MULTIPLE).wrapping_add(a);
    let combined = folded_multiply(a ^ b, MULTIPLE).wrapping_add(INCREMENT);
    let rot1 = (combined & 63) as u32;
    let rot2 = ((combined >> 16) & 63) as u32;
    let rot3 = ((combined >> 32) & 63) as u32;
    let rot4 = ((combined >> 48) & 63) as u32;
    (
        k1.rotate_left(rot1),
        k2.rotate_left(rot2),
        k3.rotate_left(rot3),
        k4.rotate_left(rot4),
    )
}

impl Default for RandomState {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = AHasher;

    /// Constructs a new [AHasher] with keys based on compile time generated constants** and the location
    /// this object was constructed at in memory. This means that two different [BuildHasher]s will will generate
    /// [AHasher]s that will return different hashcodes, but [Hasher]s created from the same [BuildHasher]
    /// will generate the same hashes for the same input data.
    ///
    /// ** - only if the `compile-time-rng` feature is enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// use ahash::{AHasher, RandomState};
    /// use std::hash::{Hasher, BuildHasher};
    ///
    /// let build_hasher = RandomState::new();
    /// let mut hasher_1 = build_hasher.build_hasher();
    /// let mut hasher_2 = build_hasher.build_hasher();
    ///
    /// hasher_1.write_u32(1234);
    /// hasher_2.write_u32(1234);
    ///
    /// assert_eq!(hasher_1.finish(), hasher_2.finish());
    ///
    /// let other_build_hasher = RandomState::new();
    /// let mut different_hasher = other_build_hasher.build_hasher();
    /// different_hasher.write_u32(1234);
    /// assert_ne!(different_hasher.finish(), hasher_1.finish());
    /// ```
    /// [Hasher]: std::hash::Hasher
    /// [BuildHasher]: std::hash::BuildHasher
    /// [HashMap]: std::collections::HashMap
    #[inline]
    fn build_hasher(&self) -> AHasher {
        AHasher::new_with_keys([self.k0, self.k1].convert(), [self.k2, self.k3].convert())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_const_rand_disabled() {
        assert_eq!(INIT_SEED, [0x2360_ED05_1FC6_5DA4, 0x4385_DF64_9FCC_F645]);
    }

    #[test]
    fn test_with_seeds_const() {
        const _CONST_RANDOM_STATE: RandomState = RandomState::with_seeds(17, 19);
    }
}
