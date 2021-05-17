// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! The HC-128 random number generator.

use core::fmt;
use rand_core::{CryptoRng, RngCore, SeedableRng, Error, le};
use rand_core::block::{BlockRngCore, BlockRng};

const SEED_WORDS: usize = 8; // 128 bit key followed by 128 bit iv

/// A cryptographically secure random number generator that uses the HC-128
/// algorithm.
///
/// HC-128 is a stream cipher designed by Hongjun Wu[^1], that we use as an
/// RNG. It is selected as one of the "stream ciphers suitable for widespread
/// adoption" by eSTREAM[^2].
///
/// HC-128 is an array based RNG. In this it is similar to RC-4 and ISAAC before
/// it, but those have never been proven cryptographically secure (or have even
/// been significantly compromised, as in the case of RC-4[^5]).
///
/// Because HC-128 works with simple indexing into a large array and with a few
/// operations that parallelize well, it has very good performance. The size of
/// the array it needs, 4kb, can however be a disadvantage.
///
/// This implementation is not based on the version of HC-128 submitted to the
/// eSTREAM contest, but on a later version by the author with a few small
/// improvements from December 15, 2009[^3].
///
/// HC-128 has no known weaknesses that are easier to exploit than doing a
/// brute-force search of 2<sup>128</sup>. A very comprehensive analysis of the
/// current state of known attacks / weaknesses of HC-128 is given in *Some
/// Results On Analysis And Implementation Of HC-128 Stream Cipher*[^4].
///
/// The average cycle length is expected to be
/// 2<sup>1024*32+10-1</sup> = 2<sup>32777</sup>.
/// We support seeding with a 256-bit array, which matches the 128-bit key
/// concatenated with a 128-bit IV from the stream cipher.
///
/// This implementation uses an output buffer of sixteen `u32` words, and uses
/// [`BlockRng`] to implement the [`RngCore`] methods.
///
/// ## References
/// [^1]: Hongjun Wu (2008). ["The Stream Cipher HC-128"](
///       http://www.ecrypt.eu.org/stream/p3ciphers/hc/hc128_p3.pdf).
///       *The eSTREAM Finalists*, LNCS 4986, pp. 39–47, Springer-Verlag.
///
/// [^2]: [eSTREAM: the ECRYPT Stream Cipher Project](
///       http://www.ecrypt.eu.org/stream/)
///
/// [^3]: Hongjun Wu, [Stream Ciphers HC-128 and HC-256](
///       https://www.ntu.edu.sg/home/wuhj/research/hc/index.html)
///
/// [^4]: Shashwat Raizada (January 2015),["Some Results On Analysis And
///       Implementation Of HC-128 Stream Cipher"](
///       http://library.isical.ac.in:8080/jspui/bitstream/123456789/6636/1/TH431.pdf).
///
/// [^5]: Internet Engineering Task Force (February 2015),
///       ["Prohibiting RC4 Cipher Suites"](https://tools.ietf.org/html/rfc7465).
#[derive(Clone, Debug)]
pub struct Hc128Rng(BlockRng<Hc128Core>);

impl RngCore for Hc128Rng {
    #[inline]
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    #[inline]
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest)
    }

    #[inline]
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
        self.0.try_fill_bytes(dest)
    }
}

impl SeedableRng for Hc128Rng {
    type Seed = <Hc128Core as SeedableRng>::Seed;

    #[inline]
    fn from_seed(seed: Self::Seed) -> Self {
        Hc128Rng(BlockRng::<Hc128Core>::from_seed(seed))
    }

    #[inline]
    fn from_rng<R: RngCore>(rng: R) -> Result<Self, Error> {
        BlockRng::<Hc128Core>::from_rng(rng).map(Hc128Rng)
    }
}

impl CryptoRng for Hc128Rng {}

/// The core of `Hc128Rng`, used with `BlockRng`.
#[derive(Clone)]
pub struct Hc128Core {
    t: [u32; 1024],
    counter1024: usize,
}

