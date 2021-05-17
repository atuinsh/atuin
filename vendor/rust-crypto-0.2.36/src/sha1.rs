// Copyright 2012 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

/*!
An implementation of the SHA-1 cryptographic hash algorithm.

To use this module, first create a `Sha1` object using the `Sha1` constructor,
then feed it an input message using the `input` or `input_str` methods,
which may be called any number of times; they will buffer the input until
there is enough to call the block algorithm.

After the entire input has been fed to the hash read the result using
the `result` or `result_str` methods. The first will return bytes, and
the second will return a `String` object of the same bytes represented
in hexadecimal form.

The `Sha1` object may be reused to create multiple hashes by calling
the `reset()` method. These traits are implemented by all hash digest
algorithms that implement the `Digest` trait. An example of use is:

```rust
use self::crypto::digest::Digest;
use self::crypto::sha1::Sha1;

// create a Sha1 object
let mut hasher = Sha1::new();

// write input message
hasher.input_str("hello world");

// read hash digest
let hex = hasher.result_str();

assert_eq!(hex, "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
```

# Mathematics

The mathematics of the SHA-1 algorithm are quite interesting. In its
definition, The SHA-1 algorithm uses:

* 1 binary operation on bit-arrays:
  * "exclusive or" (XOR)
* 2 binary operations on integers:
  * "addition" (ADD)
  * "rotate left" (ROL)
* 3 ternary operations on bit-arrays:
  * "choose" (CH)
  * "parity" (PAR)
  * "majority" (MAJ)

Some of these functions are commonly found in all hash digest
algorithms, but some, like "parity" is only found in SHA-1.
 */

use digest::Digest;
use cryptoutil::{write_u32_be, read_u32v_be, add_bytes_to_bits, FixedBuffer, FixedBuffer64, StandardPadding};
use simd::u32x4;

const STATE_LEN: usize = 5;
const BLOCK_LEN: usize = 16;

const K0: u32 = 0x5A827999u32;
const K1: u32 = 0x6ED9EBA1u32;
const K2: u32 = 0x8F1BBCDCu32;
const K3: u32 = 0xCA62C1D6u32;

/// Not an intrinsic, but gets the first element of a vector.
#[inline]
pub fn sha1_first(w0: u32x4) -> u32 {
    w0.0
}

/// Not an intrinsic, but adds a word to the first element of a vector.
#[inline]
pub fn sha1_first_add(e: u32, w0: u32x4) -> u32x4 {
    let u32x4(a, b, c, d) = w0;
    u32x4(e.wrapping_add(a), b, c, d)
}

/// Emulates `llvm.x86.sha1msg1` intrinsic.
fn sha1msg1(a: u32x4, b: u32x4) -> u32x4 {
    let u32x4(_, _, w2, w3) = a;
    let u32x4(w4, w5, _, _) = b;
    a ^ u32x4(w2, w3, w4, w5)
}

/// Emulates `llvm.x86.sha1msg2` intrinsic.
fn sha1msg2(a: u32x4, b: u32x4) -> u32x4 {
    let u32x4(x0, x1, x2, x3) = a;
    let u32x4(_, w13, w14, w15) = b;

    let w16 = (x0 ^ w13).rotate_left(1);
    let w17 = (x1 ^ w14).rotate_left(1);
    let w18 = (x2 ^ w15).rotate_left(1);
    let w19 = (x3 ^ w16).rotate_left(1);

    u32x4(w16, w17, w18, w19)
}

/// Performs 4 rounds of the message schedule update.
pub fn sha1_schedule_x4(v0: u32x4, v1: u32x4, v2: u32x4, v3: u32x4) -> u32x4 {
    sha1msg2(sha1msg1(v0, v1) ^ v2, v3)
}

/// Emulates `llvm.x86.sha1nexte` intrinsic.
#[inline]
pub fn sha1_first_half(abcd: u32x4, msg: u32x4) -> u32x4 {
    sha1_first_add(sha1_first(abcd).rotate_left(30), msg)
}

