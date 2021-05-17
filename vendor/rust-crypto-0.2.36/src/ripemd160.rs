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
 * An implementation of the RIPEMD-160 cryptographic hash.
 *
 * First create a `Ripemd160` object using the `Ripemd160` constructor,
 * then feed it input using the `input` or `input_str` methods, which
 * may be called any number of times.
 *
 * After the entire input has been fed to the hash read the result using
 * the `result` or `result_str` methods.
 *
 * The `Ripemd160` object may be reused to create multiple hashes by
 * calling the `reset` method.
 */


use cryptoutil::{write_u32_le, read_u32v_le, add_bytes_to_bits, FixedBuffer,
    FixedBuffer64, StandardPadding};
use digest::Digest;

// Some unexported constants
const DIGEST_BUF_LEN: usize = 5;
const WORK_BUF_LEN: usize = 16;

/// Structure representing the state of a Ripemd160 computation
#[derive(Clone, Copy)]
pub struct Ripemd160 {
    h: [u32; DIGEST_BUF_LEN],
    length_bits: u64,
    buffer: FixedBuffer64,
    computed: bool,
}

fn circular_shift(bits: u32, word: u32) -> u32 {
    word << bits as usize | word >> (32u32 - bits) as usize
}

macro_rules! round(
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr,
     $x:expr, $bits:expr, $add:expr, $round:expr) => ({
        $a = $a.wrapping_add($round).wrapping_add($x).wrapping_add($add);
        $a = circular_shift($bits, $a).wrapping_add($e);
        $c = circular_shift(10, $c);
    });
);