// Custom Debug implementation that does not expose the internal state
impl fmt::Debug for Hc128Core {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Hc128Core {{}}")
    }
}

impl BlockRngCore for Hc128Core {
    type Item = u32;
    type Results = [u32; 16];

    fn generate(&mut self, results: &mut Self::Results) {
        assert!(self.counter1024 % 16 == 0);

        let cc = self.counter1024 % 512;
        let dd = (cc + 16) % 512;
        let ee = cc.wrapping_sub(16) % 512;

        if self.counter1024 & 512 == 0 {
            // P block
            results[0]  = self.step_p(cc+0,  cc+1,  ee+13, ee+6,  ee+4);
            results[1]  = self.step_p(cc+1,  cc+2,  ee+14, ee+7,  ee+5);
            results[2]  = self.step_p(cc+2,  cc+3,  ee+15, ee+8,  ee+6);
            results[3]  = self.step_p(cc+3,  cc+4,  cc+0,  ee+9,  ee+7);
            results[4]  = self.step_p(cc+4,  cc+5,  cc+1,  ee+10, ee+8);
            results[5]  = self.step_p(cc+5,  cc+6,  cc+2,  ee+11, ee+9);
            results[6]  = self.step_p(cc+6,  cc+7,  cc+3,  ee+12, ee+10);
            results[7]  = self.step_p(cc+7,  cc+8,  cc+4,  ee+13, ee+11);
            results[8]  = self.step_p(cc+8,  cc+9,  cc+5,  ee+14, ee+12);
            results[9]  = self.step_p(cc+9,  cc+10, cc+6,  ee+15, ee+13);
            results[10] = self.step_p(cc+10, cc+11, cc+7,  cc+0,  ee+14);
            results[11] = self.step_p(cc+11, cc+12, cc+8,  cc+1,  ee+15);
            results[12] = self.step_p(cc+12, cc+13, cc+9,  cc+2,  cc+0);
            results[13] = self.step_p(cc+13, cc+14, cc+10, cc+3,  cc+1);
            results[14] = self.step_p(cc+14, cc+15, cc+11, cc+4,  cc+2);
            results[15] = self.step_p(cc+15, dd+0,  cc+12, cc+5,  cc+3);
        } else {
            // Q block
            results[0]  = self.step_q(cc+0,  cc+1,  ee+13, ee+6,  ee+4);
            results[1]  = self.step_q(cc+1,  cc+2,  ee+14, ee+7,  ee+5);
            results[2]  = self.step_q(cc+2,  cc+3,  ee+15, ee+8,  ee+6);
            results[3]  = self.step_q(cc+3,  cc+4,  cc+0,  ee+9,  ee+7);
            results[4]  = self.step_q(cc+4,  cc+5,  cc+1,  ee+10, ee+8);
            results[5]  = self.step_q(cc+5,  cc+6,  cc+2,  ee+11, ee+9);
            results[6]  = self.step_q(cc+6,  cc+7,  cc+3,  ee+12, ee+10);
            results[7]  = self.step_q(cc+7,  cc+8,  cc+4,  ee+13, ee+11);
            results[8]  = self.step_q(cc+8,  cc+9,  cc+5,  ee+14, ee+12);
            results[9]  = self.step_q(cc+9,  cc+10, cc+6,  ee+15, ee+13);
            results[10] = self.step_q(cc+10, cc+11, cc+7,  cc+0,  ee+14);
            results[11] = self.step_q(cc+11, cc+12, cc+8,  cc+1,  ee+15);
            results[12] = self.step_q(cc+12, cc+13, cc+9,  cc+2,  cc+0);
            results[13] = self.step_q(cc+13, cc+14, cc+10, cc+3,  cc+1);
            results[14] = self.step_q(cc+14, cc+15, cc+11, cc+4,  cc+2);
            results[15] = self.step_q(cc+15, dd+0,  cc+12, cc+5,  cc+3);
        }
        self.counter1024 = self.counter1024.wrapping_add(16);
    }
}

