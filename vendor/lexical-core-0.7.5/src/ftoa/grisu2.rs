//! Internal implementation of the Grisu2 algorithm.
//!
//! The optimized routines are adapted from Andrea Samoljuk's `fpconv` library,
//! which is available [here](https://github.com/night-shift/fpconv).
//!
//! The following benchmarks were run on an "Intel(R) Core(TM) i7-6560U
//! CPU @ 2.20GHz" CPU, on Fedora 28, Linux kernel version 4.18.16-200
//! (x86-64), using the lexical formatter, `dtoa::write()` or `x.to_string()`,
//! avoiding any inefficiencies in Rust string parsing for `format!(...)`
//! or `write!()` macros. The code was compiled with LTO and at an optimization
//! level of 3.
//!
//! The benchmarks with `std` were compiled using "rustc 1.29.2 (17a9dc751
//! 2018-10-05", and the `no_std` benchmarks were compiled using "rustc
//! 1.31.0-nightly (46880f41b 2018-10-15)".
//!
//! The benchmark code may be found `benches/ftoa.rs`.
//!
//! # Benchmarks
//!
//! | Type  |  lexical (ns/iter) | to_string (ns/iter)   | Relative Increase |
//! |:-----:|:------------------:|:---------------------:|:-----------------:|
//! | f32   | 1,221,025          | 2,711,290             | 2.22x             |
//! | f64   | 1,248,397          | 3,558,305             | 2.85x             |
//!
//! # Raw Benchmarks
//!
//! ```text
//! test f32_dtoa      ... bench:   1,174,070 ns/iter (+/- 442,501)
//! test f32_lexical   ... bench:   1,433,234 ns/iter (+/- 633,261)
//! test f32_ryu       ... bench:     669,828 ns/iter (+/- 192,291)
//! test f32_to_string ... bench:   3,341,733 ns/iter (+/- 1,346,744)
//! test f64_dtoa      ... bench:   1,302,522 ns/iter (+/- 364,655)
//! test f64_lexical   ... bench:   1,375,384 ns/iter (+/- 596,860)
//! test f64_ryu       ... bench:   1,015,171 ns/iter (+/- 187,552)
//! test f64_to_string ... bench:   3,900,299 ns/iter (+/- 521,956)
//! ```

// Code the generate the benchmark plot:
//  import numpy as np
//  import pandas as pd
//  import matplotlib.pyplot as plt
//  plt.style.use('ggplot')
//  lexical = np.array([1221025, 1375384]) / 1e6
//  dtoa = np.array([1174070, 1302522]) / 1e6
//  ryu = np.array([669828, 1015171]) / 1e6
//  rustcore = np.array([2711290, 3558305]) / 1e6
//  index = ["f32", "f64"]
//  df = pd.DataFrame({'lexical': lexical, 'lexical (dtoa)': dtoa, 'lexical (ryu)': ryu, 'rustcore': rustcore}, index = index, columns=['lexical', 'lexical (dtoa)', 'lexical (ryu)', 'rustcore'])
//  ax = df.plot.bar(rot=0, figsize=(16, 8), fontsize=14, color=['#E24A33', '#988ED5', '#8EBA42', '#348ABD'])
//  ax.set_ylabel("ms/iter")
//  ax.figure.tight_layout()
//  ax.legend(loc=2, prop={'size': 14})
//  plt.show()

use crate::float::ExtendedFloat80;
use crate::util::*;

// CACHED POWERS