macro_rules! process_block(
    ($h:ident, $data:expr,
     $( round1: h_ordering $f0:expr, $f1:expr, $f2:expr, $f3:expr, $f4:expr;
                data_index $data_index1:expr; roll_shift $bits1:expr )*;
     $( round2: h_ordering $g0:expr, $g1:expr, $g2:expr, $g3:expr, $g4:expr;
                data_index $data_index2:expr; roll_shift $bits2:expr )*;
     $( round3: h_ordering $h0:expr, $h1:expr, $h2:expr, $h3:expr, $h4:expr;
                data_index $data_index3:expr; roll_shift $bits3:expr )*;
     $( round4: h_ordering $i0:expr, $i1:expr, $i2:expr, $i3:expr, $i4:expr;
                data_index $data_index4:expr; roll_shift $bits4:expr )*;
     $( round5: h_ordering $j0:expr, $j1:expr, $j2:expr, $j3:expr, $j4:expr;
                data_index $data_index5:expr; roll_shift $bits5:expr )*;
     $( par_round1: h_ordering $pj0:expr, $pj1:expr, $pj2:expr, $pj3:expr, $pj4:expr;
                    data_index $pdata_index1:expr; roll_shift $pbits1:expr )*;
     $( par_round2: h_ordering $pi0:expr, $pi1:expr, $pi2:expr, $pi3:expr, $pi4:expr;
                    data_index $pdata_index2:expr; roll_shift $pbits2:expr )*;
     $( par_round3: h_ordering $ph0:expr, $ph1:expr, $ph2:expr, $ph3:expr, $ph4:expr;
                    data_index $pdata_index3:expr; roll_shift $pbits3:expr )*;
     $( par_round4: h_ordering $pg0:expr, $pg1:expr, $pg2:expr, $pg3:expr, $pg4:expr;
                    data_index $pdata_index4:expr; roll_shift $pbits4:expr )*;
     $( par_round5: h_ordering $pf0:expr, $pf1:expr, $pf2:expr, $pf3:expr, $pf4:expr;
                    data_index $pdata_index5:expr; roll_shift $pbits5:expr )*;
    ) => ({
        let mut bb = *$h;
        let mut bbb = *$h;

        // Round 1
        $( round!(bb[$f0], bb[$f1], bb[$f2], bb[$f3], bb[$f4],
                  $data[$data_index1], $bits1, 0x00000000,
                  bb[$f1] ^ bb[$f2] ^ bb[$f3]); )*

        // Round 2
        $( round!(bb[$g0], bb[$g1], bb[$g2], bb[$g3], bb[$g4],
                  $data[$data_index2], $bits2, 0x5a827999,
                  (bb[$g1] & bb[$g2]) | (!bb[$g1] & bb[$g3])); )*

        // Round 3
        $( round!(bb[$h0], bb[$h1], bb[$h2], bb[$h3], bb[$h4],
                  $data[$data_index3], $bits3, 0x6ed9eba1,
                  (bb[$h1] | !bb[$h2]) ^ bb[$h3]); )*

        // Round 4
        $( round!(bb[$i0], bb[$i1], bb[$i2], bb[$i3], bb[$i4],
                  $data[$data_index4], $bits4, 0x8f1bbcdc,
                  (bb[$i1] & bb[$i3]) | (bb[$i2] & !bb[$i3])); )*

        // Round 5
        $( round!(bb[$j0], bb[$j1], bb[$j2], bb[$j3], bb[$j4],
                  $data[$data_index5], $bits5, 0xa953fd4e,
                  bb[$j1] ^ (bb[$j2] | !bb[$j3])); )*

        // Parallel rounds: these are the same as the previous five
        // rounds except that the constants have changed, we work
        // with the other buffer, and they are applied in reverse
        // order.

        // Parallel Round 1
        $( round!(bbb[$pj0], bbb[$pj1], bbb[$pj2], bbb[$pj3], bbb[$pj4],
                  $data[$pdata_index1], $pbits1, 0x50a28be6,
                  bbb[$pj1] ^ (bbb[$pj2] | !bbb[$pj3])); )*

        // Parallel Round 2
        $( round!(bbb[$pi0], bbb[$pi1], bbb[$pi2], bbb[$pi3], bbb[$pi4],
                  $data[$pdata_index2], $pbits2, 0x5c4dd124,
                  (bbb[$pi1] & bbb[$pi3]) | (bbb[$pi2] & !bbb[$pi3])); )*

        // Parallel Round 3
        $( round!(bbb[$ph0], bbb[$ph1], bbb[$ph2], bbb[$ph3], bbb[$ph4],
                  $data[$pdata_index3], $pbits3, 0x6d703ef3,
                  (bbb[$ph1] | !bbb[$ph2]) ^ bbb[$ph3]); )*

        // Parallel Round 4
        $( round!(bbb[$pg0], bbb[$pg1], bbb[$pg2], bbb[$pg3], bbb[$pg4],
                  $data[$pdata_index4], $pbits4, 0x7a6d76e9,
                  (bbb[$pg1] & bbb[$pg2]) | (!bbb[$pg1] & bbb[$pg3])); )*

        // Parallel Round 5
        $( round!(bbb[$pf0], bbb[$pf1], bbb[$pf2], bbb[$pf3], bbb[$pf4],
                  $data[$pdata_index5], $pbits5, 0x00000000,
                  bbb[$pf1] ^ bbb[$pf2] ^ bbb[$pf3]); )*

        // Combine results
        bbb[3] = bbb[3].wrapping_add($h[1]).wrapping_add(bb[2]);
        $h[1]   = $h[2].wrapping_add(bb[3]).wrapping_add(bbb[4]);
        $h[2]   = $h[3].wrapping_add(bb[4]).wrapping_add(bbb[0]);
        $h[3]   = $h[4].wrapping_add(bb[0]).wrapping_add(bbb[1]);
        $h[4]   = $h[0].wrapping_add(bb[1]).wrapping_add(bbb[2]);
        $h[0]   =                 bbb[3];
    });
);