impl Hc128Core {
    // One step of HC-128, update P and generate 32 bits keystream
    #[inline(always)]
    fn step_p(&mut self, i: usize, i511: usize, i3: usize, i10: usize, i12: usize)
         -> u32
    {
        let (p, q) = self.t.split_at_mut(512);
        // FIXME: it would be great if we the bounds checks here could be
        // optimized out, and we would not need unsafe.
        // This improves performance by about 7%.
        unsafe {
            let temp0 = p.get_unchecked(i511).rotate_right(23);
            let temp1 = p.get_unchecked(i3).rotate_right(10);
            let temp2 = p.get_unchecked(i10).rotate_right(8);
            *p.get_unchecked_mut(i) = p.get_unchecked(i)
                                       .wrapping_add(temp2)
                                       .wrapping_add(temp0 ^ temp1);
            let temp3 = {
                // The h1 function in HC-128
                let a = *p.get_unchecked(i12) as u8;
                let c = (p.get_unchecked(i12) >> 16) as u8;
                q[a as usize].wrapping_add(q[256 + c as usize])
            };
            temp3 ^ p.get_unchecked(i)
        }
    }

    // One step of HC-128, update Q and generate 32 bits keystream
    // Similar to `step_p`, but `p` and `q` are swapped, and the rotates are to
    // the left instead of to the right.
    #[inline(always)]
    fn step_q(&mut self, i: usize, i511: usize, i3: usize, i10: usize, i12: usize)
         -> u32
    {
        let (p, q) = self.t.split_at_mut(512);
        unsafe {
            let temp0 = q.get_unchecked(i511).rotate_left(23);
            let temp1 = q.get_unchecked(i3).rotate_left(10);
            let temp2 = q.get_unchecked(i10).rotate_left(8);
            *q.get_unchecked_mut(i) = q.get_unchecked(i)
                                       .wrapping_add(temp2)
                                       .wrapping_add(temp0 ^ temp1);
            let temp3 = {
                // The h2 function in HC-128
                let a = *q.get_unchecked(i12) as u8;
                let c = (q.get_unchecked(i12) >> 16) as u8;
                p[a as usize].wrapping_add(p[256 + c as usize])
            };
            temp3 ^ q.get_unchecked(i)
        }
    }

