//! # aHash
//!
//! This hashing algorithm is intended to be a high performance, (hardware specific), keyed hash function.
//! This can be seen as a DOS resistant alternative to `FxHash`, or a fast equivalent to `SipHash`.
//! It provides a high speed hash algorithm, but where the result is not predictable without knowing a Key.
//! This allows it to be used in a `HashMap` without allowing for the possibility that an malicious user can
//! induce a collision.
//!
//! # How aHash works
//!
//! aHash uses the hardware AES instruction on x86 processors to provide a keyed hash function.
//! aHash is not a cryptographically secure hash.
#![deny(clippy::correctness, clippy::complexity, clippy::perf)]
#![allow(clippy::pedantic, clippy::cast_lossless, clippy::unreadable_literal)]
#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![cfg_attr(feature = "specialize", feature(specialization))]

#[macro_use]
mod convert;

#[cfg(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes", not(miri)))]
mod aes_hash;
mod fallback_hash;
#[cfg(test)]
mod hash_quality_test;

mod operations;
#[cfg(feature = "std")]
mod hash_map;
#[cfg(feature = "std")]
mod hash_set;
mod random_state;
mod specialize;

#[cfg(feature = "compile-time-rng")]
use const_random::const_random;

#[cfg(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes", not(miri)))]
pub use crate::aes_hash::AHasher;

#[cfg(not(all(any(target_arch = "x86", target_arch = "x86_64"), target_feature = "aes", not(miri))))]
pub use crate::fallback_hash::AHasher;
pub use crate::random_state::RandomState;

pub use crate::specialize::CallHasher;

#[cfg(feature = "std")]
pub use crate::hash_map::AHashMap;
#[cfg(feature = "std")]
pub use crate::hash_set::AHashSet;
use core::hash::Hasher;

/// Provides a default [Hasher] compile time generated constants for keys.
/// This is typically used in conjunction with [BuildHasherDefault] to create
/// [AHasher]s in order to hash the keys of the map.
/// 
/// Generally it is preferable to use [RandomState] instead, so that different
/// hashmaps will have different keys. However if fixed keys are desireable this 
/// may be used instead.
///
/// # Example
/// ```
/// use std::hash::BuildHasherDefault;
/// use ahash::{AHasher, RandomState};
/// use std::collections::HashMap;
///
/// let mut map: HashMap<i32, i32, BuildHasherDefault<AHasher>> = HashMap::default();
/// map.insert(12, 34);
/// ```
///
/// [BuildHasherDefault]: std::hash::BuildHasherDefault
/// [Hasher]: std::hash::Hasher
/// [HashMap]: std::collections::HashMap
impl Default for AHasher {

    /// Constructs a new [AHasher] with compile time generated constants for keys if the 
    /// `compile-time-rng`feature is enabled. Otherwise the keys will be fixed constants.
    /// This means the keys will be the same from one instance to another,
    /// but different from build to the next. So if it is possible for a potential
    /// attacker to have access to the compiled binary it would be better
    /// to specify keys generated at runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// use ahash::AHasher;
    /// use std::hash::Hasher;
    ///
    /// let mut hasher_1 = AHasher::default();
    /// let mut hasher_2 = AHasher::default();
    ///
    /// hasher_1.write_u32(1234);
    /// hasher_2.write_u32(1234);
    ///
    /// assert_eq!(hasher_1.finish(), hasher_2.finish());
    /// ```
    #[inline]
    #[cfg(feature = "compile-time-rng")]
    fn default() -> AHasher {
        AHasher::new_with_keys(const_random!(u128), const_random!(u128))
    }
    
    /// Constructs a new [AHasher] with compile time generated constants for keys if the 
    /// `compile-time-rng`feature is enabled. Otherwise the keys will be fixed constants.
    /// This means the keys will be the same from one instance to another,
    /// but different from build to the next. So if it is possible for a potential
    /// attacker to have access to the compiled binary it would be better
    /// to specify keys generated at runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// use ahash::AHasher;
    /// use std::hash::Hasher;
    ///
    /// let mut hasher_1 = AHasher::default();
    /// let mut hasher_2 = AHasher::default();
    ///
    /// hasher_1.write_u32(1234);
    /// hasher_2.write_u32(1234);
    ///
    /// assert_eq!(hasher_1.finish(), hasher_2.finish());
    /// ```
    #[inline]
    #[cfg(not(feature = "compile-time-rng"))]
    fn default() -> AHasher {
        const K1: u128 = (random_state::INIT_SEED[0] as u128).wrapping_mul(random_state::MULTIPLE as u128);
        const K2: u128 = (random_state::INIT_SEED[1] as u128).wrapping_mul(random_state::MULTIPLE as u128);
        AHasher::new_with_keys(K1, K2)
    }
}

/// Used for specialization. (Sealed)
pub(crate) trait HasherExt: Hasher {
    #[doc(hidden)]
    fn hash_u64(self, value: u64) -> u64;

    #[doc(hidden)]
    fn short_finish(&self) -> u64;
}

impl<T: Hasher> HasherExt for T {
    #[inline]
    #[cfg(feature = "specialize")]
    default fn hash_u64(self, value: u64) -> u64 {
        value.get_hash(self)
    }
    #[inline]
    #[cfg(not(feature = "specialize"))]
    fn hash_u64(self, value: u64) -> u64 {
        value.get_hash(self)
    }
    #[inline]
    #[cfg(feature = "specialize")]
    default fn short_finish(&self) -> u64 {
        self.finish()
    }
    #[inline]
    #[cfg(not(feature = "specialize"))]
    fn short_finish(&self) -> u64 {
        self.finish()
    }
}

// #[inline(never)]
// #[doc(hidden)]
// pub fn hash_test(input: &[u8]) -> u64 {
//     let a = AHasher::new_with_keys(11111111111_u128, 2222222222_u128);
//     input.get_hash(a)
// }

#[cfg(test)]
mod test {
    use crate::convert::Convert;
    use crate::*;
    use std::collections::HashMap;

    #[cfg(feature = "std")]
    #[test]
    fn test_default_builder() {
        use core::hash::BuildHasherDefault;

        let mut map = HashMap::<u32, u64, BuildHasherDefault<AHasher>>::default();
        map.insert(1, 3);
    }
    #[test]
    fn test_builder() {
        let mut map = HashMap::<u32, u64, RandomState>::default();
        map.insert(1, 3);
    }

    #[test]
    fn test_conversion() {
        let input: &[u8] = b"dddddddd";
        let bytes: u64 = as_array!(input, 8).convert();
        assert_eq!(bytes, 0x6464646464646464);
    }

    #[test]
    fn test_ahasher_construction() {
        let _ = AHasher::new_with_keys(1234, 5678);
    }
}
