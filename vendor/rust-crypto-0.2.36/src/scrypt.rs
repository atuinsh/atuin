// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

/*!
 * This module implements the Scrypt key derivation function as specified in [1].
 *
 * # References
 * [1] - C. Percival. Stronger Key Derivation Via Sequential Memory-Hard Functions.
 *       http://www.tarsnap.com/scrypt/scrypt.pdf
 */

use std;
use std::iter::repeat;
use std::io;
use std::mem::size_of;
use cryptoutil::copy_memory;

use rand::{OsRng, Rng};
use serialize::base64;
use serialize::base64::{FromBase64, ToBase64};

use cryptoutil::{read_u32_le, read_u32v_le, write_u32_le};
use hmac::Hmac;
use pbkdf2::pbkdf2;
use sha2::Sha256;
use util::fixed_time_eq;

// The salsa20/8 core function.
fn salsa20_8(input: &[u8], output: &mut [u8]) {

    let mut x = [0u32; 16];
    read_u32v_le(&mut x, input);

    let rounds = 8;

    macro_rules! run_round (
        ($($set_idx:expr, $idx_a:expr, $idx_b:expr, $rot:expr);*) => { {
            $( x[$set_idx] ^= x[$idx_a].wrapping_add(x[$idx_b]).rotate_left($rot); )*
        } }
    );

    for _ in 0..rounds / 2 {
        run_round!(
            0x4, 0x0, 0xc, 7;
            0x8, 0x4, 0x0, 9;
            0xc, 0x8, 0x4, 13;
            0x0, 0xc, 0x8, 18;
            0x9, 0x5, 0x1, 7;
            0xd, 0x9, 0x5, 9;
            0x1, 0xd, 0x9, 13;
            0x5, 0x1, 0xd, 18;
            0xe, 0xa, 0x6, 7;
            0x2, 0xe, 0xa, 9;
            0x6, 0x2, 0xe, 13;
            0xa, 0x6, 0x2, 18;
            0x3, 0xf, 0xb, 7;
            0x7, 0x3, 0xf, 9;
            0xb, 0x7, 0x3, 13;
            0xf, 0xb, 0x7, 18;
            0x1, 0x0, 0x3, 7;
            0x2, 0x1, 0x0, 9;
            0x3, 0x2, 0x1, 13;
            0x0, 0x3, 0x2, 18;
            0x6, 0x5, 0x4, 7;
            0x7, 0x6, 0x5, 9;
            0x4, 0x7, 0x6, 13;
            0x5, 0x4, 0x7, 18;
            0xb, 0xa, 0x9, 7;
            0x8, 0xb, 0xa, 9;
            0x9, 0x8, 0xb, 13;
            0xa, 0x9, 0x8, 18;
            0xc, 0xf, 0xe, 7;
            0xd, 0xc, 0xf, 9;
            0xe, 0xd, 0xc, 13;
            0xf, 0xe, 0xd, 18
        )
    }

    for i in 0..16 {
        write_u32_le(
            &mut output[i * 4..(i + 1) * 4],
            x[i].wrapping_add(read_u32_le(&input[i * 4..(i + 1) * 4])));
    }
}

fn xor(x: &[u8], y: &[u8], output: &mut [u8]) {
    for ((out, &x_i), &y_i) in output.iter_mut().zip(x.iter()).zip(y.iter()) {
        *out = x_i ^ y_i;
    }
}

// Execute the BlockMix operation
// input - the input vector. The length must be a multiple of 128.
// output - the output vector. Must be the same length as input.
fn scrypt_block_mix(input: &[u8], output: &mut [u8]) {
    let mut x = [0u8; 64];
    copy_memory(&input[input.len() - 64..], &mut x);

    let mut t = [0u8; 64];

    for (i, chunk) in input.chunks(64).enumerate() {
        xor(&x, chunk, &mut t);
        salsa20_8(&t, &mut x);
        let pos = if i % 2 == 0 { (i / 2) * 64 } else { (i / 2) * 64 + input.len() / 2 };
        copy_memory(&x, &mut output[pos..pos + 64]);
    }
}

