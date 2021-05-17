// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.


use buffer::{BufferResult, RefReadBuffer, RefWriteBuffer};
use symmetriccipher::{Encryptor, Decryptor, SynchronousStreamCipher, SymmetricCipherError};
use cryptoutil::{read_u32_le, symm_enc_or_dec, write_u32_le};

use std::ptr;


#[derive(Copy)]
pub struct Hc128 {
    p: [u32; 512],
    q: [u32; 512],
    cnt: usize,
    output: [u8; 4],
    output_index: usize
}

impl Clone for Hc128 { fn clone(&self) -> Hc128 { *self } }

impl Hc128 {
    pub fn new(key: &[u8], nonce: &[u8]) -> Hc128 {
        assert!(key.len() == 16);
        assert!(nonce.len() == 16);
        let mut hc128 = Hc128 { p: [0; 512], q: [0; 512], cnt: 0, output: [0; 4], output_index: 0 };
        hc128.init(&key, &nonce);

        hc128
    }

    fn init(&mut self, key : &[u8], nonce : &[u8]) {
        self.cnt = 0;

        let mut w : [u32; 1280] = [0; 1280];

        for i in 0..16 {
            w[i >> 2] |= (key[i] as u32) << (8 * (i & 0x3));
        }
        unsafe {
            ptr::copy_nonoverlapping(w.as_ptr(), w.as_mut_ptr().offset(4), 4);
        }

        for i in 0..nonce.len() & 16 {
            w[(i >> 2) + 8] |= (nonce[i] as u32) << (8 * (i & 0x3));
        }
        unsafe {
            ptr::copy_nonoverlapping(w.as_ptr().offset(8), w.as_mut_ptr().offset(12), 4);
        }

        for i in 16..1280 {
            w[i] = f2(w[i - 2]).wrapping_add(w[i - 7]).wrapping_add(f1(w[i - 15])).wrapping_add(w[i - 16]).wrapping_add(i as u32);
        }

        // Copy contents of w into p and q
        unsafe {
            ptr::copy_nonoverlapping(w.as_ptr().offset(256), self.p.as_mut_ptr(),  512);
            ptr::copy_nonoverlapping(w.as_ptr().offset(768), self.q.as_mut_ptr(), 512);
        }

        for i in 0..512 {
            self.p[i] = self.step();
        }
        for i in 0..512 {
            self.q[i] = self.step();
        }

        self.cnt = 0;
    }

    fn step(&mut self) -> u32 {
        let j : usize = self.cnt & 0x1FF;

        // Precompute resources
        let dim_j3 : usize = (j.wrapping_sub(3)) & 0x1FF;
        let dim_j10 : usize = (j.wrapping_sub(10)) & 0x1FF;
        let dim_j511 : usize = (j.wrapping_sub(511)) & 0x1FF;
        let dim_j12 : usize = (j.wrapping_sub(12)) & 0x1FF;

        let ret : u32;

        if self.cnt < 512 {
            self.p[j] = self.p[j].wrapping_add(self.p[dim_j3].rotate_right(10) ^ self.p[dim_j511].rotate_right(23)).wrapping_add(self.p[dim_j10].rotate_right(8));
            ret = (self.q[(self.p[dim_j12] & 0xFF) as usize].wrapping_add(self.q[(((self.p[dim_j12] >> 16) & 0xFF) + 256) as usize])) ^ self.p[j];
        } else {
            self.q[j] = self.q[j].wrapping_add(self.q[dim_j3].rotate_left(10) ^ self.q[dim_j511].rotate_left(23)).wrapping_add(self.q[dim_j10].rotate_left(8));
            ret = (self.p[(self.q[dim_j12] & 0xFF) as usize].wrapping_add(self.p[(((self.q[dim_j12] >> 16) & 0xFF) + 256) as usize])) ^ self.q[j];
        }

        self.cnt = (self.cnt + 1) & 0x3FF;
        ret
    }

    fn next(&mut self) -> u8 {
        if self.output_index == 0 {
            let step = self.step();
            write_u32_le(&mut self.output, step);
        }
        let ret = self.output[self.output_index];
        self.output_index = (self.output_index + 1) & 0x3;

        ret
    }
}

fn f1(x: u32) -> u32 {
    let ret : u32 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
    ret
}

fn f2(x: u32) -> u32 {
    let ret : u32 = x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10);
    ret
}

impl SynchronousStreamCipher for Hc128 {
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        assert!(input.len() == output.len());