fn process_msg_block(data: &[u8], h: &mut [u32; DIGEST_BUF_LEN]) {
    let mut w = [0u32; WORK_BUF_LEN];
    read_u32v_le(&mut w[0..16], data);
    process_block!(h, w[..],
        // Round 1
        round1: h_ordering 0, 1, 2, 3, 4; data_index  0; roll_shift 11
        round1: h_ordering 4, 0, 1, 2, 3; data_index  1; roll_shift 14
        round1: h_ordering 3, 4, 0, 1, 2; data_index  2; roll_shift 15
        round1: h_ordering 2, 3, 4, 0, 1; data_index  3; roll_shift 12
        round1: h_ordering 1, 2, 3, 4, 0; data_index  4; roll_shift  5
        round1: h_ordering 0, 1, 2, 3, 4; data_index  5; roll_shift  8
        round1: h_ordering 4, 0, 1, 2, 3; data_index  6; roll_shift  7
        round1: h_ordering 3, 4, 0, 1, 2; data_index  7; roll_shift  9
        round1: h_ordering 2, 3, 4, 0, 1; data_index  8; roll_shift 11
        round1: h_ordering 1, 2, 3, 4, 0; data_index  9; roll_shift 13
        round1: h_ordering 0, 1, 2, 3, 4; data_index 10; roll_shift 14
        round1: h_ordering 4, 0, 1, 2, 3; data_index 11; roll_shift 15
        round1: h_ordering 3, 4, 0, 1, 2; data_index 12; roll_shift  6
        round1: h_ordering 2, 3, 4, 0, 1; data_index 13; roll_shift  7
        round1: h_ordering 1, 2, 3, 4, 0; data_index 14; roll_shift  9
        round1: h_ordering 0, 1, 2, 3, 4; data_index 15; roll_shift  8;

        // Round 2
        round2: h_ordering 4, 0, 1, 2, 3; data_index  7; roll_shift  7
        round2: h_ordering 3, 4, 0, 1, 2; data_index  4; roll_shift  6
        round2: h_ordering 2, 3, 4, 0, 1; data_index 13; roll_shift  8
        round2: h_ordering 1, 2, 3, 4, 0; data_index  1; roll_shift 13
        round2: h_ordering 0, 1, 2, 3, 4; data_index 10; roll_shift 11
        round2: h_ordering 4, 0, 1, 2, 3; data_index  6; roll_shift  9
        round2: h_ordering 3, 4, 0, 1, 2; data_index 15; roll_shift  7
        round2: h_ordering 2, 3, 4, 0, 1; data_index  3; roll_shift 15
        round2: h_ordering 1, 2, 3, 4, 0; data_index 12; roll_shift  7
        round2: h_ordering 0, 1, 2, 3, 4; data_index  0; roll_shift 12
        round2: h_ordering 4, 0, 1, 2, 3; data_index  9; roll_shift 15
        round2: h_ordering 3, 4, 0, 1, 2; data_index  5; roll_shift  9
        round2: h_ordering 2, 3, 4, 0, 1; data_index  2; roll_shift 11
        round2: h_ordering 1, 2, 3, 4, 0; data_index 14; roll_shift  7
        round2: h_ordering 0, 1, 2, 3, 4; data_index 11; roll_shift 13
        round2: h_ordering 4, 0, 1, 2, 3; data_index  8; roll_shift 12;

        // Round 3
        round3: h_ordering 3, 4, 0, 1, 2; data_index  3; roll_shift 11
        round3: h_ordering 2, 3, 4, 0, 1; data_index 10; roll_shift 13
        round3: h_ordering 1, 2, 3, 4, 0; data_index 14; roll_shift  6
        round3: h_ordering 0, 1, 2, 3, 4; data_index  4; roll_shift  7
        round3: h_ordering 4, 0, 1, 2, 3; data_index  9; roll_shift 14
        round3: h_ordering 3, 4, 0, 1, 2; data_index 15; roll_shift  9
        round3: h_ordering 2, 3, 4, 0, 1; data_index  8; roll_shift 13
        round3: h_ordering 1, 2, 3, 4, 0; data_index  1; roll_shift 15
        round3: h_ordering 0, 1, 2, 3, 4; data_index  2; roll_shift 14
        round3: h_ordering 4, 0, 1, 2, 3; data_index  7; roll_shift  8
        round3: h_ordering 3, 4, 0, 1, 2; data_index  0; roll_shift 13
        round3: h_ordering 2, 3, 4, 0, 1; data_index  6; roll_shift  6
        round3: h_ordering 1, 2, 3, 4, 0; data_index 13; roll_shift  5
        round3: h_ordering 0, 1, 2, 3, 4; data_index 11; roll_shift 12
        round3: h_ordering 4, 0, 1, 2, 3; data_index  5; roll_shift  7
        round3: h_ordering 3, 4, 0, 1, 2; data_index 12; roll_shift  5;

        // Round 4
        round4: h_ordering 2, 3, 4, 0, 1; data_index  1; roll_shift 11
        round4: h_ordering 1, 2, 3, 4, 0; data_index  9; roll_shift 12
        round4: h_ordering 0, 1, 2, 3, 4; data_index 11; roll_shift 14
        round4: h_ordering 4, 0, 1, 2, 3; data_index 10; roll_shift 15
        round4: h_ordering 3, 4, 0, 1, 2; data_index  0; roll_shift 14
        round4: h_ordering 2, 3, 4, 0, 1; data_index  8; roll_shift 15
        round4: h_ordering 1, 2, 3, 4, 0; data_index 12; roll_shift  9
        round4: h_ordering 0, 1, 2, 3, 4; data_index  4; roll_shift  8
        round4: h_ordering 4, 0, 1, 2, 3; data_index 13; roll_shift  9
        round4: h_ordering 3, 4, 0, 1, 2; data_index  3; roll_shift 14
        round4: h_ordering 2, 3, 4, 0, 1; data_index  7; roll_shift  5
        round4: h_ordering 1, 2, 3, 4, 0; data_index 15; roll_shift  6
        round4: h_ordering 0, 1, 2, 3, 4; data_index 14; roll_shift  8
        round4: h_ordering 4, 0, 1, 2, 3; data_index  5; roll_shift  6
        round4: h_ordering 3, 4, 0, 1, 2; data_index  6; roll_shift  5
        round4: h_ordering 2, 3, 4, 0, 1; data_index  2; roll_shift 12;

        // Round 5
        round5: h_ordering 1, 2, 3, 4, 0; data_index  4; roll_shift  9
        round5: h_ordering 0, 1, 2, 3, 4; data_index  0; roll_shift 15
        round5: h_ordering 4, 0, 1, 2, 3; data_index  5; roll_shift  5
        round5: h_ordering 3, 4, 0, 1, 2; data_index  9; roll_shift 11
        round5: h_ordering 2, 3, 4, 0, 1; data_index  7; roll_shift  6
        round5: h_ordering 1, 2, 3, 4, 0; data_index 12; roll_shift  8
        round5: h_ordering 0, 1, 2, 3, 4; data_index  2; roll_shift 13
        round5: h_ordering 4, 0, 1, 2, 3; data_index 10; roll_shift 12
        round5: h_ordering 3, 4, 0, 1, 2; data_index 14; roll_shift  5
        round5: h_ordering 2, 3, 4, 0, 1; data_index  1; roll_shift 12
        round5: h_ordering 1, 2, 3, 4, 0; data_index  3; roll_shift 13
        round5: h_ordering 0, 1, 2, 3, 4; data_index  8; roll_shift 14
        round5: h_ordering 4, 0, 1, 2, 3; data_index 11; roll_shift 11
        round5: h_ordering 3, 4, 0, 1, 2; data_index  6; roll_shift  8
        round5: h_ordering 2, 3, 4, 0, 1; data_index 15; roll_shift  5
        round5: h_ordering 1, 2, 3, 4, 0; data_index 13; roll_shift  6;

        // Parallel Round 1
        par_round1: h_ordering 0, 1, 2, 3, 4; data_index  5; roll_shift  8
        par_round1: h_ordering 4, 0, 1, 2, 3; data_index 14; roll_shift  9
        par_round1: h_ordering 3, 4, 0, 1, 2; data_index  7; roll_shift  9
        par_round1: h_ordering 2, 3, 4, 0, 1; data_index  0; roll_shift 11
        par_round1: h_ordering 1, 2, 3, 4, 0; data_index  9; roll_shift 13
        par_round1: h_ordering 0, 1, 2, 3, 4; data_index  2; roll_shift 15
        par_round1: h_ordering 4, 0, 1, 2, 3; data_index 11; roll_shift 15
        par_round1: h_ordering 3, 4, 0, 1, 2; data_index  4; roll_shift  5
        par_round1: h_ordering 2, 3, 4, 0, 1; data_index 13; roll_shift  7
        par_round1: h_ordering 1, 2, 3, 4, 0; data_index  6; roll_shift  7
        par_round1: h_ordering 0, 1, 2, 3, 4; data_index 15; roll_shift  8
        par_round1: h_ordering 4, 0, 1, 2, 3; data_index  8; roll_shift 11
        par_round1: h_ordering 3, 4, 0, 1, 2; data_index  1; roll_shift 14
        par_round1: h_ordering 2, 3, 4, 0, 1; data_index 10; roll_shift 14
        par_round1: h_ordering 1, 2, 3, 4, 0; data_index  3; roll_shift 12
        par_round1: h_ordering 0, 1, 2, 3, 4; data_index 12; roll_shift  6;

        // Parallel Round 2
        par_round2: h_ordering 4, 0, 1, 2, 3; data_index  6; roll_shift  9
        par_round2: h_ordering 3, 4, 0, 1, 2; data_index 11; roll_shift 13
        par_round2: h_ordering 2, 3, 4, 0, 1; data_index  3; roll_shift 15
        par_round2: h_ordering 1, 2, 3, 4, 0; data_index  7; roll_shift  7
        par_round2: h_ordering 0, 1, 2, 3, 4; data_index  0; roll_shift 12
        par_round2: h_ordering 4, 0, 1, 2, 3; data_index 13; roll_shift  8
        par_round2: h_ordering 3, 4, 0, 1, 2; data_index  5; roll_shift  9
        par_round2: h_ordering 2, 3, 4, 0, 1; data_index 10; roll_shift 11
        par_round2: h_ordering 1, 2, 3, 4, 0; data_index 14; roll_shift  7
        par_round2: h_ordering 0, 1, 2, 3, 4; data_index 15; roll_shift  7
        par_round2: h_ordering 4, 0, 1, 2, 3; data_index  8; roll_shift 12
        par_round2: h_ordering 3, 4, 0, 1, 2; data_index 12; roll_shift  7
        par_round2: h_ordering 2, 3, 4, 0, 1; data_index  4; roll_shift  6
        par_round2: h_ordering 1, 2, 3, 4, 0; data_index  9; roll_shift 15
        par_round2: h_ordering 0, 1, 2, 3, 4; data_index  1; roll_shift 13
        par_round2: h_ordering 4, 0, 1, 2, 3; data_index  2; roll_shift 11;

        // Parallel Round 3
        par_round3: h_ordering 3, 4, 0, 1, 2; data_index 15; roll_shift  9
        par_round3: h_ordering 2, 3, 4, 0, 1; data_index  5; roll_shift  7
        par_round3: h_ordering 1, 2, 3, 4, 0; data_index  1; roll_shift 15
        par_round3: h_ordering 0, 1, 2, 3, 4; data_index  3; roll_shift 11
        par_round3: h_ordering 4, 0, 1, 2, 3; data_index  7; roll_shift  8
        par_round3: h_ordering 3, 4, 0, 1, 2; data_index 14; roll_shift  6
        par_round3: h_ordering 2, 3, 4, 0, 1; data_index  6; roll_shift  6
        par_round3: h_ordering 1, 2, 3, 4, 0; data_index  9; roll_shift 14
        par_round3: h_ordering 0, 1, 2, 3, 4; data_index 11; roll_shift 12
        par_round3: h_ordering 4, 0, 1, 2, 3; data_index  8; roll_shift 13
        par_round3: h_ordering 3, 4, 0, 1, 2; data_index 12; roll_shift  5
        par_round3: h_ordering 2, 3, 4, 0, 1; data_index  2; roll_shift 14
        par_round3: h_ordering 1, 2, 3, 4, 0; data_index 10; roll_shift 13
        par_round3: h_ordering 0, 1, 2, 3, 4; data_index  0; roll_shift 13
        par_round3: h_ordering 4, 0, 1, 2, 3; data_index  4; roll_shift  7
        par_round3: h_ordering 3, 4, 0, 1, 2; data_index 13; roll_shift  5;

        // Parallel Round 4
        par_round4: h_ordering 2, 3, 4, 0, 1; data_index  8; roll_shift 15
        par_round4: h_ordering 1, 2, 3, 4, 0; data_index  6; roll_shift  5
        par_round4: h_ordering 0, 1, 2, 3, 4; data_index  4; roll_shift  8
        par_round4: h_ordering 4, 0, 1, 2, 3; data_index  1; roll_shift 11
        par_round4: h_ordering 3, 4, 0, 1, 2; data_index  3; roll_shift 14
        par_round4: h_ordering 2, 3, 4, 0, 1; data_index 11; roll_shift 14
        par_round4: h_ordering 1, 2, 3, 4, 0; data_index 15; roll_shift  6
        par_round4: h_ordering 0, 1, 2, 3, 4; data_index  0; roll_shift 14
        par_round4: h_ordering 4, 0, 1, 2, 3; data_index  5; roll_shift  6
        par_round4: h_ordering 3, 4, 0, 1, 2; data_index 12; roll_shift  9
        par_round4: h_ordering 2, 3, 4, 0, 1; data_index  2; roll_shift 12
        par_round4: h_ordering 1, 2, 3, 4, 0; data_index 13; roll_shift  9
        par_round4: h_ordering 0, 1, 2, 3, 4; data_index  9; roll_shift 12
        par_round4: h_ordering 4, 0, 1, 2, 3; data_index  7; roll_shift  5
        par_round4: h_ordering 3, 4, 0, 1, 2; data_index 10; roll_shift 15
        par_round4: h_ordering 2, 3, 4, 0, 1; data_index 14; roll_shift  8;

        // Parallel Round 5
        par_round5: h_ordering 1, 2, 3, 4, 0; data_index 12; roll_shift  8
        par_round5: h_ordering 0, 1, 2, 3, 4; data_index 15; roll_shift  5
        par_round5: h_ordering 4, 0, 1, 2, 3; data_index 10; roll_shift 12
        par_round5: h_ordering 3, 4, 0, 1, 2; data_index  4; roll_shift  9
        par_round5: h_ordering 2, 3, 4, 0, 1; data_index  1; roll_shift 12
        par_round5: h_ordering 1, 2, 3, 4, 0; data_index  5; roll_shift  5
        par_round5: h_ordering 0, 1, 2, 3, 4; data_index  8; roll_shift 14
        par_round5: h_ordering 4, 0, 1, 2, 3; data_index  7; roll_shift  6
        par_round5: h_ordering 3, 4, 0, 1, 2; data_index  6; roll_shift  8
        par_round5: h_ordering 2, 3, 4, 0, 1; data_index  2; roll_shift 13
        par_round5: h_ordering 1, 2, 3, 4, 0; data_index 13; roll_shift  6
        par_round5: h_ordering 0, 1, 2, 3, 4; data_index 14; roll_shift  5
        par_round5: h_ordering 4, 0, 1, 2, 3; data_index  0; roll_shift 15
        par_round5: h_ordering 3, 4, 0, 1, 2; data_index  3; roll_shift 13
        par_round5: h_ordering 2, 3, 4, 0, 1; data_index  9; roll_shift 11
        par_round5: h_ordering 1, 2, 3, 4, 0; data_index 11; roll_shift 11;
    );
}