// Execute the ROMix operation in-place.
// b - the data to operate on
// v - a temporary variable to store the vector V
// t - a temporary variable to store the result of the xor
// n - the scrypt parameter N
fn scrypt_ro_mix(b: &mut [u8], v: &mut [u8], t: &mut [u8], n: usize) {
    fn integerify(x: &[u8], n: usize) -> usize {
        // n is a power of 2, so n - 1 gives us a bitmask that we can use to perform a calculation
        // mod n using a simple bitwise and.
        let mask = n - 1;
        // This cast is safe since we're going to get the value mod n (which is a power of 2), so we
        // don't have to care about truncating any of the high bits off
        let result = (read_u32_le(&x[x.len() - 64..x.len() - 60]) as usize) & mask;
        result
    }

    let len = b.len();

    for chunk in v.chunks_mut(len) {
        copy_memory(b, chunk);
        scrypt_block_mix(chunk, b);
    }

    for _ in 0..n {
        let j = integerify(b, n);
        xor(b, &v[j * len..(j + 1) * len], t);
        scrypt_block_mix(t, b);
    }
}

/**
 * The Scrypt parameter values.
 */
#[derive(Clone, Copy)]
pub struct ScryptParams {
    log_n: u8,
    r: u32,
    p: u32
}

impl ScryptParams {
    /**
     * Create a new instance of ScryptParams.
     *
     * # Arguments
     *
     * * log_n - The log2 of the Scrypt parameter N
     * * r - The Scrypt parameter r
     * * p - The Scrypt parameter p
     *
     */
    pub fn new(log_n: u8, r: u32, p: u32) -> ScryptParams {
        assert!(r > 0);
        assert!(p > 0);
        assert!(log_n > 0);
        assert!((log_n as usize) < size_of::<usize>() * 8);
        assert!(size_of::<usize>() >= size_of::<u32>() || (r <= std::usize::MAX as u32 && p < std::usize::MAX as u32));

        let r = r as usize;
        let p = p as usize;

        let n: usize = 1 << log_n;

        // check that r * 128 doesn't overflow
        let r128 = match r.checked_mul(128) {
            Some(x) => x,
            None => panic!("Invalid Scrypt parameters.")
        };

        // check that n * r * 128 doesn't overflow
        match r128.checked_mul(n) {
            Some(_) => { },
            None => panic!("Invalid Scrypt parameters.")
        };

        // check that p * r * 128 doesn't overflow
        match r128.checked_mul(p) {
            Some(_) => { },
            None => panic!("Invalid Scrypt parameters.")
        };

        // This check required by Scrypt:
        // check: n < 2^(128 * r / 8)
        // r * 16 won't overflow since r128 didn't
        assert!((log_n as usize) < r * 16);

        // This check required by Scrypt:
        // check: p <= ((2^32-1) * 32) / (128 * r)
        // It takes a bit of re-arranging to get the check above into this form, but, it is indeed
        // the same.
        assert!(r * p < 0x40000000);

        ScryptParams {
            log_n: log_n,
            r: r as u32,
            p: p as u32
        }
    }
}

/**
 * The scrypt key derivation function.
 *
 * # Arguments
 *
 * * password - The password to process as a byte vector
 * * salt - The salt value to use as a byte vector
 * * params - The ScryptParams to use
 * * output - The resulting derived key is returned in this byte vector.
 *
 */