/// Emulates `llvm.x86.sha1rnds4` intrinsic.
/// Performs 4 rounds of the message block digest.
pub fn sha1_digest_round_x4(abcd: u32x4, work: u32x4, i: i8) -> u32x4 {
    const K0V: u32x4 = u32x4(K0, K0, K0, K0);
    const K1V: u32x4 = u32x4(K1, K1, K1, K1);
    const K2V: u32x4 = u32x4(K2, K2, K2, K2);
    const K3V: u32x4 = u32x4(K3, K3, K3, K3);

    match i {
        0 => sha1rnds4c(abcd, work + K0V),
        1 => sha1rnds4p(abcd, work + K1V),
        2 => sha1rnds4m(abcd, work + K2V),
        3 => sha1rnds4p(abcd, work + K3V),
        _ => panic!("unknown icosaround index")
    }
}

/// Not an intrinsic, but helps emulate `llvm.x86.sha1rnds4` intrinsic.
fn sha1rnds4c(abcd: u32x4, msg: u32x4) -> u32x4 {
    let u32x4(mut a, mut b, mut c, mut d) = abcd;
    let u32x4(t, u, v, w) = msg;
    let mut e = 0u32;

    macro_rules! bool3ary_202 {
        ($a:expr, $b:expr, $c:expr) => (($c ^ ($a & ($b ^ $c))))
    } // Choose, MD5F, SHA1C

    e = e.wrapping_add(a.rotate_left(5)).wrapping_add(bool3ary_202!(b, c, d)).wrapping_add(t);
    b = b.rotate_left(30);

    d = d.wrapping_add(e.rotate_left(5)).wrapping_add(bool3ary_202!(a, b, c)).wrapping_add(u);
    a = a.rotate_left(30);

    c = c.wrapping_add(d.rotate_left(5)).wrapping_add(bool3ary_202!(e, a, b)).wrapping_add(v);
    e = e.rotate_left(30);

    b = b.wrapping_add(c.rotate_left(5)).wrapping_add(bool3ary_202!(d, e, a)).wrapping_add(w);
    d = d.rotate_left(30);

    u32x4(b, c, d, e)
}

/// Not an intrinsic, but helps emulate `llvm.x86.sha1rnds4` intrinsic.
fn sha1rnds4p(abcd: u32x4, msg: u32x4) -> u32x4 {
    let u32x4(mut a, mut b, mut c, mut d) = abcd;
    let u32x4(t, u, v, w) = msg;
    let mut e = 0u32;

    macro_rules! bool3ary_150 {
        ($a:expr, $b:expr, $c:expr) => (($a ^ $b ^ $c))
    } // Parity, XOR, MD5H, SHA1P

    e = e.wrapping_add(a.rotate_left(5)).wrapping_add(bool3ary_150!(b, c, d)).wrapping_add(t);
    b = b.rotate_left(30);

    d = d.wrapping_add(e.rotate_left(5)).wrapping_add(bool3ary_150!(a, b, c)).wrapping_add(u);
    a = a.rotate_left(30);

    c = c.wrapping_add(d.rotate_left(5)).wrapping_add(bool3ary_150!(e, a, b)).wrapping_add(v);
    e = e.rotate_left(30);

    b = b.wrapping_add(c.rotate_left(5)).wrapping_add(bool3ary_150!(d, e, a)).wrapping_add(w);
    d = d.rotate_left(30);

    u32x4(b, c, d, e)
}

/// Not an intrinsic, but helps emulate `llvm.x86.sha1rnds4` intrinsic.
fn sha1rnds4m(abcd: u32x4, msg: u32x4) -> u32x4 {
    let u32x4(mut a, mut b, mut c, mut d) = abcd;
    let u32x4(t, u, v, w) = msg;
    let mut e = 0u32;

    macro_rules! bool3ary_232 {
        ($a:expr, $b:expr, $c:expr) => (($a & $b) ^ ($a & $c) ^ ($b & $c))
    } // Majority, SHA1M

    e = e.wrapping_add(a.rotate_left(5)).wrapping_add(bool3ary_232!(b, c, d)).wrapping_add(t);
    b = b.rotate_left(30);

    d = d.wrapping_add(e.rotate_left(5)).wrapping_add(bool3ary_232!(a, b, c)).wrapping_add(u);
    a = a.rotate_left(30);

    c = c.wrapping_add(d.rotate_left(5)).wrapping_add(bool3ary_232!(e, a, b)).wrapping_add(v);
    e = e.rotate_left(30);

    b = b.wrapping_add(c.rotate_left(5)).wrapping_add(bool3ary_232!(d, e, a)).wrapping_add(w);
    d = d.rotate_left(30);

    u32x4(b, c, d, e)
}

