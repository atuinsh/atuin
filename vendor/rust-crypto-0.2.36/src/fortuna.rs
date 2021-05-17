// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

/*!
 * An implementation of the Fortuna CSPRNG
 *
 * First create a `FortunaRng` object using either the `new_unseeded`
 * constructor or `SeedableRng::from_seed`. Additional entropy may be
 * added using the method `add_random_event`, or the underlying RNG
 * maybe reseeded directly by `SeedableRng::reseed`. Note that this is
 * not recommended, since the generator automatically reseeds itself
 * using the data provided by `add_random_events` through an
 * accumulator. The accumulator is part of Fortuna's design and using
 * `SeedableRng::reseed` directly bypasses it.
 *
 * Note that the underlying block cipher is `AesSafe256Encryptor` which
 * is designed to be timing-attack resistant. The speed hit from this
 * is in line with a "safety first" API, but be aware of it.
 *
 * Fortuna was originally described in
 *   Practical Cryptography, Niels Ferguson and Bruce Schneier.
 *   John Wiley & Sons, 2003.
 *
 * Comments throughout this file contain references of the form
 * (PC 1.2.3); these refer to sections within this text.
 *
 * # A note on forking
 *
 * Proper behaviour for a CSRNG on a process fork is to reseed itself with
 * the timestamp and new process ID, to ensure that after forking the child
 * process does not share the same RNG state (and therefore the same output)
 * as its parent.
 *
 * However, this appears not to be possible in Rust, due to
 *     https://github.com/rust-lang/rust/issues/16799
 * The reason is that Rust's process management all happens through its
 * stdlib runtime, which explicitly does not support forking, so it provides
 * no mechanism with which to detect forks.
 *
 * What this means is that if you are writing forking code (using `#![no_std]`
 * say) then you need to EXPLICITLY RESEED THE RNG AFTER FORKING.
 */

use cryptoutil::copy_memory;

use rand::{Rng, SeedableRng};
use time::precise_time_s;

use aessafe::AesSafe256Encryptor;
use cryptoutil::read_u32_le;
use digest::Digest;
use sha2::Sha256;
use symmetriccipher::BlockEncryptor;

/// Length in bytes that the first pool must be before a "catastrophic
/// reseed" is allowed to happen. (A direct reseed through the
/// `SeedableRng` API is not affected by this limit.)
pub const MIN_POOL_SIZE: usize = 64;
/// Maximum number of bytes to generate before rekeying
const MAX_GEN_SIZE: usize = (1 << 20);
/// Length in bytes of the AES key
const KEY_LEN: usize = 32;
/// Length in bytes of the AES counter
const CTR_LEN: usize = 16;
/// Length in bytes of the AES block
const AES_BLOCK_SIZE: usize = 16;
/// Number of pools used to accumulate entropy
const NUM_POOLS: usize = 32;

/// The underlying PRNG (PC 9.4)
struct FortunaGenerator {
    key: [u8; KEY_LEN],
    ctr: [u8; CTR_LEN],
}

impl FortunaGenerator {
    /// Creates a new generator (PC 9.4.1)
    fn new() -> FortunaGenerator {
        FortunaGenerator {
            key: [0; KEY_LEN],
            ctr: [0; CTR_LEN],
        }
    }

    /// Increments the counter in place
    fn increment_counter(&mut self) {
        for i in 0..self.ctr.len() {
            self.ctr[i] = self.ctr[i].wrapping_add(1);
            // As soon as we don't carry, stop
            if self.ctr[i] != 0 {
                break;
            }
        }
    }

    /// Reseeds the generator (PC 9.4.2)
    fn reseed(&mut self, s: &[u8]) {
        // Compute key as Sha256d( key || s )
        let mut hasher = Sha256::new();
        hasher.input(&self.key[..]);
        hasher.input(s);
        hasher.result(&mut self.key);
        hasher = Sha256::new();
        hasher.input(&self.key[..]);
        hasher.result(&mut self.key[..]);
        // Increment the counter
        self.increment_counter();
    }

