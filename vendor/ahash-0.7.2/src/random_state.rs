#[cfg(all(feature = "runtime-rng", not(all(feature = "compile-time-rng", test))))]
use crate::convert::Convert;
#[cfg(feature = "specialize")]
use crate::BuildHasherExt;

#[cfg(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes", not(miri)))]
pub use crate::aes_hash::*;

#[cfg(not(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes", not(miri))))]
pub use crate::fallback_hash::*;

#[cfg(all(feature = "compile-time-rng", any(not(feature = "runtime-rng"), test)))]
use const_random::const_random;
use core::fmt;
use core::hash::BuildHasher;
#[cfg(feature = "specialize")]
use core::hash::Hash;
use core::hash::Hasher;

#[cfg(not(feature = "std"))]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std as alloc;


#[cfg(all(feature = "runtime-rng", not(all(feature = "compile-time-rng", test))))]
use alloc::boxed::Box;
use core::sync::atomic::{AtomicUsize, Ordering};
#[cfg(all(feature = "runtime-rng", not(all(feature = "compile-time-rng", test))))]
use once_cell::race::OnceBox;

#[cfg(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes", not(miri)))]
use crate::aes_hash::*;
#[cfg(not(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes", not(miri))))]
use crate::fallback_hash::*;

#[cfg(all(feature = "runtime-rng", not(all(feature = "compile-time-rng", test))))]
static SEEDS: OnceBox<[[u64; 4]; 2]> = OnceBox::new();

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) const PI: [u64; 4] = [
    0x243f_6a88_85a3_08d3,
    0x1319_8a2e_0370_7344,
    0xa409_3822_299f_31d0,
    0x082e_fa98_ec4e_6c89,
];

pub(crate) const PI2: [u64; 4] = [
    0x4528_21e6_38d0_1377,
    0xbe54_66cf_34e9_0c6c,
    0xc0ac_29b7_c97c_50dd,
    0x3f84_d5b5_b547_0917,
];

#[inline]
pub(crate) fn seeds() -> [u64; 4] {
    #[cfg(all(feature = "runtime-rng", not(all(feature = "compile-time-rng", test))))]
    {
        SEEDS.get_or_init(|| {
            let mut result: [u8; 64] = [0; 64];
            getrandom::getrandom(&mut result).expect("getrandom::getrandom() failed.");
            Box::new(result.convert())
        })[1]
    }
    #[cfg(all(feature = "compile-time-rng", any(not(feature = "runtime-rng"), test)))]
    {
        [
            const_random!(u64),
            const_random!(u64),
            const_random!(u64),
            const_random!(u64),
        ]
    }
    #[cfg(all(not(feature = "runtime-rng"), not(feature = "compile-time-rng")))]
    {
        PI
    }
}

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
    /// Use randomly generated keys
    #[inline]
    pub fn new() -> RandomState {
        #[cfg(all(feature = "runtime-rng", not(all(feature = "compile-time-rng", test))))]
        {
            let seeds = SEEDS.get_or_init(|| {
                let mut result: [u8; 64] = [0; 64];
                getrandom::getrandom(&mut result).expect("getrandom::getrandom() failed.");
                Box::new(result.convert())
            });
            RandomState::from_keys(seeds[0], seeds[1])
        }
        #[cfg(all(feature = "compile-time-rng", any(not(feature = "runtime-rng"), test)))]
        {
            RandomState::from_keys(
                [
                    const_random!(u64),
                    const_random!(u64),
                    const_random!(u64),
                    const_random!(u64),
                ],
                [
                    const_random!(u64),
                    const_random!(u64),
                    const_random!(u64),
                    const_random!(u64),
                ],
            )
        }
        #[cfg(all(not(feature = "runtime-rng"), not(feature = "compile-time-rng")))]
        {
            RandomState::from_keys(PI, PI2)
        }
    }

    /// Allows for supplying seeds, but each time it is called the resulting state will be different.
    /// This is done using a static counter, so it can safely be used with a fixed keys.
    #[inline]
    pub fn generate_with(k0: u64, k1: u64, k2: u64, k3: u64) -> RandomState {
        RandomState::from_keys(seeds(), [k0, k1, k2, k3])
    }

    fn from_keys(a: [u64; 4], b: [u64; 4]) -> RandomState {
        let [k0, k1, k2, k3] = a;
        let mut hasher = AHasher::from_random_state(&RandomState { k0, k1, k2, k3 });

        let stack_mem_loc = &hasher as *const _ as usize;
        #[cfg(not(all(target_arch = "arm", target_os = "none")))]
        {
            hasher.write_usize(COUNTER.fetch_add(stack_mem_loc, Ordering::Relaxed));
        }
        #[cfg(all(target_arch = "arm", target_os = "none"))]
        {
            let previous = COUNTER.load(Ordering::Relaxed);
            let new = previous.wrapping_add(stack_mem_loc);
            COUNTER.store(new, Ordering::Relaxed);
            hasher.write_usize(new);
        }
        #[cfg(all(not(feature = "runtime-rng"), not(feature = "compile-time-rng")))]
        hasher.write_usize(&PI as *const _ as usize);
        let mix = |k: u64| {
            let mut h = hasher.clone();
            h.write_u64(k);
            h.finish()
        };

        RandomState {
            k0: mix(b[0]),
            k1: mix(b[1]),
            k2: mix(b[2]),
            k3: mix(b[3]),
        }
    }

    /// Internal. Used by Default.
    #[inline]
    pub(crate) fn with_fixed_keys() -> RandomState {
        let [k0, k1, k2, k3] = seeds();
        RandomState { k0, k1, k2, k3 }
    }

    /// Allows for explicitly setting the seeds to used.
    #[inline]
    pub const fn with_seeds(k0: u64, k1: u64, k2: u64, k3: u64) -> RandomState {
        RandomState { k0: k0 ^ PI2[0], k1: k1 ^ PI2[1], k2: k2 ^ PI2[2], k3: k3 ^ PI2[3] }
    }
}