/// Process a block with the SHA-1 algorithm.
pub fn sha1_digest_block_u32(state: &mut [u32; 5], block: &[u32; 16]) {

    macro_rules! schedule {
        ($v0:expr, $v1:expr, $v2:expr, $v3:expr) => (
            sha1msg2(sha1msg1($v0, $v1) ^ $v2, $v3)
        )
    }

    macro_rules! rounds4 {
        ($h0:ident, $h1:ident, $wk:expr, $i:expr) => (
            sha1_digest_round_x4($h0, sha1_first_half($h1, $wk), $i)
        )
    }

    // Rounds 0..20
    let mut h0 = u32x4(state[0],
                       state[1],
                       state[2],
                       state[3]);
    let mut w0 = u32x4(block[0],
                       block[1],
                       block[2],
                       block[3]);
    let mut h1 = sha1_digest_round_x4(h0, sha1_first_add(state[4], w0), 0);
    let mut w1 = u32x4(block[4],
                       block[5],
                       block[6],
                       block[7]);
    h0 = rounds4!(h1, h0, w1, 0);
    let mut w2 = u32x4(block[8],
                       block[9],
                       block[10],
                       block[11]);
    h1 = rounds4!(h0, h1, w2, 0);
    let mut w3 = u32x4(block[12],
                       block[13],
                       block[14],
                       block[15]);
    h0 = rounds4!(h1, h0, w3, 0);
    let mut w4 = schedule!(w0, w1, w2, w3);
    h1 = rounds4!(h0, h1, w4, 0);

    // Rounds 20..40
    w0 = schedule!(w1, w2, w3, w4);
    h0 = rounds4!(h1, h0, w0, 1);
    w1 = schedule!(w2, w3, w4, w0);
    h1 = rounds4!(h0, h1, w1, 1);
    w2 = schedule!(w3, w4, w0, w1);
    h0 = rounds4!(h1, h0, w2, 1);
    w3 = schedule!(w4, w0, w1, w2);
    h1 = rounds4!(h0, h1, w3, 1);
    w4 = schedule!(w0, w1, w2, w3);
    h0 = rounds4!(h1, h0, w4, 1);

    // Rounds 40..60
    w0 = schedule!(w1, w2, w3, w4);
    h1 = rounds4!(h0, h1, w0, 2);
    w1 = schedule!(w2, w3, w4, w0);
    h0 = rounds4!(h1, h0, w1, 2);
    w2 = schedule!(w3, w4, w0, w1);
    h1 = rounds4!(h0, h1, w2, 2);
    w3 = schedule!(w4, w0, w1, w2);
    h0 = rounds4!(h1, h0, w3, 2);
    w4 = schedule!(w0, w1, w2, w3);
    h1 = rounds4!(h0, h1, w4, 2);

    // Rounds 60..80
    w0 = schedule!(w1, w2, w3, w4);
    h0 = rounds4!(h1, h0, w0, 3);
    w1 = schedule!(w2, w3, w4, w0);
    h1 = rounds4!(h0, h1, w1, 3);
    w2 = schedule!(w3, w4, w0, w1);
    h0 = rounds4!(h1, h0, w2, 3);
    w3 = schedule!(w4, w0, w1, w2);
    h1 = rounds4!(h0, h1, w3, 3);
    w4 = schedule!(w0, w1, w2, w3);
    h0 = rounds4!(h1, h0, w4, 3);

    let e = sha1_first(h1).rotate_left(30);
    let u32x4(a, b, c, d) = h0;

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

/// Process a block with the SHA-1 algorithm. (See more...)
///
/// SHA-1 is a cryptographic hash function, and as such, it operates
/// on an arbitrary number of bytes. This function operates on a fixed
/// number of bytes. If you call this function with anything other than
/// 64 bytes, then it will panic! This function takes two arguments:
///
/// * `state` is reference to an **array** of 5 words.
/// * `block` is reference to a **slice** of 64 bytes.
///
/// If you want the function that performs a message digest on an arbitrary
/// number of bytes, then see also the `Sha1` struct above.
///
/// # Implementation
///
/// First, some background. Both ARM and Intel are releasing documentation
/// that they plan to include instruction set extensions for SHA1 and SHA256
/// sometime in the near future. Second, LLVM won't lower these intrinsics yet,
/// so these functions were written emulate these instructions. Finally,
/// the block function implemented with these emulated intrinsics turned out
/// to be quite fast! What follows is a discussion of this CPU-level view
/// of the SHA-1 algorithm and how it relates to the mathematical definition.
///
/// The SHA instruction set extensions can be divided up into two categories:
///
/// * message work schedule update calculation ("schedule" v., "work" n.)
/// * message block 80-round digest calculation ("digest" v., "block" n.)
///
/// The schedule-related functions can be used to easily perform 4 rounds
/// of the message work schedule update calculation, as shown below:
///
/// ```ignore
/// macro_rules! schedule_x4 {
///     ($v0:expr, $v1:expr, $v2:expr, $v3:expr) => (
///         sha1msg2(sha1msg1($v0, $v1) ^ $v2, $v3)
///     )
/// }
///
/// macro_rules! round_x4 {
///     ($h0:ident, $h1:ident, $wk:expr, $i:expr) => (
///         sha1rnds4($h0, sha1_first_half($h1, $wk), $i)
///     )
/// }
/// ```
///
/// and also shown above is how the digest-related functions can be used to
/// perform 4 rounds of the message block digest calculation.
///
pub fn sha1_digest_block(state: &mut [u32; 5], block: &[u8/*; 64*/]) {
    assert_eq!(block.len(), BLOCK_LEN*4);
    let mut block2 = [0u32; BLOCK_LEN];
    read_u32v_be(&mut block2[..], block);
    sha1_digest_block_u32(state, &block2);
}

fn add_input(st: &mut Sha1, msg: &[u8]) {
    assert!((!st.computed));
    // Assumes that msg.len() can be converted to u64 without overflow
    st.length_bits = add_bytes_to_bits(st.length_bits, msg.len() as u64);
    let st_h = &mut st.h;
    st.buffer.input(msg, |d: &[u8]| { sha1_digest_block(st_h, d); });
}

fn mk_result(st: &mut Sha1, rs: &mut [u8]) {
    if !st.computed {
        let st_h = &mut st.h;
        st.buffer.standard_padding(8, |d: &[u8]| { sha1_digest_block(&mut *st_h, d) });
        write_u32_be(st.buffer.next(4), (st.length_bits >> 32) as u32 );
        write_u32_be(st.buffer.next(4), st.length_bits as u32);
        sha1_digest_block(st_h, st.buffer.full_buffer());

        st.computed = true;
    }

    write_u32_be(&mut rs[0..4], st.h[0]);
    write_u32_be(&mut rs[4..8], st.h[1]);
    write_u32_be(&mut rs[8..12], st.h[2]);
    write_u32_be(&mut rs[12..16], st.h[3]);
    write_u32_be(&mut rs[16..20], st.h[4]);
}

/// Structure representing the state of a Sha1 computation
#[derive(Clone, Copy)]
pub struct Sha1 {
    h: [u32; STATE_LEN],
    length_bits: u64,
    buffer: FixedBuffer64,
    computed: bool,
}

impl Sha1 {
    /// Construct a `sha` object
    pub fn new() -> Sha1 {
        let mut st = Sha1 {
            h: [0u32; STATE_LEN],
            length_bits: 0u64,
            buffer: FixedBuffer64::new(),
            computed: false,
        };
        st.reset();
        st
    }
}

impl Digest for Sha1 {
    fn reset(&mut self) {
        self.length_bits = 0;
        self.h[0] = 0x67452301u32;
        self.h[1] = 0xEFCDAB89u32;
        self.h[2] = 0x98BADCFEu32;
        self.h[3] = 0x10325476u32;
        self.h[4] = 0xC3D2E1F0u32;
        self.buffer.reset();
        self.computed = false;
    }
    fn input(&mut self, msg: &[u8]) { add_input(self, msg); }
    fn result(&mut self, out: &mut [u8]) { mk_result(self, out) }
    fn output_bits(&self) -> usize { 160 }
    fn block_size(&self) -> usize { 64 }
}

#[cfg(test)]
mod tests {
    use cryptoutil::test::test_digest_1million_random;
    use digest::Digest;
    use sha1::Sha1;

    #[derive(Clone)]
    struct Test {
        input: &'static str,
        output: Vec<u8>,
        output_str: &'static str,
    }

    #[test]
    fn test() {
        let tests = vec![
            // Test messages from FIPS 180-1
            Test {
                input: "abc",
                output: vec![
                    0xA9u8, 0x99u8, 0x3Eu8, 0x36u8,
                    0x47u8, 0x06u8, 0x81u8, 0x6Au8,
                    0xBAu8, 0x3Eu8, 0x25u8, 0x71u8,
                    0x78u8, 0x50u8, 0xC2u8, 0x6Cu8,
                    0x9Cu8, 0xD0u8, 0xD8u8, 0x9Du8,
                ],
                output_str: "a9993e364706816aba3e25717850c26c9cd0d89d"
            },
            Test {
                input:
                     "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                output: vec![
                    0x84u8, 0x98u8, 0x3Eu8, 0x44u8,
                    0x1Cu8, 0x3Bu8, 0xD2u8, 0x6Eu8,
                    0xBAu8, 0xAEu8, 0x4Au8, 0xA1u8,
                    0xF9u8, 0x51u8, 0x29u8, 0xE5u8,
                    0xE5u8, 0x46u8, 0x70u8, 0xF1u8,
                ],
                output_str: "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
            },
            // Examples from wikipedia
            Test {
                input: "The quick brown fox jumps over the lazy dog",
                output: vec![
                    0x2fu8, 0xd4u8, 0xe1u8, 0xc6u8,
                    0x7au8, 0x2du8, 0x28u8, 0xfcu8,
                    0xedu8, 0x84u8, 0x9eu8, 0xe1u8,
                    0xbbu8, 0x76u8, 0xe7u8, 0x39u8,
                    0x1bu8, 0x93u8, 0xebu8, 0x12u8,
                ],
                output_str: "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12",
            },
            Test {
                input: "The quick brown fox jumps over the lazy cog",
                output: vec![
                    0xdeu8, 0x9fu8, 0x2cu8, 0x7fu8,
                    0xd2u8, 0x5eu8, 0x1bu8, 0x3au8,
                    0xfau8, 0xd3u8, 0xe8u8, 0x5au8,
                    0x0bu8, 0xd1u8, 0x7du8, 0x9bu8,
                    0x10u8, 0x0du8, 0xb4u8, 0xb3u8,
                ],
                output_str: "de9f2c7fd25e1b3afad3e85a0bd17d9b100db4b3",
            },
        ];

        // Test that it works when accepting the message all at once

        let mut out = [0u8; 20];

        let mut sh = Box::new(Sha1::new());
        for t in tests.iter() {
            (*sh).input_str(t.input);
            sh.result(&mut out);
            assert!(t.output[..] == out[..]);

            let out_str = (*sh).result_str();
            assert_eq!(out_str.len(), 40);
            assert!(&out_str[..] == t.output_str);

            sh.reset();
        }


        // Test that it works when accepting the message in pieces
        for t in tests.iter() {
            let len = t.input.len();
            let mut left = len;
            while left > 0 {
                let take = (left + 1) / 2;
                (*sh).input_str(&t.input[len - left..take + len - left]);
                left = left - take;
            }
            sh.result(&mut out);
            assert!(t.output[..] == out[..]);

            let out_str = (*sh).result_str();
            assert_eq!(out_str.len(), 40);
            assert!(&out_str[..] == t.output_str);

            sh.reset();
        }
    }

    #[test]
    fn test_1million_random_sha1() {
        let mut sh = Sha1::new();
        test_digest_1million_random(
            &mut sh,
            64,
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use digest::Digest;
    use sha1::{STATE_LEN, BLOCK_LEN};
    use sha1::{Sha1, sha1_digest_block_u32};

    #[bench]
    pub fn sha1_block(bh: & mut Bencher) {
        let mut state = [0u32; STATE_LEN];
        let words = [1u32; BLOCK_LEN];
        bh.iter( || {
            sha1_digest_block_u32(&mut state, &words);
        });
        bh.bytes = 64u64;
    }

    #[bench]
    pub fn sha1_10(bh: & mut Bencher) {
        let mut sh = Sha1::new();
        let bytes = [1u8; 10];
        bh.iter( || {
            sh.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn sha1_1k(bh: & mut Bencher) {
        let mut sh = Sha1::new();
        let bytes = [1u8; 1024];
        bh.iter( || {
            sh.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn sha1_64k(bh: & mut Bencher) {
        let mut sh = Sha1::new();
        let bytes = [1u8; 65536];
        bh.iter( || {
            sh.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

}