perftools_inline!{
/// Find cached power of 10 from the exponent.
fn cached_grisu_power(exp: i32, k: &mut i32)
    -> &'static ExtendedFloat80
{
    // FLOATING POINT CONSTANTS
    const ONE_LOG_TEN: f64 = 0.30102999566398114;
    const NPOWERS: i32 = 87;
    const FIRSTPOWER: i32 = -348;       // 10 ^ -348
    const STEPPOWERS: i32 = 8;
    const EXPMAX: i32 = -32;
    const EXPMIN: i32 = -60;

    let approx = -((exp + NPOWERS).as_f64()) * ONE_LOG_TEN;
    let approx = approx.as_i32();
    let mut idx = ((approx - FIRSTPOWER) / STEPPOWERS).as_usize();

    loop {
        // Use `arr.get(idx)`, which explicitly provides a reference,
        // instead of `arr[idx]`, which provides a value.
        // We have a bug in versions <= 1.27.0 where it creates
        // a local copy, which we then get the reference to and return.
        // This allows use-after-free, without any warning, so we're
        // using unidiomatic code to avoid any issue.
        let power = GRISU_POWERS_OF_TEN.get(idx).unwrap();
        let current = exp + power.exp + 64;
        if current < EXPMIN {
            idx += 1;
            continue;
        }

        if current > EXPMAX {
            idx -= 1;
            continue;
        }

        *k = FIRSTPOWER + idx.as_i32() * STEPPOWERS;
        return power;
    }
}}

