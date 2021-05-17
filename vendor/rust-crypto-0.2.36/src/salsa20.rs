// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use buffer::{BufferResult, RefReadBuffer, RefWriteBuffer};
use symmetriccipher::{Encryptor, Decryptor, SynchronousStreamCipher, SymmetricCipherError};
use cryptoutil::{read_u32_le, symm_enc_or_dec, write_u32_le, xor_keystream};
use simd::u32x4;

use std::cmp;

#[derive(Clone, Copy)]
struct SalsaState {
  a: u32x4,
  b: u32x4,
  c: u32x4,
  d: u32x4
}

#[derive(Copy)]
pub struct Salsa20 {
    state: SalsaState,
    output: [u8; 64],
    offset: usize,
}

impl Clone for Salsa20 { fn clone(&self) -> Salsa20 { *self } }

const S7:u32x4 = u32x4(7, 7, 7, 7);
const S9:u32x4 = u32x4(9, 9, 9, 9);
const S13:u32x4 = u32x4(13, 13, 13, 13);
const S18:u32x4 = u32x4(18, 18, 18, 18);
const S32:u32x4 = u32x4(32, 32, 32, 32);

macro_rules! prepare_rowround {
    ($a: expr, $b: expr, $c: expr) => {{
        let u32x4(a10, a11, a12, a13) = $a;
        $a = u32x4(a13, a10, a11, a12);
        let u32x4(b10, b11, b12, b13) = $b;
        $b = u32x4(b12, b13, b10, b11);
        let u32x4(c10, c11, c12, c13) = $c;
        $c = u32x4(c11, c12, c13, c10);
    }}
}

macro_rules! prepare_columnround {
    ($a: expr, $b: expr, $c: expr) => {{
        let u32x4(a13, a10, a11, a12) = $a;
        $a = u32x4(a10, a11, a12, a13);
        let u32x4(b12, b13, b10, b11) = $b;
        $b = u32x4(b10, b11, b12, b13);
        let u32x4(c11, c12, c13, c10) = $c;
        $c = u32x4(c10, c11, c12, c13);
    }}
}

macro_rules! add_rotate_xor {
    ($dst: expr, $a: expr, $b: expr, $shift: expr) => {{
        let v = $a + $b;
        let r = S32 - $shift;
        let right = v >> r;
        $dst = $dst ^ (v << $shift) ^ right
    }}
}

fn columnround(state: &mut SalsaState) -> () {
    add_rotate_xor!(state.a, state.d, state.c, S7);
    add_rotate_xor!(state.b, state.a, state.d, S9);
    add_rotate_xor!(state.c, state.b, state.a, S13);
    add_rotate_xor!(state.d, state.c, state.b, S18);
}

fn rowround(state: &mut SalsaState) -> () {
    add_rotate_xor!(state.c, state.d, state.a, S7);
    add_rotate_xor!(state.b, state.c, state.d, S9);
    add_rotate_xor!(state.a, state.c, state.b, S13);
    add_rotate_xor!(state.d, state.a, state.b, S18);
}

impl Salsa20 {
    pub fn new(key: &[u8], nonce: &[u8]) -> Salsa20 {
        assert!(key.len() == 16 || key.len() == 32);
        assert!(nonce.len() == 8);
        Salsa20 { state: Salsa20::expand(key, nonce), output: [0; 64], offset: 64 }
    }

    pub fn new_xsalsa20(key: &[u8], nonce: &[u8]) -> Salsa20 {
        assert!(key.len() == 32);
        assert!(nonce.len() == 24);
        let mut xsalsa20 = Salsa20 { state: Salsa20::expand(key, &nonce[0..16]), output: [0; 64], offset: 64 };

        let mut new_key = [0; 32];
        xsalsa20.hsalsa20_hash(&mut new_key);
        xsalsa20.state = Salsa20::expand(&new_key, &nonce[16..24]);

        xsalsa20
    }