pub fn scrypt(password: &[u8], salt: &[u8], params: &ScryptParams, output: &mut [u8]) {
    // This check required by Scrypt:
    // check output.len() > 0 && output.len() <= (2^32 - 1) * 32
    assert!(output.len() > 0);
    assert!(output.len() / 32 <= 0xffffffff);

    // The checks in the ScryptParams constructor guarantee that the following is safe:
    let n = 1 << params.log_n;
    let r128 = (params.r as usize) * 128;
    let pr128 = (params.p as usize) * r128;
    let nr128 = n * r128;

    let mut mac = Hmac::new(Sha256::new(), password);

    let mut b: Vec<u8> = repeat(0).take(pr128).collect();
    pbkdf2(&mut mac, salt, 1, &mut b);

    let mut v: Vec<u8> = repeat(0).take(nr128).collect();
    let mut t: Vec<u8> = repeat(0).take(r128).collect();

    for chunk in &mut b.chunks_mut(r128) {
        scrypt_ro_mix(chunk, &mut v, &mut t, n);
    }

    pbkdf2(&mut mac, &*b, 1, output);
}

/**
 * scrypt_simple is a helper function that should be sufficient for the majority of cases where
 * an application needs to use Scrypt to hash a password for storage. The result is a String that
 * contains the parameters used as part of its encoding. The scrypt_check function may be used on
 * a password to check if it is equal to a hashed value.
 *
 * # Format
 *
 * The format of the output is a modified version of the Modular Crypt Format that encodes algorithm
 * used and the parameter values. If all parameter values can each fit within a single byte, a
 * compact format is used (format 0). However, if any value cannot, an expanded format where the r
 * and p parameters are encoded using 4 bytes (format 1) is used. Both formats use a 128-bit salt
 * and a 256-bit hash. The format is indicated as "rscrypt" which is short for "Rust Scrypt format."
 *
 * $rscrypt$<format>$<base64(log_n,r,p)>$<base64(salt)>$<based64(hash)>$
 *
 * # Arguments
 *
 * * password - The password to process as a str
 * * params - The ScryptParams to use
 *
 */
pub fn scrypt_simple(password: &str, params: &ScryptParams) -> io::Result<String> {
    let mut rng = try!(OsRng::new());

    // 128-bit salt
    let salt: Vec<u8> = rng.gen_iter::<u8>().take(16).collect();

    // 256-bit derived key
    let mut dk = [0u8; 32];

    scrypt(password.as_bytes(), &*salt, params, &mut dk);

    let mut result = "$rscrypt$".to_string();
    if params.r < 256 && params.p < 256 {
        result.push_str("0$");
        let mut tmp = [0u8; 3];
        tmp[0] = params.log_n;
        tmp[1] = params.r as u8;
        tmp[2] = params.p as u8;
        result.push_str(&*tmp.to_base64(base64::STANDARD));
    } else {
        result.push_str("1$");
        let mut tmp = [0u8; 9];
        tmp[0] = params.log_n;
        write_u32_le(&mut tmp[1..5], params.r);
        write_u32_le(&mut tmp[5..9], params.p);
        result.push_str(&*tmp.to_base64(base64::STANDARD));
    }
    result.push('$');
    result.push_str(&*salt.to_base64(base64::STANDARD));
    result.push('$');
    result.push_str(&*dk.to_base64(base64::STANDARD));
    result.push('$');

    Ok(result)
}

/**
 * scrypt_check compares a password against the result of a previous call to scrypt_simple and
 * returns true if the passed in password hashes to the same value.
 *
 * # Arguments
 *
 * * password - The password to process as a str
 * * hashed_value - A string representing a hashed password returned by scrypt_simple()
 *
 */