/// Cached powers of ten as specified by the Grisu algorithm.
///
/// Cached powers of 10^k, calculated as if by:
/// `ceil((alpha-e+63) * ONE_LOG_TEN);`
const GRISU_POWERS_OF_TEN: [ExtendedFloat80; 87] = [
    ExtendedFloat80 { mant: 18054884314459144840, exp: -1220 },
    ExtendedFloat80 { mant: 13451937075301367670, exp: -1193 },
    ExtendedFloat80 { mant: 10022474136428063862, exp: -1166 },
    ExtendedFloat80 { mant: 14934650266808366570, exp: -1140 },
    ExtendedFloat80 { mant: 11127181549972568877, exp: -1113 },
    ExtendedFloat80 { mant: 16580792590934885855, exp: -1087 },
    ExtendedFloat80 { mant: 12353653155963782858, exp: -1060 },
    ExtendedFloat80 { mant: 18408377700990114895, exp: -1034 },
    ExtendedFloat80 { mant: 13715310171984221708, exp: -1007 },
    ExtendedFloat80 { mant: 10218702384817765436, exp: -980 },
    ExtendedFloat80 { mant: 15227053142812498563, exp: -954 },
    ExtendedFloat80 { mant: 11345038669416679861, exp: -927 },
    ExtendedFloat80 { mant: 16905424996341287883, exp: -901 },
    ExtendedFloat80 { mant: 12595523146049147757, exp: -874 },
    ExtendedFloat80 { mant: 9384396036005875287, exp: -847 },
    ExtendedFloat80 { mant: 13983839803942852151, exp: -821 },
    ExtendedFloat80 { mant: 10418772551374772303, exp: -794 },
    ExtendedFloat80 { mant: 15525180923007089351, exp: -768 },
    ExtendedFloat80 { mant: 11567161174868858868, exp: -741 },
    ExtendedFloat80 { mant: 17236413322193710309, exp: -715 },
    ExtendedFloat80 { mant: 12842128665889583758, exp: -688 },
    ExtendedFloat80 { mant: 9568131466127621947, exp: -661 },
    ExtendedFloat80 { mant: 14257626930069360058, exp: -635 },
    ExtendedFloat80 { mant: 10622759856335341974, exp: -608 },
    ExtendedFloat80 { mant: 15829145694278690180, exp: -582 },
    ExtendedFloat80 { mant: 11793632577567316726, exp: -555 },
    ExtendedFloat80 { mant: 17573882009934360870, exp: -529 },
    ExtendedFloat80 { mant: 13093562431584567480, exp: -502 },
    ExtendedFloat80 { mant: 9755464219737475723, exp: -475 },
    ExtendedFloat80 { mant: 14536774485912137811, exp: -449 },
    ExtendedFloat80 { mant: 10830740992659433045, exp: -422 },
    ExtendedFloat80 { mant: 16139061738043178685, exp: -396 },
    ExtendedFloat80 { mant: 12024538023802026127, exp: -369 },
    ExtendedFloat80 { mant: 17917957937422433684, exp: -343 },
    ExtendedFloat80 { mant: 13349918974505688015, exp: -316 },
    ExtendedFloat80 { mant: 9946464728195732843, exp: -289 },
    ExtendedFloat80 { mant: 14821387422376473014, exp: -263 },
    ExtendedFloat80 { mant: 11042794154864902060, exp: -236 },
    ExtendedFloat80 { mant: 16455045573212060422, exp: -210 },
    ExtendedFloat80 { mant: 12259964326927110867, exp: -183 },
    ExtendedFloat80 { mant: 18268770466636286478, exp: -157 },
    ExtendedFloat80 { mant: 13611294676837538539, exp: -130 },
    ExtendedFloat80 { mant: 10141204801825835212, exp: -103 },
    ExtendedFloat80 { mant: 15111572745182864684, exp: -77 },
    ExtendedFloat80 { mant: 11258999068426240000, exp: -50 },
    ExtendedFloat80 { mant: 16777216000000000000, exp: -24 },
    ExtendedFloat80 { mant: 12500000000000000000, exp:  3 },
    ExtendedFloat80 { mant: 9313225746154785156, exp:  30 },
    ExtendedFloat80 { mant: 13877787807814456755, exp: 56 },
    ExtendedFloat80 { mant: 10339757656912845936, exp: 83 },
    ExtendedFloat80 { mant: 15407439555097886824, exp: 109 },
    ExtendedFloat80 { mant: 11479437019748901445, exp: 136 },
    ExtendedFloat80 { mant: 17105694144590052135, exp: 162 },
    ExtendedFloat80 { mant: 12744735289059618216, exp: 189 },
    ExtendedFloat80 { mant: 9495567745759798747, exp: 216 },
    ExtendedFloat80 { mant: 14149498560666738074, exp: 242 },
    ExtendedFloat80 { mant: 10542197943230523224, exp: 269 },
    ExtendedFloat80 { mant: 15709099088952724970, exp: 295 },
    ExtendedFloat80 { mant: 11704190886730495818, exp: 322 },
    ExtendedFloat80 { mant: 17440603504673385349, exp: 348 },
    ExtendedFloat80 { mant: 12994262207056124023, exp: 375 },
    ExtendedFloat80 { mant: 9681479787123295682, exp: 402 },
    ExtendedFloat80 { mant: 14426529090290212157, exp: 428 },
    ExtendedFloat80 { mant: 10748601772107342003, exp: 455 },
    ExtendedFloat80 { mant: 16016664761464807395, exp: 481 },
    ExtendedFloat80 { mant: 11933345169920330789, exp: 508 },
    ExtendedFloat80 { mant: 17782069995880619868, exp: 534 },
    ExtendedFloat80 { mant: 13248674568444952270, exp: 561 },
    ExtendedFloat80 { mant: 9871031767461413346, exp: 588 },
    ExtendedFloat80 { mant: 14708983551653345445, exp: 614 },
    ExtendedFloat80 { mant: 10959046745042015199, exp: 641 },
    ExtendedFloat80 { mant: 16330252207878254650, exp: 667 },
    ExtendedFloat80 { mant: 12166986024289022870, exp: 694 },
    ExtendedFloat80 { mant: 18130221999122236476, exp: 720 },
    ExtendedFloat80 { mant: 13508068024458167312, exp: 747 },
    ExtendedFloat80 { mant: 10064294952495520794, exp: 774 },
    ExtendedFloat80 { mant: 14996968138956309548, exp: 800 },
    ExtendedFloat80 { mant: 11173611982879273257, exp: 827 },
    ExtendedFloat80 { mant: 16649979327439178909, exp: 853 },
    ExtendedFloat80 { mant: 12405201291620119593, exp: 880 },
    ExtendedFloat80 { mant: 9242595204427927429, exp: 907 },
    ExtendedFloat80 { mant: 13772540099066387757, exp: 933 },
    ExtendedFloat80 { mant: 10261342003245940623, exp: 960 },
    ExtendedFloat80 { mant: 15290591125556738113, exp: 986 },
    ExtendedFloat80 { mant: 11392378155556871081, exp: 1013 },
    ExtendedFloat80 { mant: 16975966327722178521, exp: 1039 },
    ExtendedFloat80 { mant: 12648080533535911531, exp: 1066 }
];