impl Ripemd160 {
    // Construct a `Ripemd` object
    pub fn new() -> Ripemd160 {
        let mut st = Ripemd160 {
            h: [0u32; DIGEST_BUF_LEN],
            length_bits: 0u64,
            buffer: FixedBuffer64::new(),
            computed: false,
        };
        st.reset();
        st
    }
}

impl Digest for Ripemd160 {

    /**
     * Resets the hash to its original state also clearing the buffer.
     * To be used in between hashing separate messages to avoid having
     * to recreate and allocate the whole structure.
     */
    fn reset(&mut self) {
        self.length_bits = 0;
        self.h[0] = 0x67452301u32;
        self.h[1] = 0xefcdab89u32;
        self.h[2] = 0x98badcfeu32;
        self.h[3] = 0x10325476u32;
        self.h[4] = 0xc3d2e1f0u32;
        self.buffer.reset();
        self.computed = false;
    }

    /**
     * Adds the input `msg` to the hash. This method can be called repeatedly
     * for use with streaming messages.
     */
    fn input(&mut self, msg: &[u8]) {
        assert!(!self.computed);
        // Assumes that msg.len() can be converted to u64 without overflow
        self.length_bits = add_bytes_to_bits(self.length_bits, msg.len() as u64);
        let st_h = &mut self.h;
        self.buffer.input(msg, |d: &[u8]| {process_msg_block(d, &mut *st_h);}
        );
    }