        if input.len() <= 4 {
            // Process data bytewise
            for (inb, outb) in input.iter().zip(output.iter_mut()) {
                *outb = *inb ^ self.next();
            }
        } else {
            let mut data_index = 0;
            let data_index_end = data_index + input.len();

            /*  Process any unused keystream (self.buffer)
             *  remaining from previous operations */
            while self.output_index > 0 && data_index < data_index_end {
                output[data_index] = input[data_index] ^ self.next();
                data_index += 1;
            }

            /*  Process input data blockwise until depleted,
             *  or remaining length less than block size
             *  (size of the keystream buffer, self.buffer : 4 bytes) */
            while data_index + 4 <= data_index_end {
                let data_index_inc = data_index + 4;

                // Read input as le-u32
                let input_u32 = read_u32_le(&input[data_index..data_index_inc]);
                // XOR with keystream u32
                let xored = input_u32 ^ self.step();
                // Write output as le-u32
                write_u32_le(&mut output[data_index..data_index_inc], xored);

                data_index = data_index_inc;
            }

            /*  Process remaining data, if any
             *  (e.g. input length not divisible by 4) */
            while data_index < data_index_end {
                output[data_index] = input[data_index] ^ self.next();
                data_index += 1;
            }
        }
    }
}

impl Encryptor for Hc128 {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

impl Decryptor for Hc128 {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}


#[cfg(test)]
mod test {
    use hc128::Hc128;
    use symmetriccipher::SynchronousStreamCipher;
    use serialize::hex::{FromHex};

    // Vectors from http://www.ecrypt.eu.org/stream/svn/viewcvs.cgi/ecrypt/trunk/submissions/hc-256/hc-128/verified.test-vectors?rev=210&view=markup

    #[test]
    fn test_hc128_ecrypt_set_2_vector_0() {
        let key = "00000000000000000000000000000000".from_hex().unwrap();
        let nonce = "00000000000000000000000000000000".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "82001573A003FD3B7FD72FFB0EAF63AAC62F12DEB629DCA72785A66268EC758B1EDB36900560898178E0AD009ABF1F491330DC1C246E3D6CB264F6900271D59C";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut hc128 = Hc128::new(key.as_ref(), nonce.as_ref());
        hc128.process(&input, &mut output);
        let result: &[u8] = output.as_ref();
        let expected: &[u8] = expected_output.as_ref();
        assert!(result == expected);    }

    #[test]
    fn test_hc128_ecrypt_set_6_vector_1() {
        let key = "0558ABFE51A4F74A9DF04396E93C8FE2".from_hex().unwrap();
        let nonce = "167DE44BB21980E74EB51C83EA51B81F".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "4F864BF3C96D0363B1903F0739189138F6ED2BC0AF583FEEA0CEA66BA7E06E63FB28BF8B3CA0031D24ABB511C57DD17BFC2861C32400072CB680DF2E58A5CECC";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut hc128 = Hc128::new(key.as_ref(), nonce.as_ref());
        hc128.process(&input, &mut output);
        let result: &[u8] = output.as_ref();
        let expected: &[u8] = expected_output.as_ref();
        assert!(result == expected);
    }

    #[test]
    fn test_hc128_ecrypt_set_6_vector_2() {
        let key = "0A5DB00356A9FC4FA2F5489BEE4194E7".from_hex().unwrap();
        let nonce = "1F86ED54BB2289F057BE258CF35AC128".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "82168AB0023B79AAF1E6B4D823855E14A7084378036A951B1CFEF35173875ED86CB66AB8410491A08582BE40080C3102193BA567F9E95D096C3CC60927DD7901";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut hc128 = Hc128::new(key.as_ref(), nonce.as_ref());
        hc128.process(&input, &mut output);
        let result: &[u8] = output.as_ref();
        let expected: &[u8] = expected_output.as_ref();
        assert!(result == expected);
    }

    #[test]
    fn test_hc128_ecrypt_set_6_vector_3() {
        let key = "0F62B5085BAE0154A7FA4DA0F34699EC".from_hex().unwrap();
        let nonce = "288FF65DC42B92F960C72E95FC63CA31".from_hex().unwrap();

        let input = [0u8; 64];
        let expected_output_hex = "1CD8AEDDFE52E217E835D0B7E84E2922D04B1ADBCA53C4522B1AA604C42856A90AF83E2614BCE65C0AECABDD8975B55700D6A26D52FFF0888DA38F1DE20B77B7";
        let expected_output = expected_output_hex.from_hex().unwrap();

        let mut output = [0u8; 64];

        let mut hc128 = Hc128::new(&key, &nonce);
        hc128.process(&input, &mut output);
        assert!(&output[..] == &expected_output[..]);
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use symmetriccipher::SynchronousStreamCipher;
    use hc128::Hc128;

    #[bench]
    pub fn hc128_10(bh: & mut Bencher) {
        let mut hc128 = Hc128::new(&[0; 16], &[0; 16]);
        let input = [1u8; 10];
        let mut output = [0u8; 10];
        bh.iter( || {
            hc128.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn hc128_1k(bh: & mut Bencher) {
        let mut hc128 = Hc128::new(&[0; 16], &[0; 16]);
        let input = [1u8; 1024];
        let mut output = [0u8; 1024];
        bh.iter( || {
            hc128.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn hc128_64k(bh: & mut Bencher) {
        let mut hc128 = Hc128::new(&[0; 16], &[0; 16]);
        let input = [1u8; 65536];
        let mut output = [0u8; 65536];
        bh.iter( || {
            hc128.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }
}