    fn sixteen_steps(&mut self) {
        assert!(self.counter1024 % 16 == 0);

        let cc = self.counter1024 % 512;
        let dd = (cc + 16) % 512;
        let ee = cc.wrapping_sub(16) % 512;

        if self.counter1024 < 512 {
            // P block
            self.t[cc+0]  = self.step_p(cc+0,  cc+1,  ee+13, ee+6,  ee+4);
            self.t[cc+1]  = self.step_p(cc+1,  cc+2,  ee+14, ee+7,  ee+5);
            self.t[cc+2]  = self.step_p(cc+2,  cc+3,  ee+15, ee+8,  ee+6);
            self.t[cc+3]  = self.step_p(cc+3,  cc+4,  cc+0,  ee+9,  ee+7);
            self.t[cc+4]  = self.step_p(cc+4,  cc+5,  cc+1,  ee+10, ee+8);
            self.t[cc+5]  = self.step_p(cc+5,  cc+6,  cc+2,  ee+11, ee+9);
            self.t[cc+6]  = self.step_p(cc+6,  cc+7,  cc+3,  ee+12, ee+10);
            self.t[cc+7]  = self.step_p(cc+7,  cc+8,  cc+4,  ee+13, ee+11);
            self.t[cc+8]  = self.step_p(cc+8,  cc+9,  cc+5,  ee+14, ee+12);
            self.t[cc+9]  = self.step_p(cc+9,  cc+10, cc+6,  ee+15, ee+13);
            self.t[cc+10] = self.step_p(cc+10, cc+11, cc+7,  cc+0,  ee+14);
            self.t[cc+11] = self.step_p(cc+11, cc+12, cc+8,  cc+1,  ee+15);
            self.t[cc+12] = self.step_p(cc+12, cc+13, cc+9,  cc+2,  cc+0);
            self.t[cc+13] = self.step_p(cc+13, cc+14, cc+10, cc+3,  cc+1);
            self.t[cc+14] = self.step_p(cc+14, cc+15, cc+11, cc+4,  cc+2);
            self.t[cc+15] = self.step_p(cc+15, dd+0,  cc+12, cc+5,  cc+3);
        } else {
            // Q block
            self.t[cc+512+0]  = self.step_q(cc+0,  cc+1,  ee+13, ee+6,  ee+4);
            self.t[cc+512+1]  = self.step_q(cc+1,  cc+2,  ee+14, ee+7,  ee+5);
            self.t[cc+512+2]  = self.step_q(cc+2,  cc+3,  ee+15, ee+8,  ee+6);
            self.t[cc+512+3]  = self.step_q(cc+3,  cc+4,  cc+0,  ee+9,  ee+7);
            self.t[cc+512+4]  = self.step_q(cc+4,  cc+5,  cc+1,  ee+10, ee+8);
            self.t[cc+512+5]  = self.step_q(cc+5,  cc+6,  cc+2,  ee+11, ee+9);
            self.t[cc+512+6]  = self.step_q(cc+6,  cc+7,  cc+3,  ee+12, ee+10);
            self.t[cc+512+7]  = self.step_q(cc+7,  cc+8,  cc+4,  ee+13, ee+11);
            self.t[cc+512+8]  = self.step_q(cc+8,  cc+9,  cc+5,  ee+14, ee+12);
            self.t[cc+512+9]  = self.step_q(cc+9,  cc+10, cc+6,  ee+15, ee+13);
            self.t[cc+512+10] = self.step_q(cc+10, cc+11, cc+7,  cc+0,  ee+14);
            self.t[cc+512+11] = self.step_q(cc+11, cc+12, cc+8,  cc+1,  ee+15);
            self.t[cc+512+12] = self.step_q(cc+12, cc+13, cc+9,  cc+2,  cc+0);
            self.t[cc+512+13] = self.step_q(cc+13, cc+14, cc+10, cc+3,  cc+1);
            self.t[cc+512+14] = self.step_q(cc+14, cc+15, cc+11, cc+4,  cc+2);
            self.t[cc+512+15] = self.step_q(cc+15, dd+0,  cc+12, cc+5,  cc+3);
        }
        self.counter1024 += 16;
    }

    // Initialize an HC-128 random number generator. The seed has to be
    // 256 bits in length (`[u32; 8]`), matching the 128 bit `key` followed by
    // 128 bit `iv` when HC-128 where to be used as a stream cipher.
    #[inline(always)]   // single use: SeedableRng::from_seed
    fn init(seed: [u32; SEED_WORDS]) -> Self {
        #[inline]
        fn f1(x: u32) -> u32 {
            x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3)
        }

        #[inline]
        fn f2(x: u32) -> u32 {
            x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10)
        }

        let mut t = [0u32; 1024];

        // Expand the key and iv into P and Q
        let (key, iv) = seed.split_at(4);
        t[..4].copy_from_slice(key);
        t[4..8].copy_from_slice(key);
        t[8..12].copy_from_slice(iv);
        t[12..16].copy_from_slice(iv);

        // Generate the 256 intermediate values W[16] ... W[256+16-1], and
        // copy the last 16 generated values to the start op P.
        for i in 16..256+16 {
            t[i] = f2(t[i-2]).wrapping_add(t[i-7]).wrapping_add(f1(t[i-15]))
                   .wrapping_add(t[i-16]).wrapping_add(i as u32);
        }
        {
            let (p1, p2) = t.split_at_mut(256);
            p1[0..16].copy_from_slice(&p2[0..16]);
        }

        // Generate both the P and Q tables
        for i in 16..1024 {
            t[i] = f2(t[i-2]).wrapping_add(t[i-7]).wrapping_add(f1(t[i-15]))
                   .wrapping_add(t[i-16]).wrapping_add(256 + i as u32);
        }

        let mut core = Self { t, counter1024: 0 };

        // run the cipher 1024 steps
        for _ in 0..64 { core.sixteen_steps() };
        core.counter1024 = 0;
        core
    }
}