// FTOA DECIMAL

// LOOKUPS
const TENS: [u64; 20] = [
    10000000000000000000, 1000000000000000000, 100000000000000000,
    10000000000000000, 1000000000000000, 100000000000000,
    10000000000000, 1000000000000, 100000000000,
    10000000000, 1000000000, 100000000,
    10000000, 1000000, 100000,
    10000, 1000, 100,
    10, 1
];

// FPCONV GRISU

perftools_inline!{
/// Round digit to sane approximation.
fn round_digit(digits: &mut [u8], ndigits: usize, delta: u64, mut rem: u64, kappa: u64, mant: u64)
{
    while rem < mant && delta - rem >= kappa && (rem + kappa < mant || mant - rem > rem + kappa - mant)
    {
        digits[ndigits - 1] -= 1;
        rem += kappa;
    }
}}

/// Generate digits from upper and lower range on rounding of number.
fn generate_digits(fp: &ExtendedFloat80, upper: &ExtendedFloat80, lower: &ExtendedFloat80, digits: &mut [u8], k: &mut i32)
    -> usize
{
    let wmant = upper.mant - fp.mant;
    let mut delta = upper.mant - lower.mant;

    let one = ExtendedFloat80 {
        mant: 1 << -upper.exp,
        exp: upper.exp,
    };

    let mut part1 = upper.mant >> -one.exp;
    let mut part2 = upper.mant & (one.mant - 1);

    let mut idx: usize = 0;
    let mut kappa: i32 = 10;
    // 1000000000
    // Guaranteed to be safe, TENS has 20 elements.
    let mut divp = index!(TENS[10..]).iter();
    while kappa > 0 {
        // Remember not to continue! This loop has an increment condition.
        let div = divp.next().unwrap();
        let digit = part1 / div;
        if digit != 0 || idx != 0 {
            digits[idx] = digit.as_u8() + b'0';
            idx += 1;
        }

        part1 -= digit.as_u64() * div;
        kappa -= 1;

        let tmp = (part1 <<-one.exp) + part2;
        if tmp <= delta {
            *k += kappa;
            round_digit(digits, idx, delta, tmp, div << -one.exp, wmant);
            return idx;
        }
    }

    /* 10 */
    // Guaranteed to be safe, TENS has 20 elements.
    let mut unit = index!(TENS[..=18]).iter().rev();
    loop {
        part2 *= 10;
        delta *= 10;
        kappa -= 1;

        let digit = part2 >> -one.exp;
        if digit != 0 || idx != 0 {
            digits[idx] = digit.as_u8() + b'0';
            idx += 1;
        }

        part2 &= one.mant - 1;
        let ten = unit.next().unwrap();
        if part2 < delta {
            *k += kappa;
            round_digit(digits, idx, delta, part2, one.mant, wmant * ten);
            return idx;
        }
    }
}

/// Core Grisu2 algorithm for the float formatter.
fn grisu2(d: f64, digits: &mut [u8], k: &mut i32)
    -> usize
{
    let mut w = ExtendedFloat80::from_f64(d);

    let (mut lower, mut upper) = w.normalized_boundaries();
    w.normalize();

    let mut ki: i32 =  0;
    let cp = cached_grisu_power(upper.exp, &mut ki);

    w     = w.mul(cp);
    upper = upper.mul(cp);
    lower = lower.mul(cp);

    lower.mant += 1;
    upper.mant -= 1;

    *k = -ki;

    generate_digits(&w, &upper, &lower, digits, k)
}