pub fn scrypt_check(password: &str, hashed_value: &str) -> Result<bool, &'static str> {
    static ERR_STR: &'static str = "Hash is not in Rust Scrypt format.";

    let mut iter = hashed_value.split('$');

    // Check that there are no characters before the first "$"
    match iter.next() {
        Some(x) => if x != "" { return Err(ERR_STR); },
        None => return Err(ERR_STR)
    }

    // Check the name
    match iter.next() {
        Some(t) => if t != "rscrypt" { return Err(ERR_STR); },
        None => return Err(ERR_STR)
    }

    // Parse format - currenlty only version 0 (compact) and 1 (expanded) are supported
    let params: ScryptParams;
    match iter.next() {
        Some(fstr) => {
            // Parse the parameters - the size of them depends on the if we are using the compact or
            // expanded format
            let pvec = match iter.next() {
                Some(pstr) => match pstr.from_base64() {
                    Ok(x) => x,
                    Err(_) => return Err(ERR_STR)
                },
                None => return Err(ERR_STR)
            };
            match fstr {
                "0" => {
                    if pvec.len() != 3 { return Err(ERR_STR); }
                    let log_n = pvec[0];
                    let r = pvec[1] as u32;
                    let p = pvec[2] as u32;
                    params = ScryptParams::new(log_n, r, p);
                }
                "1" => {
                    if pvec.len() != 9 { return Err(ERR_STR); }
                    let log_n = pvec[0];
                    let mut pval = [0u32; 2];
                    read_u32v_le(&mut pval, &pvec[1..9]);
                    params = ScryptParams::new(log_n, pval[0], pval[1]);
                }
                _ => return Err(ERR_STR)
            }
        }
        None => return Err(ERR_STR)
    }

    // Salt
    let salt = match iter.next() {
        Some(sstr) => match sstr.from_base64() {
            Ok(salt) => salt,
            Err(_) => return Err(ERR_STR)
        },
        None => return Err(ERR_STR)
    };

    // Hashed value
    let hash = match iter.next() {
        Some(hstr) => match hstr.from_base64() {
            Ok(hash) => hash,
            Err(_) => return Err(ERR_STR)
        },
        None => return Err(ERR_STR)
    };

    // Make sure that the input ends with a "$"
    match iter.next() {
        Some(x) => if x != "" { return Err(ERR_STR); },
        None => return Err(ERR_STR)
    }

    // Make sure there is no trailing data after the final "$"
    match iter.next() {
        Some(_) => return Err(ERR_STR),
        None => { }
    }

    let mut output: Vec<u8> = repeat(0).take(hash.len()).collect();
    scrypt(password.as_bytes(), &*salt, &params, &mut output);

    // Be careful here - its important that the comparison be done using a fixed time equality
    // check. Otherwise an adversary that can measure how long this step takes can learn about the
    // hashed value which would allow them to mount an offline brute force attack against the
    // hashed password.
    Ok(fixed_time_eq(&*output, &*hash))
}

#[cfg(test)]
mod test {
    use std::iter::repeat;

    use scrypt::{scrypt, scrypt_simple, scrypt_check, ScryptParams};

    struct Test {
        password: &'static str,
        salt: &'static str,
        log_n: u8,
        r: u32,
        p: u32,
        expected: Vec<u8>
    }

    // Test vectors from [1]. The last test vector is omitted because it takes too long to run.