impl SeedableRng for Hc128Core {
    type Seed = [u8; SEED_WORDS*4];

    /// Create an HC-128 random number generator with a seed. The seed has to be
    /// 256 bits in length, matching the 128 bit `key` followed by 128 bit `iv`
    /// when HC-128 where to be used as a stream cipher.
    fn from_seed(seed: Self::Seed) -> Self {
        let mut seed_u32 = [0u32; SEED_WORDS];
        le::read_u32_into(&seed, &mut seed_u32);
        Self::init(seed_u32)
    }
}

impl CryptoRng for Hc128Core {}

#[cfg(test)]
mod test {
    use ::rand_core::{RngCore, SeedableRng};
    use super::Hc128Rng;

    #[test]
    // Test vector 1 from the paper "The Stream Cipher HC-128"
    fn test_hc128_true_values_a() {
        let seed = [0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0, // key
                    0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0]; // iv
        let mut rng = Hc128Rng::from_seed(seed);

        let mut results = [0u32; 16];
        for i in results.iter_mut() { *i = rng.next_u32(); }
        let expected = [0x73150082, 0x3bfd03a0, 0xfb2fd77f, 0xaa63af0e,
                        0xde122fc6, 0xa7dc29b6, 0x62a68527, 0x8b75ec68,
                        0x9036db1e, 0x81896005, 0x00ade078, 0x491fbf9a,
                        0x1cdc3013, 0x6c3d6e24, 0x90f664b2, 0x9cd57102];
        assert_eq!(results, expected);
    }

    #[test]
    // Test vector 2 from the paper "The Stream Cipher HC-128"
    fn test_hc128_true_values_b() {
        let seed = [0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0, // key
                    1,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0]; // iv
        let mut rng = Hc128Rng::from_seed(seed);

        let mut results = [0u32; 16];
        for i in results.iter_mut() { *i = rng.next_u32(); }
        let expected = [0xc01893d5, 0xb7dbe958, 0x8f65ec98, 0x64176604,
                        0x36fc6724, 0xc82c6eec, 0x1b1c38a7, 0xc9b42a95,
                        0x323ef123, 0x0a6a908b, 0xce757b68, 0x9f14f7bb,
                        0xe4cde011, 0xaeb5173f, 0x89608c94, 0xb5cf46ca];
        assert_eq!(results, expected);
    }