/// Write the produced digits to string.
///
/// Adds formatting for exponents, and other types of information.
fn emit_digits(digits: &mut [u8], mut ndigits: usize, dest: &mut [u8], k: i32)
    -> usize
{
    let exp = k + ndigits.as_i32() - 1;
    let mut exp = exp.abs().as_usize();

    // write plain integer (with ".0" suffix).
    if k >= 0 && exp < (ndigits + 7) {
        let idx = ndigits;
        let count = k.as_usize();
        // These are all safe, since digits.len() >= idx, and
        // dest.len() >= idx+count+2, so the range must be valid.
        copy_to_dst(dest, &index!(digits[..idx]));
        write_bytes(&mut index_mut!(dest[idx..idx+count]), b'0');
        copy_to_dst(&mut index_mut!(dest[idx+count..]), b".0");

        return ndigits + k.as_usize() + 2;
    }

    // write decimal w/o scientific notation
    if k < 0 && (k > -7 || exp < 4) {
        let offset = ndigits.as_isize() - k.abs().as_isize();
        // fp < 1.0 -> write leading zero
        if offset <= 0 {
            let offset = (-offset).as_usize();
            // These are all safe, since digits.len() >= ndigits, and
            // dest.len() >= ndigits+offset+2, so the range must be valid.
            index_mut!(dest[0] = b'0');
            index_mut!(dest[1] = b'.');
            write_bytes(&mut index_mut!(dest[2..offset+2]), b'0');
            copy_to_dst(&mut index_mut!(dest[offset+2..]), &index!(digits[..ndigits]));

            return ndigits + 2 + offset;

        } else {
            // fp > 1.0
            let offset = offset.as_usize();
            // These are all safe, since digits.len() >= ndigits, and
            // dest.len() >= ndigits+1, so the range must be valid.
            copy_to_dst(dest, &index!(digits[..offset]));
            index_mut!(dest[offset] = b'.');
            copy_to_dst(&mut index_mut!(dest[offset+1..]), &index!(digits[offset..ndigits]));

            return ndigits + 1;
        }
    }

    // write decimal w/ scientific notation
    ndigits = ndigits.min(18);

    let dst_len = dest.len();
    let mut dst_iter = dest.iter_mut();
    let mut src_iter = digits.iter().take(ndigits);
    *dst_iter.next().unwrap() = *src_iter.next().unwrap();

    if ndigits > 1 {
        *dst_iter.next().unwrap() = b'.';
        for &src in src_iter {
            *dst_iter.next().unwrap() = src;
        }
    }

    *dst_iter.next().unwrap() = exponent_notation_char(10);

    *dst_iter.next().unwrap() = match k + ndigits.as_i32() - 1 < 0 {
        true    => b'-',
        false   => b'+',
    };

    let mut cent: usize = 0;
    if exp > 99 {
        cent = exp / 100;
        *dst_iter.next().unwrap() = cent.as_u8() + b'0';
        exp -= cent * 100;
    }
    if exp > 9 {
        let dec = exp / 10;
        *dst_iter.next().unwrap() = dec.as_u8() + b'0';
        exp -= dec * 10;
    } else if cent != 0 {
        *dst_iter.next().unwrap() = b'0';
    }

    let shift = (exp % 10).as_u8();
    *dst_iter.next().unwrap() = shift + b'0';

    dst_len - dst_iter.count()
}

perftools_inline!{
fn fpconv_dtoa(d: f64, dest: &mut [u8]) -> usize
{
    let mut digits: [u8; 18] = [0; 18];
    let mut k: i32 = 0;
    let ndigits = grisu2(d, &mut digits, &mut k);
    emit_digits(&mut digits, ndigits, dest, k)
}}

// DECIMAL

perftools_inline!{
/// Forward to double_decimal.
///
/// `f` must be non-special (NaN or infinite), non-negative,
/// and non-zero.
pub(crate) fn float_decimal<'a>(f: f32, bytes: &'a mut [u8])
    -> usize
{
    double_decimal(f.as_f64(), bytes)
}}

// F64

perftools_inline!{
/// Optimized algorithm for decimal numbers.
///
/// `d` must be non-special (NaN or infinite), non-negative,
/// and non-zero.
pub(crate) fn double_decimal<'a>(d: f64, bytes: &'a mut [u8])
    -> usize
{
    fpconv_dtoa(d, bytes)
}}
