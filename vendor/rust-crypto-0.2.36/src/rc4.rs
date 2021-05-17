// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

/*!
 * An implementation of the RC4 (also sometimes called ARC4) stream cipher. THIS IMPLEMENTATION IS
 * NOT A FIXED TIME IMPLEMENTATION.
 */

use buffer::{BufferResult, RefReadBuffer, RefWriteBuffer};
use symmetriccipher::{Encryptor, Decryptor, SynchronousStreamCipher, SymmetricCipherError};
use cryptoutil::symm_enc_or_dec;

#[derive(Copy)]
pub struct Rc4 {
    i: u8,
    j: u8,
    state: [u8; 256]
}

impl Clone for Rc4 { fn clone(&self) -> Rc4 { *self } }

impl Rc4 {
    pub fn new(key: &[u8]) -> Rc4 {
        assert!(key.len() >= 1 && key.len() <= 256);
        let mut rc4 = Rc4 { i: 0, j: 0, state: [0; 256] };
        for (i, x) in rc4.state.iter_mut().enumerate() {
            *x = i as u8;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            j = j.wrapping_add(rc4.state[i]).wrapping_add(key[i % key.len()]);
            rc4.state.swap(i, j as usize);
        }
        rc4
    }
    fn next(&mut self) -> u8 {
        self.i = self.i.wrapping_add(1);
        self.j = self.j.wrapping_add(self.state[self.i as usize]);
        self.state.swap(self.i as usize, self.j as usize);
        let k = self.state[(self.state[self.i as usize].wrapping_add(self.state[self.j as usize])) as usize];
        k
    }
}

impl SynchronousStreamCipher for Rc4 {
    fn process(&mut self, input: &[u8], output: &mut [u8]) {
        assert!(input.len() == output.len());
        for (x, y) in input.iter().zip(output.iter_mut()) {
            *y = *x ^ self.next();
        }
    }
}

impl Encryptor for Rc4 {
    fn encrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

impl Decryptor for Rc4 {
    fn decrypt(&mut self, input: &mut RefReadBuffer, output: &mut RefWriteBuffer, _: bool)
            -> Result<BufferResult, SymmetricCipherError> {
        symm_enc_or_dec(self, input, output)
    }
}

#[cfg(test)]
mod test {
    use std::iter::repeat;

    use symmetriccipher::SynchronousStreamCipher;
    use rc4::Rc4;

    struct Test {
        key: &'static str,
        input: &'static str,
        output: Vec<u8>
    }

    fn tests() -> Vec<Test> {
        vec![
            Test {
                key: "Key",
                input: "Plaintext",
                output: vec![0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]
            },
            Test {
                key: "Wiki",
                input: "pedia",
                output: vec![0x10, 0x21, 0xBF, 0x04, 0x20]
            },
            Test {
                key: "Secret",
                input: "Attack at dawn",
                output: vec![0x45, 0xA0, 0x1F, 0x64, 0x5F, 0xC3, 0x5B,
                          0x38, 0x35, 0x52, 0x54, 0x4B, 0x9B, 0xF5]
            }
        ]
    }

    #[test]
    fn wikipedia_tests() {
        let tests = tests();
        for t in tests.iter() {
            let mut rc4 = Rc4::new(t.key.as_bytes());
            let mut result: Vec<u8> = repeat(0).take(t.output.len()).collect();
            rc4.process(t.input.as_bytes(), &mut result);
            assert!(result == t.output);
        }
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use symmetriccipher::SynchronousStreamCipher;
    use rc4::Rc4;

    #[bench]
    pub fn rc4_10(bh: & mut Bencher) {
        let mut rc4 = Rc4::new("key".as_bytes());
        let input = [1u8; 10];
        let mut output = [0u8; 10];
        bh.iter( || {
            rc4.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn rc4_1k(bh: & mut Bencher) {
        let mut rc4 = Rc4::new("key".as_bytes());
        let input = [1u8; 1024];
        let mut output = [0u8; 1024];
        bh.iter( || {
            rc4.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }

    #[bench]
    pub fn rc4_64k(bh: & mut Bencher) {
        let mut rc4 = Rc4::new("key".as_bytes());
        let input = [1u8; 65536];
        let mut output = [0u8; 65536];
        bh.iter( || {
            rc4.process(&input, &mut output);
        });
        bh.bytes = input.len() as u64;
    }
}