    fn expand(key: &[u8], nonce: &[u8]) -> SalsaState {
        let constant = match key.len() {
            16 => b"expand 16-byte k",
            32 => b"expand 32-byte k",
            _  => unreachable!(),
        };

        // The state vectors are laid out to facilitate SIMD operation,
        // instead of the natural matrix ordering.
        //
        //  * Constant (x0, x5, x10, x15)
        //  * Key (x1, x2, x3, x4, x11, x12, x13, x14)
        //  * Input (x6, x7, x8, x9)

        let key_tail; // (x11, x12, x13, x14)
        if key.len() == 16 {
            key_tail = key;
        } else {
            key_tail = &key[16..32];
        }

        let x8; let x9; // (x8, x9)
        if nonce.len() == 16 {
            // HSalsa uses the full 16 byte nonce.
            x8 = read_u32_le(&nonce[8..12]);
            x9 = read_u32_le(&nonce[12..16]);
        } else {
            x8 = 0;
            x9 = 0;
        }

        SalsaState {
            a: u32x4(
                read_u32_le(&key[12..16]),      // x4
                x9,                             // x9
                read_u32_le(&key_tail[12..16]), // x14
                read_u32_le(&key[8..12]),       // x3
            ),
            b: u32x4(
                x8,                             // x8
                read_u32_le(&key_tail[8..12]),  // x13
                read_u32_le(&key[4..8]),        // x2
                read_u32_le(&nonce[4..8])       // x7
            ),
            c: u32x4(
                read_u32_le(&key_tail[4..8]),   // x12
                read_u32_le(&key[0..4]),        // x1
                read_u32_le(&nonce[0..4]),      // x6
                read_u32_le(&key_tail[0..4])    // x11
            ),
            d: u32x4(
                read_u32_le(&constant[0..4]),   // x0
                read_u32_le(&constant[4..8]),   // x5
                read_u32_le(&constant[8..12]),  // x10
                read_u32_le(&constant[12..16]), // x15
            )
        }
    }

    fn hash(&mut self) {
        let mut state = self.state;
        for _ in 0..10 {
            columnround(&mut state);
            prepare_rowround!(state.a, state.b, state.c);
            rowround(&mut state);
            prepare_columnround!(state.a, state.b, state.c);
        }
        let u32x4(x4, x9, x14, x3) = self.state.a + state.a;
        let u32x4(x8, x13, x2, x7) = self.state.b + state.b;
        let u32x4(x12, x1, x6, x11) = self.state.c + state.c;
        let u32x4(x0, x5, x10, x15) = self.state.d + state.d;
        let lens = [
             x0,  x1,  x2,  x3,
             x4,  x5,  x6,  x7,
             x8,  x9, x10, x11,
            x12, x13, x14, x15
        ];
        for i in 0..lens.len() {
            write_u32_le(&mut self.output[i*4..(i+1)*4], lens[i]);
        }

        self.state.b = self.state.b + u32x4(1, 0, 0, 0);
        let u32x4(_, _, _, ctr_lo) = self.state.b;
        if ctr_lo == 0 {
            self.state.a = self.state.a + u32x4(0, 1, 0, 0);
        }

        self.offset = 0;
    }

    fn hsalsa20_hash(&mut self, out: &mut [u8]) {
        let mut state = self.state;
        for _ in 0..10 {
            columnround(&mut state);
            prepare_rowround!(state.a, state.b, state.c);
            rowround(&mut state);
            prepare_columnround!(state.a, state.b, state.c);
        }
        let u32x4(_, x9, _, _) = state.a;
        let u32x4(x8, _, _, x7) = state.b;
        let u32x4(_, _, x6, _) = state.c;
        let u32x4(x0, x5, x10, x15) = state.d;
        let lens = [
            x0, x5, x10, x15,
            x6, x7, x8, x9
        ];
        for i in 0..lens.len() {
            write_u32_le(&mut out[i*4..(i+1)*4], lens[i]);
        }
    }
}

impl SynchronousStreamCipher for Salsa20 {
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        assert!(input.len() == output.len());
        let len = input.len();
        let mut i = 0;
        while i < len {
            // If there is no keystream available in the output buffer,
            // generate the next block.
            if self.offset == 64 {
                self.hash();
            }

            // Process the min(available keystream, remaining input length).
            let count = cmp::min(64 - self.offset, len - i);
            xor_keystream(&mut output[i..i+count], &input[i..i+count], &self.output[self.offset..]);
            i += count;
            self.offset += count;
        }
    }
}

