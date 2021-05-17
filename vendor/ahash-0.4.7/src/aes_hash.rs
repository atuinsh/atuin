use crate::convert::*;
use crate::operations::*;
#[cfg(feature = "specialize")]
use crate::HasherExt;
use core::hash::Hasher;

/// A `Hasher` for hashing an arbitrary stream of bytes.
///
/// Instances of [`AHasher`] represent state that is updated while hashing data.
///
/// Each method updates the internal state based on the new data provided. Once
/// all of the data has been provided, the resulting hash can be obtained by calling
/// `finish()`
///
/// [Clone] is also provided in case you wish to calculate hashes for two different items that
/// start with the same data.
///
#[derive(Debug, Clone)]
pub struct AHasher {
    enc: u128,
    sum: u128,
    key: u128,
}

impl AHasher {
    /// Creates a new hasher keyed to the provided keys.
    ///
    /// Normally hashers are created via `AHasher::default()` for fixed keys or `RandomState::new()` for randomly
    /// generated keys and `RandomState::with_seeds(a,b)` for seeds that are set and can be reused. All of these work at
    /// map creation time (and hence don't have any overhead on a per-item bais).
    ///
    /// This method directly creates the hasher instance and performs no transformation on the provided seeds. This may
    /// be useful where a HashBuilder is not desired, such as for testing purposes.
    ///
    /// # Example
    ///
    /// ```
    /// use std::hash::Hasher;
    /// use ahash::AHasher;
    ///
    /// let mut hasher = AHasher::new_with_keys(1234, 5678);
    ///
    /// hasher.write_u32(1989);
    /// hasher.write_u8(11);
    /// hasher.write_u8(9);
    /// hasher.write(b"Huh?");
    ///
    /// println!("Hash is {:x}!", hasher.finish());
    /// ```
    #[inline]
    pub fn new_with_keys(key1: u128, key2: u128) -> Self {
        Self {
            enc: key1,
            sum: key2,
            key: key1 ^ key2,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_with_keys(key1: u64, key2: u64) -> AHasher {
        use crate::random_state::scramble_keys;
        let (k1, k2, k3, k4) = scramble_keys(key1, key2);
        AHasher {
            enc: [k1, k2].convert(),
            sum: [k3, k4].convert(),
            key: add_by_64s([k1, k2], [k3, k4]).convert(),
        }
    }

    #[inline(always)]
    fn add_in_length(&mut self, length: u64) {
        //This will be scrambled by the next AES round.
        let mut enc: [u64; 2] = self.enc.convert();
        enc[0] = enc[0].wrapping_add(length);
        self.enc = enc.convert();
    }

    #[inline(always)]
    fn hash_in(&mut self, new_value: u128) {
        self.enc = aesenc(self.enc, new_value);
        self.sum = shuffle_and_add(self.sum, new_value);
    }

    #[inline(always)]
    fn hash_in_2(&mut self, v1: u128, v2: u128) {
        self.enc = aesenc(self.enc, v1);
        self.sum = shuffle_and_add(self.sum, v1);
        self.enc = aesenc(self.enc, v2);
        self.sum = shuffle_and_add(self.sum, v2);
    }
}

#[cfg(feature = "specialize")]
impl HasherExt for AHasher {
    #[inline]
    fn hash_u64(self, value: u64) -> u64 {
        let mask = self.sum as u64;
        let rot = (self.enc & 64) as u32;
        folded_multiply(value ^ mask, crate::fallback_hash::MULTIPLE).rotate_left(rot)
    }

    #[inline]
    fn short_finish(&self) -> u64 {
        let buffer: [u64; 2] = self.enc.convert();
        folded_multiply(buffer[0], buffer[1])
    }
}

/// Provides methods to hash all of the primitive types.
impl Hasher for AHasher {
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.write_u64(i as u64);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.write_u64(i as u64);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(i as u64);
    }

    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.hash_in(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.write_u128(i as u128);
    }

    #[inline]
    #[allow(clippy::collapsible_if)]
    fn write(&mut self, input: &[u8]) {
        let mut data = input;
        let length = data.len();
        self.add_in_length(length as u64);
        //A 'binary search' on sizes reduces the number of comparisons.
        if data.len() < 8 {
            let value: [u64; 2] = if data.len() >= 2 {
                if data.len() >= 4 {
                    //len 4-8
                    [data.read_u32().0 as u64, data.read_last_u32() as u64]
                } else {
                    //len 2-3
                    [data.read_u16().0 as u64, data[data.len() - 1] as u64]
                }
            } else {
                if data.len() > 0 {
                    [data[0] as u64, 0]
                } else {
                    [0, 0]
                }
            };
            self.hash_in(value.convert());
        } else {
            if data.len() > 32 {
                if data.len() > 64 {
                    let tail = data.read_last_u128x4();
                    let mut current: [u128; 4] = [self.key; 4];
                    current[0] = aesenc(current[0], tail[0]);
                    current[1] = aesenc(current[1], tail[1]);
                    current[2] = aesenc(current[2], tail[2]);
                    current[3] = aesenc(current[3], tail[3]);
                    let mut sum: [u128; 2] = [self.key, self.key];
                    sum[0] = add_by_64s(sum[0].convert(), tail[0].convert()).convert();
                    sum[1] = add_by_64s(sum[1].convert(), tail[1].convert()).convert();
                    sum[0] = shuffle_and_add(sum[0], tail[2]);
                    sum[1] = shuffle_and_add(sum[1], tail[3]);
                    while data.len() > 64 {
                        let (blocks, rest) = data.read_u128x4();
                        current[0] = aesenc(current[0], blocks[0]);
                        current[1] = aesenc(current[1], blocks[1]);
                        current[2] = aesenc(current[2], blocks[2]);
                        current[3] = aesenc(current[3], blocks[3]);
                        sum[0] = shuffle_and_add(sum[0], blocks[0]);
                        sum[1] = shuffle_and_add(sum[1], blocks[1]);
                        sum[0] = shuffle_and_add(sum[0], blocks[2]);
                        sum[1] = shuffle_and_add(sum[1], blocks[3]);
                        data = rest;
                    }
                    self.hash_in_2(aesenc(current[0], current[1]), aesenc(current[2], current[3]));
                    self.hash_in(add_by_64s(sum[0].convert(), sum[1].convert()).convert());
                } else {
                    //len 33-64
                    let (head, _) = data.read_u128x2();
                    let tail = data.read_last_u128x2();
                    self.hash_in_2(head[0], head[1]);
                    self.hash_in_2(tail[0], tail[1]);
                }
            } else {
                if data.len() > 16 {
                    //len 17-32
                    self.hash_in_2(data.read_u128().0, data.read_last_u128());
                } else {
                    //len 9-16
                    let value: [u64; 2] = [data.read_u64().0, data.read_last_u64()];
                    self.hash_in(value.convert());
                }
            }
        }
    }
    #[inline]
    fn finish(&self) -> u64 {
        let combined = aesdec(self.sum, self.enc);
        let result: [u64; 2] = aesenc(aesenc(combined, self.key), combined).convert();
        result[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::Convert;
    use crate::operations::aesenc;
    use crate::RandomState;
    use std::hash::{BuildHasher, Hasher};
    #[test]
    fn test_sanity() {
        let mut hasher = RandomState::with_seeds(192837465, 1234567890).build_hasher();
        hasher.write_u64(0);
        let h1 = hasher.finish();
        hasher.write(&[1, 0, 0, 0, 0, 0, 0, 0]);
        let h2 = hasher.finish();
        assert_ne!(h1, h2);
    }

    #[cfg(feature = "compile-time-rng")]
    #[test]
    fn test_builder() {
        use std::collections::HashMap;
        use std::hash::BuildHasherDefault;

        let mut map = HashMap::<u32, u64, BuildHasherDefault<AHasher>>::default();
        map.insert(1, 3);
    }

    #[cfg(feature = "compile-time-rng")]
    #[test]
    fn test_default() {
        let hasher_a = AHasher::default();
        let a_enc: [u64; 2] = hasher_a.enc.convert();
        let a_sum: [u64; 2] = hasher_a.sum.convert();
        assert_ne!(0, a_enc[0]);
        assert_ne!(0, a_enc[1]);
        assert_ne!(0, a_sum[0]);
        assert_ne!(0, a_sum[1]);
        assert_ne!(a_enc[0], a_enc[1]);
        assert_ne!(a_sum[0], a_sum[1]);
        assert_ne!(a_enc[0], a_sum[0]);
        assert_ne!(a_enc[1], a_sum[1]);
        let hasher_b = AHasher::default();
        let b_enc: [u64; 2] = hasher_b.enc.convert();
        let b_sum: [u64; 2] = hasher_b.sum.convert();
        assert_eq!(a_enc[0], b_enc[0]);
        assert_eq!(a_enc[1], b_enc[1]);
        assert_eq!(a_sum[0], b_sum[0]);
        assert_eq!(a_sum[1], b_sum[1]);
    }

    #[test]
    fn test_hash() {
        let mut result: [u64; 2] = [0x6c62272e07bb0142, 0x62b821756295c58d];
        let value: [u64; 2] = [1 << 32, 0xFEDCBA9876543210];
        result = aesenc(value.convert(), result.convert()).convert();
        result = aesenc(result.convert(), result.convert()).convert();
        let mut result2: [u64; 2] = [0x6c62272e07bb0142, 0x62b821756295c58d];
        let value2: [u64; 2] = [1, 0xFEDCBA9876543210];
        result2 = aesenc(value2.convert(), result2.convert()).convert();
        result2 = aesenc(result2.convert(), result.convert()).convert();
        let result: [u8; 16] = result.convert();
        let result2: [u8; 16] = result2.convert();
        assert_ne!(hex::encode(result), hex::encode(result2));
    }

    #[test]
    fn test_conversion() {
        let input: &[u8] = "dddddddd".as_bytes();
        let bytes: u64 = as_array!(input, 8).convert();
        assert_eq!(bytes, 0x6464646464646464);
    }
}