    /// Generates some `k` 16-byte blocks of random output (PC 9.4.3)
    /// This should never be used directly, except by `generate_random_data`.
    fn generate_blocks(&mut self, k: usize, out: &mut [u8]) {
        assert!(self.ctr[..] != [0; CTR_LEN][..]);

        // Setup AES encryptor
        let block_encryptor = AesSafe256Encryptor::new(&self.key[..]);
        // Concatenate all the blocks
        for j in 0..k {
            block_encryptor.encrypt_block(&self.ctr[..],
                                          &mut out[AES_BLOCK_SIZE * j..AES_BLOCK_SIZE * (j + 1)]);
            self.increment_counter();
        }
    }

    /// Generates `n` bytes of random data (9.4.4)
    fn generate_random_data(&mut self, out: &mut [u8]) {
        let (n, rem) = (out.len() / AES_BLOCK_SIZE, out.len() % AES_BLOCK_SIZE);
        assert!(n <= MAX_GEN_SIZE);

        // Generate output
        self.generate_blocks(n, &mut out[..(n * AES_BLOCK_SIZE)]);
        if rem > 0 {
            let mut buf = [0; AES_BLOCK_SIZE];
            self.generate_blocks(1, &mut buf);
            copy_memory(&buf[..rem], &mut out[(n * AES_BLOCK_SIZE)..]);
        }

        // Rekey
        let mut new_key = [0; KEY_LEN];
        self.generate_blocks(KEY_LEN / AES_BLOCK_SIZE, &mut new_key);
        self.key = new_key;
    }
}


/// A single entropy pool (not public)
#[derive(Clone, Copy)]
struct Pool {
    state: Sha256,
    count: usize
}

impl Pool {
    fn new() -> Pool {
        Pool { state: Sha256::new(), count: 0 }
    }

    fn input(&mut self, data: &[u8]) {
        self.state.input(data);
        self.count += data.len();
    }

    fn result(&mut self, output: &mut [u8]) {
        self.state.result(output);
        // Double-SHA256 it
        self.state = Sha256::new();
        self.state.input(output);
        self.state.result(output);
        // Clear the pool state
        self.state = Sha256::new();
        self.count = 0;
    }
}

/// The `Fortuna` CSPRNG (PC 9.5)
pub struct Fortuna {
    pool: [Pool; NUM_POOLS],
    generator: FortunaGenerator,
    reseed_count: u32,
    last_reseed_time: f64
}

impl Fortuna {
    /// Creates a new unseeded `Fortuna` (PC 9.5.4)
    pub fn new_unseeded() -> Fortuna {
        Fortuna {
            pool: [Pool::new(); NUM_POOLS],
            generator: FortunaGenerator::new(),
            reseed_count: 0,
            last_reseed_time: 0.0
        }
    }

    /// Adds a random event `e` from source `s` to entropy pool `i` (PC 9.5.6)
    pub fn add_random_event(&mut self, s: u8, i: usize, e: &[u8]) {
        assert!(i <= NUM_POOLS);
        // These restrictions (and `s` in [0, 255]) are part of the Fortuna spec.
        assert!(e.len() > 0);
        assert!(e.len() <= 32);
        (&mut self.pool[i]).input(&[s]);
        (&mut self.pool[i]).input(&[e.len() as u8]);
        (&mut self.pool[i]).input(e);
    }
}

impl Rng for Fortuna {
    /// Generate a bunch of random data into `dest` (PC 9.5.5)
    ///
    /// # Failure modes
    ///
    /// If the RNG has not been seeded, and there is less than
    /// `MIN_POOL_SIZE` bytes of data in the first accumulator
    /// pool, this function will fail the task.
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        // Reseed if necessary
        let now = precise_time_s();
        if self.pool[0].count >= MIN_POOL_SIZE &&
           now - self.last_reseed_time > 0.1 {
            self.reseed_count += 1;
            self.last_reseed_time = now;
            // Compute key as Sha256d( key || s )
            let mut hash = [0; (32 * NUM_POOLS)];
            let mut n_pools = 0;
            while self.reseed_count % (1 << n_pools) == 0 {
                (&mut self.pool[n_pools]).result(&mut hash[n_pools * 32..(n_pools + 1) * 32]);
                n_pools += 1;
                assert!(n_pools < NUM_POOLS);
                assert!(n_pools < 32); // width of counter
            }
            self.generator.reseed(&hash[..n_pools * 32]);
        }
        // Fail on unseeded RNG
        if self.reseed_count == 0 {
            panic!("rust-crypto: an unseeded Fortuna was asked for random bytes!");
        }
        // Generate return data
        for dest in dest.chunks_mut(MAX_GEN_SIZE) {
            self.generator.generate_random_data(dest);
        }
    }

    fn next_u32(&mut self) -> u32 {
        let mut ret = [0; 4];
        self.fill_bytes(&mut ret);
        read_u32_le(&ret[..])
    }
}