    #[test]
    // Test vector 3 from the paper "The Stream Cipher HC-128"
    fn test_hc128_true_values_c() {
        let seed = [0x55,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0, // key
                    0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0]; // iv
        let mut rng = Hc128Rng::from_seed(seed);

        let mut results = [0u32; 16];
        for i in results.iter_mut() { *i = rng.next_u32(); }
        let expected = [0x518251a4, 0x04b4930a, 0xb02af931, 0x0639f032,
                        0xbcb4a47a, 0x5722480b, 0x2bf99f72, 0xcdc0e566,
                        0x310f0c56, 0xd3cc83e8, 0x663db8ef, 0x62dfe07f,
                        0x593e1790, 0xc5ceaa9c, 0xab03806f, 0xc9a6e5a0];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_hc128_true_values_u64() {
        let seed = [0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0, // key
                    0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0]; // iv
        let mut rng = Hc128Rng::from_seed(seed);

        let mut results = [0u64; 8];
        for i in results.iter_mut() { *i = rng.next_u64(); }
        let expected = [0x3bfd03a073150082, 0xaa63af0efb2fd77f,
                        0xa7dc29b6de122fc6, 0x8b75ec6862a68527,
                        0x818960059036db1e, 0x491fbf9a00ade078,
                        0x6c3d6e241cdc3013, 0x9cd5710290f664b2];
        assert_eq!(results, expected);

        // The RNG operates in a P block of 512 results and next a Q block.
        // After skipping 2*800 u32 results we end up somewhere in the Q block
        // of the second round
        for _ in 0..800 { rng.next_u64(); }

        for i in results.iter_mut() { *i = rng.next_u64(); }
        let expected = [0xd8c4d6ca84d0fc10, 0xf16a5d91dc66e8e7,
                        0xd800de5bc37a8653, 0x7bae1f88c0dfbb4c,
                        0x3bfe1f374e6d4d14, 0x424b55676be3fa06,
                        0xe3a1e8758cbff579, 0x417f7198c5652bcd];
        assert_eq!(results, expected);
    }

    #[test]
    fn test_hc128_true_values_bytes() {
        let seed = [0x55,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0, // key
                    0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0]; // iv
        let mut rng = Hc128Rng::from_seed(seed);
        let expected = [0x31, 0xf9, 0x2a, 0xb0, 0x32, 0xf0, 0x39, 0x06,
                 0x7a, 0xa4, 0xb4, 0xbc, 0x0b, 0x48, 0x22, 0x57,
                 0x72, 0x9f, 0xf9, 0x2b, 0x66, 0xe5, 0xc0, 0xcd,
                 0x56, 0x0c, 0x0f, 0x31, 0xe8, 0x83, 0xcc, 0xd3,
                 0xef, 0xb8, 0x3d, 0x66, 0x7f, 0xe0, 0xdf, 0x62,
                 0x90, 0x17, 0x3e, 0x59, 0x9c, 0xaa, 0xce, 0xc5,
                 0x6f, 0x80, 0x03, 0xab, 0xa0, 0xe5, 0xa6, 0xc9,
                 0x60, 0x95, 0x84, 0x7a, 0xa5, 0x68, 0x5a, 0x84,
                 0xea, 0xd5, 0xf3, 0xea, 0x73, 0xa9, 0xad, 0x01,
                 0x79, 0x7d, 0xbe, 0x9f, 0xea, 0xe3, 0xf9, 0x74,
                 0x0e, 0xda, 0x2f, 0xa0, 0xe4, 0x7b, 0x4b, 0x1b,
                 0xdd, 0x17, 0x69, 0x4a, 0xfe, 0x9f, 0x56, 0x95,
                 0xad, 0x83, 0x6b, 0x9d, 0x60, 0xa1, 0x99, 0x96,
                 0x90, 0x00, 0x66, 0x7f, 0xfa, 0x7e, 0x65, 0xe9,
                 0xac, 0x8b, 0x92, 0x34, 0x77, 0xb4, 0x23, 0xd0,
                 0xb9, 0xab, 0xb1, 0x47, 0x7d, 0x4a, 0x13, 0x0a];

        // Pick a somewhat large buffer so we can test filling with the
        // remainder from `state.results`, directly filling the buffer, and
        // filling the remainder of the buffer.
        let mut buffer = [0u8; 16*4*2];
        // Consume a value so that we have a remainder.
        assert!(rng.next_u64() == 0x04b4930a518251a4);
        rng.fill_bytes(&mut buffer);

        // [u8; 128] doesn't implement PartialEq
        assert_eq!(buffer.len(), expected.len());
        for (b, e) in buffer.iter().zip(expected.iter()) {
            assert_eq!(b, e);
        }
    }

    #[test]
    fn test_hc128_clone() {
        let seed = [0x55,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0, // key
                    0,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,0]; // iv
        let mut rng1 = Hc128Rng::from_seed(seed);
        let mut rng2 = rng1.clone();
        for _ in 0..16 {
            assert_eq!(rng1.next_u32(), rng2.next_u32());
        }
    }
}