    /**
     * Returns the resulting digest of the entire message.
     * Note: `out` must be at least 20 bytes (160 bits)
     */
    fn result(&mut self, out: &mut [u8]) {

        if !self.computed {
            let st_h = &mut self.h;
            self.buffer.standard_padding(8, |d: &[u8]| { process_msg_block(d, &mut *st_h) });

            write_u32_le(self.buffer.next(4), self.length_bits as u32);
            write_u32_le(self.buffer.next(4), (self.length_bits >> 32) as u32 );
            process_msg_block(self.buffer.full_buffer(), st_h);

            self.computed = true;
        }

        write_u32_le(&mut out[0..4], self.h[0]);
        write_u32_le(&mut out[4..8], self.h[1]);
        write_u32_le(&mut out[8..12], self.h[2]);
        write_u32_le(&mut out[12..16], self.h[3]);
        write_u32_le(&mut out[16..20], self.h[4]);
    }

    /**
     * Returns the size of the digest in bits
     */
    fn output_bits(&self) -> usize { 160 }

    /**
     * Returns the block size the hash operates on in bytes
     */
    fn block_size(&self) -> usize { 64 }
}

#[cfg(test)]
mod tests {
    use cryptoutil::test::test_digest_1million_random;
    use digest::Digest;
    use ripemd160::Ripemd160;

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
                    0x8eu8, 0xb2u8, 0x08u8, 0xf7u8,
                    0xe0u8, 0x5du8, 0x98u8, 0x7au8,
                    0x9bu8, 0x04u8, 0x4au8, 0x8eu8,
                    0x98u8, 0xc6u8, 0xb0u8, 0x87u8,
                    0xf1u8, 0x5au8, 0x0bu8, 0xfcu8,
                ],
                output_str: "8eb208f7e05d987a9b044a8e98c6b087f15a0bfc"
            },
            Test {
                input:
                     "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                output: vec![
                    0x12u8, 0xa0u8, 0x53u8, 0x38u8,
                    0x4au8, 0x9cu8, 0x0cu8, 0x88u8,
                    0xe4u8, 0x05u8, 0xa0u8, 0x6cu8,
                    0x27u8, 0xdcu8, 0xf4u8, 0x9au8,
                    0xdau8, 0x62u8, 0xebu8, 0x2bu8,
                ],
                output_str: "12a053384a9c0c88e405a06c27dcf49ada62eb2b"
            },
            // Examples from wikipedia
            Test {
                input: "The quick brown fox jumps over the lazy dog",
                output: vec![
                    0x37u8, 0xf3u8, 0x32u8, 0xf6u8,
                    0x8du8, 0xb7u8, 0x7bu8, 0xd9u8,
                    0xd7u8, 0xedu8, 0xd4u8, 0x96u8,
                    0x95u8, 0x71u8, 0xadu8, 0x67u8,
                    0x1cu8, 0xf9u8, 0xddu8, 0x3bu8,
                ],
                output_str: "37f332f68db77bd9d7edd4969571ad671cf9dd3b",
            },
            Test {
                input: "The quick brown fox jumps over the lazy cog",
                output: vec![
                    0x13u8, 0x20u8, 0x72u8, 0xdfu8,
                    0x69u8, 0x09u8, 0x33u8, 0x83u8,
                    0x5eu8, 0xb8u8, 0xb6u8, 0xadu8,
                    0x0bu8, 0x77u8, 0xe7u8, 0xb6u8,
                    0xf1u8, 0x4au8, 0xcau8, 0xd7u8,
                ],
                output_str: "132072df690933835eb8b6ad0b77e7b6f14acad7",
            },
        ];

        // Test that it works when accepting the message all at once

        let mut out = [0u8; 20];

        let mut sh = Box::new(Ripemd160::new());
        for t in tests.iter() {
            (*sh).input_str(t.input);
            sh.result(&mut out);
            assert_eq!(&t.output[..], &out[..]);

            let out_str = (*sh).result_str();
            assert_eq!(out_str.len(), 40);
            assert_eq!(&out_str[..], t.output_str);

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
            assert_eq!(&t.output[..], &out[..]);

            let out_str = (*sh).result_str();
            assert_eq!(out_str.len(), 40);
            assert!(&out_str[..] == t.output_str);

            sh.reset();
        }
    }

    #[test]
    fn test_1million_random_ripemd160() {
        let mut sh = Ripemd160::new();
        test_digest_1million_random(
            &mut sh,
            64,
            "52783243c1697bdbe16d37f97f68f08325dc1528");
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use digest::Digest;
    use ripemd160::Ripemd160;

    #[bench]
    pub fn ripemd160_10(bh: & mut Bencher) {
        let mut sh = Ripemd160::new();
        let bytes = [1u8; 10];
        bh.iter( || {
            sh.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn ripemd160_1k(bh: & mut Bencher) {
        let mut sh = Ripemd160::new();
        let bytes = [1u8; 1024];
        bh.iter( || {
            sh.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

    #[bench]
    pub fn ripemd160_64k(bh: & mut Bencher) {
        let mut sh = Ripemd160::new();
        let bytes = [1u8; 65536];
        bh.iter( || {
            sh.input(&bytes);
        });
        bh.bytes = bytes.len() as u64;
    }

}