impl<'a> SeedableRng<&'a [u8]> for Fortuna {
    fn from_seed(seed: &'a [u8]) -> Fortuna {
        let mut ret = Fortuna::new_unseeded();
        ret.reseed(seed);
        ret
    }

    fn reseed(&mut self, seed: &'a [u8]) {
        self.reseed_count += 1;
        self.last_reseed_time = precise_time_s();
        self.generator.reseed(seed);
    }
}

#[cfg(test)]
fn test_force_reseed(f: &mut Fortuna) {
    f.last_reseed_time -= 0.2;
}

#[cfg(test)]
mod tests {
    use rand::{SeedableRng, Rng};

    use super::{Fortuna, Pool, NUM_POOLS, test_force_reseed};

    #[test]
    fn test_create_unseeded() {
        let _: Fortuna = Fortuna::new_unseeded();
    }

    #[test]
    #[should_panic]
    fn test_use_unseeded() {
        let mut f: Fortuna = Fortuna::new_unseeded();
        let _ = f.next_u32();
    }

    #[test]
    #[should_panic]
    fn test_badly_seeded() {
        let mut f: Fortuna = Fortuna::new_unseeded();
        f.add_random_event(0, 0, &[10; 32]);
        let _ = f.next_u32();
    }

    #[test]
    #[should_panic]
    fn test_too_big_event() {
        let mut f: Fortuna = Fortuna::new_unseeded();
        f.add_random_event(0, 0, &[10; 33]);
    }