impl Default for RandomState {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = AHasher;

    /// Constructs a new [AHasher] with keys based on this [RandomState] object.
    /// This means that two different [RandomState]s will will generate
    /// [AHasher]s that will return different hashcodes, but [Hasher]s created from the same [BuildHasher]
    /// will generate the same hashes for the same input data.
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
        AHasher::from_random_state(self)
    }
}

#[cfg(feature = "specialize")]
impl BuildHasherExt for RandomState {
    #[inline]
    fn hash_as_u64<T: Hash + ?Sized>(&self, value: &T) -> u64 {
        let mut hasher = AHasherU64 {
            buffer: self.k0,
            pad: self.k1,
        };
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[inline]
    fn hash_as_fixed_length<T: Hash + ?Sized>(&self, value: &T) -> u64 {
        let mut hasher = AHasherFixed(self.build_hasher());
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[inline]
    fn hash_as_str<T: Hash + ?Sized>(&self, value: &T) -> u64 {
        let mut hasher = AHasherStr(self.build_hasher());
        value.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_unique() {
        let a = RandomState::new();
        let b = RandomState::new();
        assert_ne!(a.build_hasher().finish(), b.build_hasher().finish());
    }

    #[cfg(all(feature = "runtime-rng", not(all(feature = "compile-time-rng", test))))]
    #[test]
    fn test_not_pi() {
        assert_ne!(PI, seeds());
    }

    #[cfg(all(feature = "compile-time-rng", any(not(feature = "runtime-rng"), test)))]
    #[test]
    fn test_not_pi_const() {
        assert_ne!(PI, seeds());
    }

    #[cfg(all(not(feature = "runtime-rng"), not(feature = "compile-time-rng")))]
    #[test]
    fn test_pi() {
        assert_eq!(PI, seeds());
    }

    #[test]
    fn test_with_seeds_const() {
        const _CONST_RANDOM_STATE: RandomState = RandomState::with_seeds(17, 19, 21, 23);
    }
}