impl Encryptor for Salsa20 {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

impl Decryptor for Salsa20 {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

pub fn hsalsa20(key: &[u8], nonce: &[u8], out: &mut [u8]) {
    assert!(key.len() == 32);
    assert!(nonce.len() == 16);
    let mut h = Salsa20 { state: Salsa20::expand(key, nonce), output: [0; 64], offset: 64 };
    h.hsalsa20_hash(out);
}

#[cfg(test)]
mod test {
    use std::iter::repeat;

    use salsa20::Salsa20;
    use symmetriccipher::SynchronousStreamCipher;

    use digest::Digest;
    use sha2::Sha256;

    #[test]
    fn test_salsa20_128bit_ecrypt_set_1_vector_0() {
        let key = [128u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let nonce = [0u8; 8];
        let input = [0u8; 64];
        let mut stream = [0u8; 64];
        let result =
            [0x4D, 0xFA, 0x5E, 0x48, 0x1D, 0xA2, 0x3E, 0xA0,
             0x9A, 0x31, 0x02, 0x20, 0x50, 0x85, 0x99, 0x36,
             0xDA, 0x52, 0xFC, 0xEE, 0x21, 0x80, 0x05, 0x16,
             0x4F, 0x26, 0x7C, 0xB6, 0x5F, 0x5C, 0xFD, 0x7F,
             0x2B, 0x4F, 0x97, 0xE0, 0xFF, 0x16, 0x92, 0x4A,
             0x52, 0xDF, 0x26, 0x95, 0x15, 0x11, 0x0A, 0x07,
             0xF9, 0xE4, 0x60, 0xBC, 0x65, 0xEF, 0x95, 0xDA,
             0x58, 0xF7, 0x40, 0xB7, 0xD1, 0xDB, 0xB0, 0xAA];

        let mut salsa20 = Salsa20::new(&key, &nonce);
        salsa20.process(&input, &mut stream);
        assert!(stream[..] == result[..]);
    }

    #[test]
    fn test_salsa20_256bit_ecrypt_set_1_vector_0() {
        let key =
            [128u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let nonce = [0u8; 8];
        let input = [0u8; 64];
        let mut stream = [0u8; 64];
        let result =
            [0xE3, 0xBE, 0x8F, 0xDD, 0x8B, 0xEC, 0xA2, 0xE3,
             0xEA, 0x8E, 0xF9, 0x47, 0x5B, 0x29, 0xA6, 0xE7,
             0x00, 0x39, 0x51, 0xE1, 0x09, 0x7A, 0x5C, 0x38,
             0xD2, 0x3B, 0x7A, 0x5F, 0xAD, 0x9F, 0x68, 0x44,
             0xB2, 0x2C, 0x97, 0x55, 0x9E, 0x27, 0x23, 0xC7,
             0xCB, 0xBD, 0x3F, 0xE4, 0xFC, 0x8D, 0x9A, 0x07,
             0x44, 0x65, 0x2A, 0x83, 0xE7, 0x2A, 0x9C, 0x46,
             0x18, 0x76, 0xAF, 0x4D, 0x7E, 0xF1, 0xA1, 0x17];

        let mut salsa20 = Salsa20::new(&key, &nonce);
        salsa20.process(&input, &mut stream);
        assert!(stream[..] == result[..]);
    }

    #[test]
    fn test_salsa20_256bit_nacl_vector_2() {
        let key = [
            0xdc,0x90,0x8d,0xda,0x0b,0x93,0x44,0xa9,
            0x53,0x62,0x9b,0x73,0x38,0x20,0x77,0x88,
            0x80,0xf3,0xce,0xb4,0x21,0xbb,0x61,0xb9,
            0x1c,0xbd,0x4c,0x3e,0x66,0x25,0x6c,0xe4
        ];
        let nonce = [
            0x82,0x19,0xe0,0x03,0x6b,0x7a,0x0b,0x37
        ];
        let input: Vec<u8> = repeat(0).take(4194304).collect();
        let mut stream: Vec<u8> = repeat(0).take(input.len()).collect();
        let output_str = "662b9d0e3463029156069b12f918691a98f7dfb2ca0393c96bbfc6b1fbd630a2";

        let mut salsa20 = Salsa20::new(&key, &nonce);
        salsa20.process(input.as_ref(), &mut stream);

        let mut sh = Sha256::new();
        sh.input(stream.as_ref());
        let out_str = sh.result_str();
        assert!(&out_str[..] == output_str);
    }

    #[test]
    fn test_xsalsa20_cryptopp() {
        let key =
            [0x1b, 0x27, 0x55, 0x64, 0x73, 0xe9, 0x85, 0xd4,
             0x62, 0xcd, 0x51, 0x19, 0x7a, 0x9a, 0x46, 0xc7,
             0x60, 0x09, 0x54, 0x9e, 0xac, 0x64, 0x74, 0xf2,
             0x06, 0xc4, 0xee, 0x08, 0x44, 0xf6, 0x83, 0x89];
        let nonce =
            [0x69, 0x69, 0x6e, 0xe9, 0x55, 0xb6, 0x2b, 0x73,
             0xcd, 0x62, 0xbd, 0xa8, 0x75, 0xfc, 0x73, 0xd6,
             0x82, 0x19, 0xe0, 0x03, 0x6b, 0x7a, 0x0b, 0x37];
        let input = [0u8; 139];
        let mut stream = [0u8; 139];
        let result =
            [0xee, 0xa6, 0xa7, 0x25, 0x1c, 0x1e, 0x72, 0x91,
             0x6d, 0x11, 0xc2, 0xcb, 0x21, 0x4d, 0x3c, 0x25,
             0x25, 0x39, 0x12, 0x1d, 0x8e, 0x23, 0x4e, 0x65,
             0x2d, 0x65, 0x1f, 0xa4, 0xc8, 0xcf, 0xf8, 0x80,
             0x30, 0x9e, 0x64, 0x5a, 0x74, 0xe9, 0xe0, 0xa6,
             0x0d, 0x82, 0x43, 0xac, 0xd9, 0x17, 0x7a, 0xb5,
             0x1a, 0x1b, 0xeb, 0x8d, 0x5a, 0x2f, 0x5d, 0x70,
             0x0c, 0x09, 0x3c, 0x5e, 0x55, 0x85, 0x57, 0x96,
             0x25, 0x33, 0x7b, 0xd3, 0xab, 0x61, 0x9d, 0x61,
             0x57, 0x60, 0xd8, 0xc5, 0xb2, 0x24, 0xa8, 0x5b,
             0x1d, 0x0e, 0xfe, 0x0e, 0xb8, 0xa7, 0xee, 0x16,
             0x3a, 0xbb, 0x03, 0x76, 0x52, 0x9f, 0xcc, 0x09,
             0xba, 0xb5, 0x06, 0xc6, 0x18, 0xe1, 0x3c, 0xe7,
             0x77, 0xd8, 0x2c, 0x3a, 0xe9, 0xd1, 0xa6, 0xf9,
             0x72, 0xd4, 0x16, 0x02, 0x87, 0xcb, 0xfe, 0x60,
             0xbf, 0x21, 0x30, 0xfc, 0x0a, 0x6f, 0xf6, 0x04,
             0x9d, 0x0a, 0x5c, 0x8a, 0x82, 0xf4, 0x29, 0x23,
             0x1f, 0x00, 0x80];

        let mut xsalsa20 = Salsa20::new_xsalsa20(&key, &nonce);
        xsalsa20.process(&input, &mut stream);
        assert!(stream[..] == result[..]);
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use symmetriccipher::SynchronousStreamCipher;
    use salsa20::Salsa20;

    #[bench]
    pub fn salsa20_10(bh: & mut Bencher) {
        let mut salsa20 = Salsa20::new(&[0; 32], &[0; 8]);
        let input = [1u8; 10];
        let mut output = [0u8; 10];
        bh.iter( || {
            salsa20.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn salsa20_1k(bh: & mut Bencher) {
        let mut salsa20 = Salsa20::new(&[0; 32], &[0; 8]);
        let input = [1u8; 1024];
        let mut output = [0u8; 1024];
        bh.iter( || {
            salsa20.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn salsa20_64k(bh: & mut Bencher) {
        let mut salsa20 = Salsa20::new(&[0; 32], &[0; 8]);
        let input = [1u8; 65536];
        let mut output = [0u8; 65536];
        bh.iter( || {
            salsa20.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }
}