    #[test]
    fn test_seeded() {
        // NB for this test I'm just trusting the output of the RNG to be correct.
        // I do check for some high-level features: changing most anything should
        // change the output, there should be no tests, etc.
        let mut f1: Fortuna = SeedableRng::from_seed(&[0, 1, 2, 3, 4, 5][..]);
        assert_eq!(f1.next_u32(), 3369034117);

        let mut f2: Fortuna = Fortuna::new_unseeded();
        f2.reseed(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(f2.next_u32(), 3369034117);

        // Ensure reseeding doesn't totally reset the seed. That is, this output should
        // be different from the above
        let mut f3: Fortuna = Fortuna::new_unseeded();
        f3.reseed(&[0, 1, 2, 3, 4, 5]);
        f3.reseed(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(f3.next_u32(), 2689122182);

        // These three should all be different
        let mut f4: Fortuna = Fortuna::new_unseeded();
        f4.add_random_event(0, 0, &[10; 32]);
        f4.add_random_event(0, 0, &[10; 32]);
        let x = f4.next_u32();

        let mut f5: Fortuna = Fortuna::new_unseeded();
        f5.add_random_event(0, 0, &[10; 32]);
        f5.add_random_event(0, 0, &[20; 32]);
        let y = f5.next_u32();

        let mut f6: Fortuna = Fortuna::new_unseeded();
        f6.add_random_event(0, 0, &[20; 32]);
        f6.add_random_event(0, 0, &[10; 32]);
        let z = f6.next_u32();

        assert!(x != y);
        assert!(y != z);
        assert!(x != z);
    }

    #[test]
    fn test_generator_correctness() {
        let mut output = [0; 100];
        // Expected output as in http://www.seehuhn.de/pages/fortuna
        let expected = [ 82, 254, 233, 139, 254,  85,   6, 222, 222, 149,
                        120,  35, 173,  71,  89, 232,  51, 182, 252, 139,
                        153, 153, 111,  30,  16,   7, 124, 185, 159,  24,
                         50,  68, 236, 107, 133,  18, 217, 219,  46, 134,
                        169, 156, 211,  74, 163,  17, 100, 173,  26,  70,
                        246, 193,  57, 164, 167, 175, 233, 220, 160, 114,
                          2, 200, 215,  80, 207, 218,  85,  58, 235, 117,
                        177, 223,  87, 192,  50, 251,  61,  65, 141, 100,
                         59, 228,  23, 215,  58, 107, 248, 248, 103,  57,
                        127,  31, 241,  91, 230,  33,   0, 164,  77, 46];
        let mut f: Fortuna = SeedableRng::from_seed(&[1, 2, 3, 4][..]);
        f.fill_bytes(&mut output);
        assert_eq!(&expected[..], &output[..]);

        let mut scratch = [0; (1 << 20)];
        f.generator.generate_random_data(&mut scratch);

        let expected = [122, 164,  26,  67, 102,  65,  30, 217, 219, 113,
                         14,  86, 214, 146, 185,  17, 107, 135, 183,   7,
                         18, 162, 126, 206,  46,  38,  54, 172, 248, 194,
                        118,  84, 162, 146,  83, 156, 152,  96, 192,  15,
                         23, 224, 113,  76,  21,   8, 226,  41, 161, 171,
                        197, 180, 138, 236, 126, 137, 101,  25, 219, 225,
                          3, 189,  16, 242,  33,  91,  34,  27,   8, 171,
                        171, 115, 157, 109, 248, 198, 227,  18, 204, 211,
                         42, 184,  92,  42, 171, 222, 198, 117, 162, 134,
                        116, 109,  77, 195, 187, 139,  37,  78, 224,  63];
        f.fill_bytes(&mut output);
        assert_eq!(&expected[..], &output[..]);

        f.reseed(&[5]);

        let expected = [217, 168, 141, 167,  46,   9, 218, 188,  98, 124,
                        109, 128, 242,  22, 189, 120, 180, 124,  15, 192,
                        116, 149, 211, 136, 253, 132,  60,   3,  29, 250,
                         95,  66, 133, 195,  37,  78, 242, 255, 160, 209,
                        185, 106,  68, 105,  83, 145, 165,  72, 179, 167,
                         53, 254, 183, 251, 128,  69,  78, 156, 219,  26,
                        124, 202,  35,   9, 174, 167,  41, 128, 184,  25,
                          2,   1,  63, 142, 205, 162,  69,  68, 207, 251,
                        101,  10,  29,  33, 133,  87, 189,  36, 229,  56,
                         17, 100, 138,  49,  79, 239, 210, 189, 141,  46];

        f.fill_bytes(&mut output);
        assert_eq!(&expected[..], &output[..]);
    }

    #[test]
    fn test_accumulator_correctness() {
        let mut output = [0; 100];
        // Expected output from experiments with pycryto
        // Note that this does not match the results for the Go implementation
        // as described at http://www.seehuhn.de/pages/fortuna ... I believe
        // this is because the author there is reusing some Fortuna state from
        // the previous test. These results agree with pycrypto on a fresh slate
        let mut f = Fortuna::new_unseeded();
        f.pool = [Pool::new(); NUM_POOLS];
        f.add_random_event(0, 0, &[0; 32]);
        f.add_random_event(0, 0, &[0; 32]);
        for i in 0..32 {
            f.add_random_event(1, i, &[1, 2]);
        }

        // from Crypto.Random.Fortuna import FortunaAccumulator
        // x = FortunaAccumulator.FortunaAccumulator()
        // x.add_random_event(0, 0, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0")
        // x.add_random_event(0, 0, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0")
        // x.add_random_event(1, 0, "\1\2")
        // x.add_random_event(1, 1, "\1\2")
        // print list(bytearray(x.random_data(100)))
        let expected = [ 21,  42, 103, 180, 211,  46, 177, 231, 172, 210,
                        109, 198,  34,  40, 245, 199,  76, 114, 105, 185,
                        186, 112, 183, 213,  19,  72, 186,  26, 182, 211,
                        254,  88,  67, 142, 246, 102,  80,  93, 144, 152,
                        123, 191, 168,  26,  21, 194,  69, 214, 249,  80,
                        182, 165, 203,  69, 134, 140,  11, 208,  50, 175,
                        180, 210, 110, 119,   3,  75,   1,   8,   5, 142,
                        226, 168, 179, 246,  82,  42, 223, 239, 201,  23,
                         28,  30, 195, 195,   9, 154,  31, 172, 209, 232,
                        238, 111,  75, 251, 196,  43, 217, 241,  93, 237];
        f.fill_bytes(&mut output);
        assert_eq!(&expected[..], &output[..]);

        // Immediately (less than 100ms)
        f.add_random_event(0, 0, &[0; 32]);
        f.add_random_event(0, 0, &[0; 32]);

        // x.add_random_event(0, 0, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0")
        // x.add_random_event(0, 0, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0")
        // print list(bytearray(x.random_data(100)))
        let expected = [101, 123, 175, 157, 142, 202, 211,  47, 149, 214,
                        135, 249, 148,  19,  50, 116, 169, 188, 240, 218,
                         91,  62,  35,  44, 142, 108,  95,  20,  37, 185,
                         19, 121, 128, 231, 213,  23,  94, 147,  14,  41,
                        199, 253, 246,  14, 230, 152,  11,  17, 118, 254,
                         96, 251, 171, 115,  66,  21, 196, 164,  82,   6,
                        139, 238, 135,  22, 179,   6,   6, 252, 115,  87,
                         19, 167,  56, 192, 140,  93, 132,  78,  22,  16,
                        114,  68, 123, 200,  37, 183, 163, 224, 201, 155,
                        233,  71, 111,  26,   8, 114, 232, 181,  13,  51];
        f.fill_bytes(&mut output);
        assert_eq!(&expected[..], &output[..]);

        // Simulate more than 100 ms passing
        test_force_reseed(&mut f);
        // time.sleep(0.2)
        // print list(bytearray(x.random_data(100)))
        let expected = [ 62, 147, 205, 228,  22,   3, 225, 217, 211, 202,
                         49, 148, 236, 125, 132,  43,  25, 177, 172,  93,
                         98, 177, 112, 160,  76, 101,  60,  98, 225,   9,
                        223, 120, 161,  98, 173, 178,  71,  15,  90, 153,
                         64, 179, 143,  22,  43, 165,  87, 147, 177, 128,
                         21, 105, 214, 197, 224, 187,  22, 139,  16, 153,
                        251,  48, 244,  87,  10, 104, 119, 179,  27, 255,
                         67, 148, 192,  52, 147, 216,  79, 204, 106, 112,
                        238,   0, 239,  99, 159,  96, 184,  90,  54, 122,
                        184, 241, 221, 151, 169,  29, 197,  45,  80,   6];
        f.fill_bytes(&mut output);
        assert_eq!(&expected[..], &output[..]);
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use rand::{SeedableRng, Rng};
    use test::Bencher;

    use super::Fortuna;

    #[bench]
    pub fn fortuna_new_32(bh: &mut Bencher) {
        let mut f: Fortuna = SeedableRng::from_seed(&[100; 64][..]);
        bh.iter( || {
            f.next_u32();
        });
        bh.bytes = 4;
    }

    #[bench]
    pub fn fortuna_new_64(bh: &mut Bencher) {
        let mut f: Fortuna = SeedableRng::from_seed(&[100; 64][..]);
        bh.iter( || {
            f.next_u64();
        });
        bh.bytes = 8;
    }

    #[bench]
    pub fn fortuna_new_1k(bh: &mut Bencher) {
        let mut f: Fortuna = SeedableRng::from_seed(&[100; 64][..]);
        let mut bytes = [0u8; 1024];
        bh.iter( || {
            f.fill_bytes(&mut bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn fortuna_new_64k(bh: &mut Bencher) {
        let mut f: Fortuna = SeedableRng::from_seed(&[100; 64][..]);
        let mut bytes = [0u8; 65536];
        bh.iter( || {
            f.fill_bytes(&mut bytes);
        });
        bh.bytes = bytes.len() as u64;
    }
}