    fn tests() -> Vec<Test> {
        vec![
            Test {
                password: "",
                salt: "",
                log_n: 4,
                r: 1,
                p: 1,
                expected: vec![
                    0x77, 0xd6, 0x57, 0x62, 0x38, 0x65, 0x7b, 0x20,
                    0x3b, 0x19, 0xca, 0x42, 0xc1, 0x8a, 0x04, 0x97,
                    0xf1, 0x6b, 0x48, 0x44, 0xe3, 0x07, 0x4a, 0xe8,
                    0xdf, 0xdf, 0xfa, 0x3f, 0xed, 0xe2, 0x14, 0x42,
                    0xfc, 0xd0, 0x06, 0x9d, 0xed, 0x09, 0x48, 0xf8,
                    0x32, 0x6a, 0x75, 0x3a, 0x0f, 0xc8, 0x1f, 0x17,
                    0xe8, 0xd3, 0xe0, 0xfb, 0x2e, 0x0d, 0x36, 0x28,
                    0xcf, 0x35, 0xe2, 0x0c, 0x38, 0xd1, 0x89, 0x06 ]
            },
            Test {
                password: "password",
                salt: "NaCl",
                log_n: 10,
                r: 8,
                p: 16,
                expected: vec![
                    0xfd, 0xba, 0xbe, 0x1c, 0x9d, 0x34, 0x72, 0x00,
                    0x78, 0x56, 0xe7, 0x19, 0x0d, 0x01, 0xe9, 0xfe,
                    0x7c, 0x6a, 0xd7, 0xcb, 0xc8, 0x23, 0x78, 0x30,
                    0xe7, 0x73, 0x76, 0x63, 0x4b, 0x37, 0x31, 0x62,
                    0x2e, 0xaf, 0x30, 0xd9, 0x2e, 0x22, 0xa3, 0x88,
                    0x6f, 0xf1, 0x09, 0x27, 0x9d, 0x98, 0x30, 0xda,
                    0xc7, 0x27, 0xaf, 0xb9, 0x4a, 0x83, 0xee, 0x6d,
                    0x83, 0x60, 0xcb, 0xdf, 0xa2, 0xcc, 0x06, 0x40 ]
            },
            Test {
                password: "pleaseletmein",
                salt: "SodiumChloride",
                log_n: 14,
                r: 8,
                p: 1,
                expected: vec![
                    0x70, 0x23, 0xbd, 0xcb, 0x3a, 0xfd, 0x73, 0x48,
                    0x46, 0x1c, 0x06, 0xcd, 0x81, 0xfd, 0x38, 0xeb,
                    0xfd, 0xa8, 0xfb, 0xba, 0x90, 0x4f, 0x8e, 0x3e,
                    0xa9, 0xb5, 0x43, 0xf6, 0x54, 0x5d, 0xa1, 0xf2,
                    0xd5, 0x43, 0x29, 0x55, 0x61, 0x3f, 0x0f, 0xcf,
                    0x62, 0xd4, 0x97, 0x05, 0x24, 0x2a, 0x9a, 0xf9,
                    0xe6, 0x1e, 0x85, 0xdc, 0x0d, 0x65, 0x1e, 0x40,
                    0xdf, 0xcf, 0x01, 0x7b, 0x45, 0x57, 0x58, 0x87 ]
            },
        ]
    }

    #[test]
    fn test_scrypt() {
        let tests = tests();
        for t in tests.iter() {
            let mut result: Vec<u8> = repeat(0).take(t.expected.len()).collect();
            let params = ScryptParams::new(t.log_n, t.r, t.p);
            scrypt(t.password.as_bytes(), t.salt.as_bytes(), &params, &mut result);
            assert!(result == t.expected);
        }
    }

    fn test_scrypt_simple(log_n: u8, r: u32, p: u32) {
        let password = "password";

        let params = ScryptParams::new(log_n, r, p);
        let out1 = scrypt_simple(password, &params).unwrap();
        let out2 = scrypt_simple(password, &params).unwrap();

        // This just makes sure that a salt is being applied. It doesn't verify that that salt is
        // cryptographically strong, however.
        assert!(out1 != out2);

        match scrypt_check(password, &out1[..]) {
            Ok(r) => assert!(r),
            Err(_) => panic!()
        }
        match scrypt_check(password, &out2[..]) {
            Ok(r) => assert!(r),
            Err(_) => panic!()
        }

        match scrypt_check("wrong", &out1[..]) {
            Ok(r) => assert!(!r),
            Err(_) => panic!()
        }
        match scrypt_check("wrong", &out2[..]) {
            Ok(r) => assert!(!r),
            Err(_) => panic!()
        }
    }

    #[test]
    fn test_scrypt_simple_compact() {
        // These parameters are intentionally very weak - the goal is to make the test run quickly!
        test_scrypt_simple(7, 8, 1);
    }

    #[test]
    fn test_scrypt_simple_expanded() {
        // These parameters are intentionally very weak - the goal is to make the test run quickly!
        test_scrypt_simple(3, 1, 256);
    }
}
