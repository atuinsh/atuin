// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use aesni;

use aessafe;
use blockmodes::{PaddingProcessor, EcbEncryptor, EcbDecryptor, CbcEncryptor, CbcDecryptor, CtrMode,
    CtrModeX8};
use symmetriccipher::{Encryptor, Decryptor, SynchronousStreamCipher};
use util;

/// AES key size
#[derive(Clone, Copy)]
pub enum KeySize {
    KeySize128,
    KeySize192,
    KeySize256
}

/// Get the best implementation of an EcbEncryptor
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn ecb_encryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        padding: X) -> Box<Encryptor> {
    if util::supports_aesni() {
        let aes_enc = aesni::AesNiEncryptor::new(key_size, key);
        let enc = Box::new(EcbEncryptor::new(aes_enc, padding));
        enc
    } else {
        match key_size {
            KeySize::KeySize128 => {
                let aes_enc = aessafe::AesSafe128Encryptor::new(key);
                let enc = Box::new(EcbEncryptor::new(aes_enc, padding));
                enc
            }
            KeySize::KeySize192 => {
                let aes_enc = aessafe::AesSafe192Encryptor::new(key);
                let enc = Box::new(EcbEncryptor::new(aes_enc, padding));
                enc
            }
            KeySize::KeySize256 => {
                let aes_enc = aessafe::AesSafe256Encryptor::new(key);
                let enc = Box::new(EcbEncryptor::new(aes_enc, padding));
                enc
            }
        }
    }
}

/// Get the best implementation of an EcbEncryptor
#[cfg(all(not(target_arch = "x86"), not(target_arch = "x86_64")))]
pub fn ecb_encryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        padding: X) -> Box<Encryptor> {
    match key_size {
        KeySize::KeySize128 => {
            let aes_enc = aessafe::AesSafe128Encryptor::new(key);
            let enc = Box::new(EcbEncryptor::new(aes_enc, padding));
            enc
        }
        KeySize::KeySize192 => {
            let aes_enc = aessafe::AesSafe192Encryptor::new(key);
            let enc = Box::new(EcbEncryptor::new(aes_enc, padding));
            enc
        }
        KeySize::KeySize256 => {
            let aes_enc = aessafe::AesSafe256Encryptor::new(key);
            let enc = Box::new(EcbEncryptor::new(aes_enc, padding));
            enc
        }
    }
}

/// Get the best implementation of an EcbDecryptor
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn ecb_decryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        padding: X) -> Box<Decryptor> {
    if util::supports_aesni() {
        let aes_dec = aesni::AesNiDecryptor::new(key_size, key);
        let dec = Box::new(EcbDecryptor::new(aes_dec, padding));
        dec
    } else {
        match key_size {
            KeySize::KeySize128 => {
                let aes_dec = aessafe::AesSafe128Decryptor::new(key);
                let dec = Box::new(EcbDecryptor::new(aes_dec, padding));
                dec
            }
            KeySize::KeySize192 => {
                let aes_dec = aessafe::AesSafe192Decryptor::new(key);
                let dec = Box::new(EcbDecryptor::new(aes_dec, padding));
                dec
            }
            KeySize::KeySize256 => {
                let aes_dec = aessafe::AesSafe256Decryptor::new(key);
                let dec = Box::new(EcbDecryptor::new(aes_dec, padding));
                dec
            }
        }
    }
}

/// Get the best implementation of an EcbDecryptor
#[cfg(all(not(target_arch = "x86"), not(target_arch = "x86_64")))]
pub fn ecb_decryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        padding: X) -> Box<Decryptor> {
    match key_size {
        KeySize::KeySize128 => {
            let aes_dec = aessafe::AesSafe128Decryptor::new(key);
            let dec = Box::new(EcbDecryptor::new(aes_dec, padding));
            dec
        }
        KeySize::KeySize192 => {
            let aes_dec = aessafe::AesSafe192Decryptor::new(key);
            let dec = Box::new(EcbDecryptor::new(aes_dec, padding));
            dec
        }
        KeySize::KeySize256 => {
            let aes_dec = aessafe::AesSafe256Decryptor::new(key);
            let dec = Box::new(EcbDecryptor::new(aes_dec, padding));
            dec
        }
    }
}

/// Get the best implementation of a CbcEncryptor
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn cbc_encryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        iv: &[u8],
        padding: X) -> Box<Encryptor + 'static> {
    if util::supports_aesni() {
        let aes_enc = aesni::AesNiEncryptor::new(key_size, key);
        let enc = Box::new(CbcEncryptor::new(aes_enc, padding, iv.to_vec()));
        enc
    } else {
        match key_size {
            KeySize::KeySize128 => {
                let aes_enc = aessafe::AesSafe128Encryptor::new(key);
                let enc = Box::new(CbcEncryptor::new(aes_enc, padding, iv.to_vec()));
                enc
            }
            KeySize::KeySize192 => {
                let aes_enc = aessafe::AesSafe192Encryptor::new(key);
                let enc = Box::new(CbcEncryptor::new(aes_enc, padding, iv.to_vec()));
                enc
            }
            KeySize::KeySize256 => {
                let aes_enc = aessafe::AesSafe256Encryptor::new(key);
                let enc = Box::new(CbcEncryptor::new(aes_enc, padding, iv.to_vec()));
                enc
            }
        }
    }
}

/// Get the best implementation of a CbcEncryptor
#[cfg(all(not(target_arch = "x86"), not(target_arch = "x86_64")))]
pub fn cbc_encryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        iv: &[u8],
        padding: X) -> Box<Encryptor + 'static> {
    match key_size {
        KeySize::KeySize128 => {
            let aes_enc = aessafe::AesSafe128Encryptor::new(key);
            let enc = Box::new(CbcEncryptor::new(aes_enc, padding, iv.to_vec()));
            enc
        }
        KeySize::KeySize192 => {
            let aes_enc = aessafe::AesSafe192Encryptor::new(key);
            let enc = Box::new(CbcEncryptor::new(aes_enc, padding, iv.to_vec()));
            enc
        }
        KeySize::KeySize256 => {
            let aes_enc = aessafe::AesSafe256Encryptor::new(key);
            let enc = Box::new(CbcEncryptor::new(aes_enc, padding, iv.to_vec()));
            enc
        }
    }
}

/// Get the best implementation of a CbcDecryptor
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn cbc_decryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        iv: &[u8],
        padding: X) -> Box<Decryptor + 'static> {
    if util::supports_aesni() {
        let aes_dec = aesni::AesNiDecryptor::new(key_size, key);
        let dec = Box::new(CbcDecryptor::new(aes_dec, padding, iv.to_vec()));
        dec
    } else {
        match key_size {
            KeySize::KeySize128 => {
                let aes_dec = aessafe::AesSafe128Decryptor::new(key);
                let dec = Box::new(CbcDecryptor::new(aes_dec, padding, iv.to_vec()));
                dec
            }
            KeySize::KeySize192 => {
                let aes_dec = aessafe::AesSafe192Decryptor::new(key);
                let dec = Box::new(CbcDecryptor::new(aes_dec, padding, iv.to_vec()));
                dec
            }
            KeySize::KeySize256 => {
                let aes_dec = aessafe::AesSafe256Decryptor::new(key);
                let dec = Box::new(CbcDecryptor::new(aes_dec, padding, iv.to_vec()));
                dec
            }
        }
    }
}

/// Get the best implementation of a CbcDecryptor
#[cfg(all(not(target_arch = "x86"), not(target_arch = "x86_64")))]
pub fn cbc_decryptor<X: PaddingProcessor + Send + 'static>(
        key_size: KeySize,
        key: &[u8],
        iv: &[u8],
        padding: X) -> Box<Decryptor + 'static> {
    match key_size {
        KeySize::KeySize128 => {
            let aes_dec = aessafe::AesSafe128Decryptor::new(key);
            let dec = Box::new(CbcDecryptor::new(aes_dec, padding, iv.to_vec()));
            dec as Box<Decryptor + 'static>
        }
        KeySize::KeySize192 => {
            let aes_dec = aessafe::AesSafe192Decryptor::new(key);
            let dec = Box::new(CbcDecryptor::new(aes_dec, padding, iv.to_vec()));
            dec as Box<Decryptor + 'static>
        }
        KeySize::KeySize256 => {
            let aes_dec = aessafe::AesSafe256Decryptor::new(key);
            let dec = Box::new(CbcDecryptor::new(aes_dec, padding, iv.to_vec()));
            dec as Box<Decryptor + 'static>
        }
    }
}

/// Get the best implementation of a Ctr
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn ctr(
        key_size: KeySize,
        key: &[u8],
        iv: &[u8]) -> Box<SynchronousStreamCipher + 'static> {
    if util::supports_aesni() {
        let aes_dec = aesni::AesNiEncryptor::new(key_size, key);
        let dec = Box::new(CtrMode::new(aes_dec, iv.to_vec()));
        dec
    } else {
        match key_size {
            KeySize::KeySize128 => {
                let aes_dec = aessafe::AesSafe128EncryptorX8::new(key);
                let dec = Box::new(CtrModeX8::new(aes_dec, iv));
                dec
            }
            KeySize::KeySize192 => {
                let aes_dec = aessafe::AesSafe192EncryptorX8::new(key);
                let dec = Box::new(CtrModeX8::new(aes_dec, iv));
                dec
            }
            KeySize::KeySize256 => {
                let aes_dec = aessafe::AesSafe256EncryptorX8::new(key);
                let dec = Box::new(CtrModeX8::new(aes_dec, iv));
                dec
            }
        }
    }
}

/// Get the best implementation of a Ctr
#[cfg(all(not(target_arch = "x86"), not(target_arch = "x86_64")))]
pub fn ctr(
        key_size: KeySize,
        key: &[u8],
        iv: &[u8]) -> Box<SynchronousStreamCipher + 'static> {
    match key_size {
        KeySize::KeySize128 => {
            let aes_dec = aessafe::AesSafe128EncryptorX8::new(key);
            let dec = Box::new(CtrModeX8::new(aes_dec, iv));
            dec as Box<SynchronousStreamCipher>
        }
        KeySize::KeySize192 => {
            let aes_dec = aessafe::AesSafe192EncryptorX8::new(key);
            let dec = Box::new(CtrModeX8::new(aes_dec, iv));
            dec as Box<SynchronousStreamCipher>
        }
        KeySize::KeySize256 => {
            let aes_dec = aessafe::AesSafe256EncryptorX8::new(key);
            let dec = Box::new(CtrModeX8::new(aes_dec, iv));
            dec as Box<SynchronousStreamCipher>
        }
    }
}

#[cfg(test)]
mod test {
    use std::iter::repeat;

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    use aesni;

    use aessafe;
    use symmetriccipher::{BlockEncryptor, BlockDecryptor, BlockEncryptorX8, BlockDecryptorX8,
            SynchronousStreamCipher};
    use util;
    use aes;
    use aes::KeySize::{KeySize128, KeySize192, KeySize256};

    // Test vectors from:
    // http://www.inconteam.com/software-development/41-encryption/55-aes-test-vectors

    struct Test {
        key: Vec<u8>,
        data: Vec<TestData>
    }

    struct TestData {
        plain: Vec<u8>,
        cipher: Vec<u8>
    }

    fn tests128() -> Vec<Test> {
        vec![
            Test {
                key: vec![0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6,
                       0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c],
                data: vec![
                    TestData {
                        plain:  vec![0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
                                 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a],
                        cipher: vec![0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60,
                                 0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66, 0xef, 0x97]
                    },
                    TestData {
                        plain:  vec![0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
                                 0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51],
                        cipher: vec![0xf5, 0xd3, 0xd5, 0x85, 0x03, 0xb9, 0x69, 0x9d,
                                 0xe7, 0x85, 0x89, 0x5a, 0x96, 0xfd, 0xba, 0xaf]
                    },
                    TestData {
                        plain:  vec![0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
                                 0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef],
                        cipher: vec![0x43, 0xb1, 0xcd, 0x7f, 0x59, 0x8e, 0xce, 0x23,
                                 0x88, 0x1b, 0x00, 0xe3, 0xed, 0x03, 0x06, 0x88]
                    },
                    TestData {
                        plain:  vec![0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
                                 0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10],
                        cipher: vec![0x7b, 0x0c, 0x78, 0x5e, 0x27, 0xe8, 0xad, 0x3f,
                                 0x82, 0x23, 0x20, 0x71, 0x04, 0x72, 0x5d, 0xd4]
                    }
                ]
            }
        ]
    }

    fn tests192() -> Vec<Test> {
        vec![
            Test {
                key: vec![0x8e, 0x73, 0xb0, 0xf7, 0xda, 0x0e, 0x64, 0x52, 0xc8, 0x10, 0xf3, 0x2b,
                       0x80, 0x90, 0x79, 0xe5, 0x62, 0xf8, 0xea, 0xd2, 0x52, 0x2c, 0x6b, 0x7b],
                data: vec![
                    TestData {
                        plain:  vec![0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
                                  0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a],
                        cipher: vec![0xbd, 0x33, 0x4f, 0x1d, 0x6e, 0x45, 0xf2, 0x5f,
                                  0xf7, 0x12, 0xa2, 0x14, 0x57, 0x1f, 0xa5, 0xcc]
                    },
                    TestData {
                        plain:  vec![0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
                                  0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51],
                        cipher: vec![0x97, 0x41, 0x04, 0x84, 0x6d, 0x0a, 0xd3, 0xad,
                                  0x77, 0x34, 0xec, 0xb3, 0xec, 0xee, 0x4e, 0xef]
                    },
                    TestData {
                        plain:  vec![0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
                                  0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef],
                        cipher: vec![0xef, 0x7a, 0xfd, 0x22, 0x70, 0xe2, 0xe6, 0x0a,
                                  0xdc, 0xe0, 0xba, 0x2f, 0xac, 0xe6, 0x44, 0x4e]
                    },
                    TestData {
                        plain:  vec![0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
                                  0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10],
                        cipher: vec![0x9a, 0x4b, 0x41, 0xba, 0x73, 0x8d, 0x6c, 0x72,
                                  0xfb, 0x16, 0x69, 0x16, 0x03, 0xc1, 0x8e, 0x0e]
                    }
                ]
            }
        ]
    }

    fn tests256() -> Vec<Test> {
        vec![
            Test {
                key: vec![0x60, 0x3d, 0xeb, 0x10, 0x15, 0xca, 0x71, 0xbe,
                       0x2b, 0x73, 0xae, 0xf0, 0x85, 0x7d, 0x77, 0x81,
                       0x1f, 0x35, 0x2c, 0x07, 0x3b, 0x61, 0x08, 0xd7,
                       0x2d, 0x98, 0x10, 0xa3, 0x09, 0x14, 0xdf, 0xf4],
                data: vec![
                    TestData {
                        plain:  vec![0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
                                  0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a],
                        cipher: vec![0xf3, 0xee, 0xd1, 0xbd, 0xb5, 0xd2, 0xa0, 0x3c,
                                  0x06, 0x4b, 0x5a, 0x7e, 0x3d, 0xb1, 0x81, 0xf8]
                    },
                    TestData {
                        plain:  vec![0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
                                  0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51],
                        cipher: vec![0x59, 0x1c, 0xcb, 0x10, 0xd4, 0x10, 0xed, 0x26,
                                  0xdc, 0x5b, 0xa7, 0x4a, 0x31, 0x36, 0x28, 0x70]
                    },
                    TestData {
                        plain:  vec![0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
                                  0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef],
                        cipher: vec![0xb6, 0xed, 0x21, 0xb9, 0x9c, 0xa6, 0xf4, 0xf9,
                                  0xf1, 0x53, 0xe7, 0xb1, 0xbe, 0xaf, 0xed, 0x1d]
                    },
                    TestData {
                        plain:  vec![0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
                                  0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10],
                        cipher: vec![0x23, 0x30, 0x4b, 0x7a, 0x39, 0xf9, 0xf3, 0xff,
                                  0x06, 0x7d, 0x8d, 0x8f, 0x9e, 0x24, 0xec, 0xc7]
                    }
                ]
            }
        ]
    }

    struct CtrTest {
        key: Vec<u8>,
        ctr: Vec<u8>,
        plain: Vec<u8>,
        cipher: Vec<u8>
    }

    fn aes_ctr_tests() -> Vec<CtrTest> {
        vec![
            CtrTest {
                key: repeat(1).take(16).collect(),
                ctr: repeat(3).take(16).collect(),
                plain: repeat(2).take(33).collect(),
                cipher: vec![
                    0x64, 0x3e, 0x05, 0x19, 0x79, 0x78, 0xd7, 0x45,
                    0xa9, 0x10, 0x5f, 0xd8, 0x4c, 0xd7, 0xe6, 0xb1,
                    0x5f, 0x66, 0xc6, 0x17, 0x4b, 0x25, 0xea, 0x24,
                    0xe6, 0xf9, 0x19, 0x09, 0xb7, 0xdd, 0x84, 0xfb,
                    0x86 ]
            }
        ]
    }

    fn run_test<E: BlockEncryptor, D: BlockDecryptor>(enc: &mut E, dec: &mut D, test: &Test) {
        let mut tmp = [0u8; 16];
        for data in test.data.iter() {
            enc.encrypt_block(&data.plain[..], &mut tmp);
            assert!(tmp[..] == data.cipher[..]);
            dec.decrypt_block(&data.cipher[..], &mut tmp);
            assert!(tmp[..] == data.plain[..]);
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn test_aesni_128() {
        if util::supports_aesni() {
            let tests = tests128();
            for t in tests.iter() {
                let mut enc = aesni::AesNiEncryptor::new(KeySize128, &t.key[..]);
                let mut dec = aesni::AesNiDecryptor::new(KeySize128, &t.key[..]);
                run_test(&mut enc, &mut dec, t);
            }
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn test_aesni_192() {
        if util::supports_aesni() {
            let tests = tests192();
            for t in tests.iter() {
                let mut enc = aesni::AesNiEncryptor::new(KeySize192, &t.key[..]);
                let mut dec = aesni::AesNiDecryptor::new(KeySize192, &t.key[..]);
                run_test(&mut enc, &mut dec, t);
            }
        }
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn test_aesni_256() {
        if util::supports_aesni() {
            let tests = tests256();
            for t in tests.iter() {
                let mut enc = aesni::AesNiEncryptor::new(KeySize256, &t.key[..]);
                let mut dec = aesni::AesNiDecryptor::new(KeySize256, &t.key[..]);
                run_test(&mut enc, &mut dec, t);
            }
        }
    }

    #[test]
    fn test_aessafe_128() {
        let tests = tests128();
        for t in tests.iter() {
            let mut enc = aessafe::AesSafe128Encryptor::new(&t.key[..]);
            let mut dec = aessafe::AesSafe128Decryptor::new(&t.key[..]);
            run_test(&mut enc, &mut dec, t);
        }
    }

    #[test]
    fn test_aessafe_192() {
        let tests = tests192();
        for t in tests.iter() {
            let mut enc = aessafe::AesSafe192Encryptor::new(&t.key[..]);
            let mut dec = aessafe::AesSafe192Decryptor::new(&t.key[..]);
            run_test(&mut enc, &mut dec, t);
        }
    }

    #[test]
    fn test_aessafe_256() {
        let tests = tests256();
        for t in tests.iter() {
            let mut enc = aessafe::AesSafe256Encryptor::new(&t.key[..]);
            let mut dec = aessafe::AesSafe256Decryptor::new(&t.key[..]);
            run_test(&mut enc, &mut dec, t);
        }
    }

    // The following test vectors are all from NIST SP 800-38A

    #[test]
    fn test_aessafe_128_x8() {
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6,
            0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c ];
        let plain: [u8; 128] = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
            0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a,
            0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
            0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51,
            0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
            0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef,
            0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10,
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
            0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a,
            0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
            0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51,
            0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
            0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef,
            0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10 ];
        let cipher: [u8; 128] = [
            0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60,
            0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66, 0xef, 0x97,
            0xf5, 0xd3, 0xd5, 0x85, 0x03, 0xb9, 0x69, 0x9d,
            0xe7, 0x85, 0x89, 0x5a, 0x96, 0xfd, 0xba, 0xaf,
            0x43, 0xb1, 0xcd, 0x7f, 0x59, 0x8e, 0xce, 0x23,
            0x88, 0x1b, 0x00, 0xe3, 0xed, 0x03, 0x06, 0x88,
            0x7b, 0x0c, 0x78, 0x5e, 0x27, 0xe8, 0xad, 0x3f,
            0x82, 0x23, 0x20, 0x71, 0x04, 0x72, 0x5d, 0xd4,
            0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60,
            0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66, 0xef, 0x97,
            0xf5, 0xd3, 0xd5, 0x85, 0x03, 0xb9, 0x69, 0x9d,
            0xe7, 0x85, 0x89, 0x5a, 0x96, 0xfd, 0xba, 0xaf,
            0x43, 0xb1, 0xcd, 0x7f, 0x59, 0x8e, 0xce, 0x23,
            0x88, 0x1b, 0x00, 0xe3, 0xed, 0x03, 0x06, 0x88,
            0x7b, 0x0c, 0x78, 0x5e, 0x27, 0xe8, 0xad, 0x3f,
            0x82, 0x23, 0x20, 0x71, 0x04, 0x72, 0x5d, 0xd4 ];

        let enc = aessafe::AesSafe128EncryptorX8::new(&key);
        let dec = aessafe::AesSafe128DecryptorX8::new(&key);
        let mut tmp = [0u8; 128];
        enc.encrypt_block_x8(&plain, &mut tmp);
        assert!(tmp[..] == cipher[..]);
        dec.decrypt_block_x8(&cipher, &mut tmp);
        assert!(tmp[..] == plain[..]);
    }

    #[test]
    fn test_aessafe_192_x8() {
        let key: [u8; 24] = [
            0x8e, 0x73, 0xb0, 0xf7, 0xda, 0x0e, 0x64, 0x52, 0xc8, 0x10, 0xf3, 0x2b,
            0x80, 0x90, 0x79, 0xe5, 0x62, 0xf8, 0xea, 0xd2, 0x52, 0x2c, 0x6b, 0x7b ];
        let plain: [u8; 128] = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
            0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a,
            0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
            0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51,
            0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
            0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef,
            0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10,
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
            0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a,
            0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
            0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51,
            0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
            0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef,
            0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10 ];
        let cipher: [u8; 128] = [
            0xbd, 0x33, 0x4f, 0x1d, 0x6e, 0x45, 0xf2, 0x5f,
            0xf7, 0x12, 0xa2, 0x14, 0x57, 0x1f, 0xa5, 0xcc,
            0x97, 0x41, 0x04, 0x84, 0x6d, 0x0a, 0xd3, 0xad,
            0x77, 0x34, 0xec, 0xb3, 0xec, 0xee, 0x4e, 0xef,
            0xef, 0x7a, 0xfd, 0x22, 0x70, 0xe2, 0xe6, 0x0a,
            0xdc, 0xe0, 0xba, 0x2f, 0xac, 0xe6, 0x44, 0x4e,
            0x9a, 0x4b, 0x41, 0xba, 0x73, 0x8d, 0x6c, 0x72,
            0xfb, 0x16, 0x69, 0x16, 0x03, 0xc1, 0x8e, 0x0e,
            0xbd, 0x33, 0x4f, 0x1d, 0x6e, 0x45, 0xf2, 0x5f,
            0xf7, 0x12, 0xa2, 0x14, 0x57, 0x1f, 0xa5, 0xcc,
            0x97, 0x41, 0x04, 0x84, 0x6d, 0x0a, 0xd3, 0xad,
            0x77, 0x34, 0xec, 0xb3, 0xec, 0xee, 0x4e, 0xef,
            0xef, 0x7a, 0xfd, 0x22, 0x70, 0xe2, 0xe6, 0x0a,
            0xdc, 0xe0, 0xba, 0x2f, 0xac, 0xe6, 0x44, 0x4e,
            0x9a, 0x4b, 0x41, 0xba, 0x73, 0x8d, 0x6c, 0x72,
            0xfb, 0x16, 0x69, 0x16, 0x03, 0xc1, 0x8e, 0x0e ];

        let enc = aessafe::AesSafe192EncryptorX8::new(&key);
        let dec = aessafe::AesSafe192DecryptorX8::new(&key);
        let mut tmp = [0u8; 128];
        enc.encrypt_block_x8(&plain, &mut tmp);
        assert!(tmp[..] == cipher[..]);
        dec.decrypt_block_x8(&cipher, &mut tmp);
        assert!(tmp[..] == plain[..]);
    }

    #[test]
    fn test_aessafe_256_x8() {
        let key: [u8; 32] = [
            0x60, 0x3d, 0xeb, 0x10, 0x15, 0xca, 0x71, 0xbe,
            0x2b, 0x73, 0xae, 0xf0, 0x85, 0x7d, 0x77, 0x81,
            0x1f, 0x35, 0x2c, 0x07, 0x3b, 0x61, 0x08, 0xd7,
            0x2d, 0x98, 0x10, 0xa3, 0x09, 0x14, 0xdf, 0xf4 ];
        let plain: [u8; 128] = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
            0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a,
            0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
            0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51,
            0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
            0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef,
            0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10,
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96,
            0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a,
            0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c,
            0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51,
            0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11,
            0xe5, 0xfb, 0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef,
            0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10 ];
        let cipher: [u8; 128] = [
            0xf3, 0xee, 0xd1, 0xbd, 0xb5, 0xd2, 0xa0, 0x3c,
            0x06, 0x4b, 0x5a, 0x7e, 0x3d, 0xb1, 0x81, 0xf8,
            0x59, 0x1c, 0xcb, 0x10, 0xd4, 0x10, 0xed, 0x26,
            0xdc, 0x5b, 0xa7, 0x4a, 0x31, 0x36, 0x28, 0x70,
            0xb6, 0xed, 0x21, 0xb9, 0x9c, 0xa6, 0xf4, 0xf9,
            0xf1, 0x53, 0xe7, 0xb1, 0xbe, 0xaf, 0xed, 0x1d,
            0x23, 0x30, 0x4b, 0x7a, 0x39, 0xf9, 0xf3, 0xff,
            0x06, 0x7d, 0x8d, 0x8f, 0x9e, 0x24, 0xec, 0xc7,
            0xf3, 0xee, 0xd1, 0xbd, 0xb5, 0xd2, 0xa0, 0x3c,
            0x06, 0x4b, 0x5a, 0x7e, 0x3d, 0xb1, 0x81, 0xf8,
            0x59, 0x1c, 0xcb, 0x10, 0xd4, 0x10, 0xed, 0x26,
            0xdc, 0x5b, 0xa7, 0x4a, 0x31, 0x36, 0x28, 0x70,
            0xb6, 0xed, 0x21, 0xb9, 0x9c, 0xa6, 0xf4, 0xf9,
            0xf1, 0x53, 0xe7, 0xb1, 0xbe, 0xaf, 0xed, 0x1d,
            0x23, 0x30, 0x4b, 0x7a, 0x39, 0xf9, 0xf3, 0xff,
            0x06, 0x7d, 0x8d, 0x8f, 0x9e, 0x24, 0xec, 0xc7 ];

        let enc = aessafe::AesSafe256EncryptorX8::new(&key);
        let dec = aessafe::AesSafe256DecryptorX8::new(&key);
        let mut tmp = [0u8; 128];
        enc.encrypt_block_x8(&plain, &mut tmp);
        assert!(tmp[..] == cipher[..]);
        dec.decrypt_block_x8(&cipher, &mut tmp);
        assert!(tmp[..] == plain[..]);
    }

    #[test]
    fn aes_ctr_box() {
        let tests = aes_ctr_tests();
        for test in tests.iter() {
            let mut aes_enc = aes::ctr(aes::KeySize::KeySize128, &test.key[..], &test.ctr[..]);
            let mut result: Vec<u8> = repeat(0).take(test.plain.len()).collect();
            aes_enc.process(&test.plain[..], &mut result[..]);
            let res: &[u8] = result.as_ref();
            assert!(res == &test.cipher[..]);
        }
    }
}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    use aesni;

    use aessafe;
    use symmetriccipher::{BlockEncryptor, BlockEncryptorX8};
    use util;
    use aes::KeySize::{self, KeySize128, KeySize192, KeySize256};

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[bench]
    pub fn aesni_128_bench(bh: &mut Bencher) {
        aesni_bench(bh, KeySize128);
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[bench]
    pub fn aesni_192_bench(bh: &mut Bencher) {
        aesni_bench(bh, KeySize192);
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[bench]
    pub fn aesni_256_bench(bh: &mut Bencher) {
        aesni_bench(bh, KeySize256);
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    fn aesni_bench(bh: &mut Bencher, key_size: KeySize) {
        if util::supports_aesni() {
            let key: [u8; 16] = [1u8; 16];
            let plain: [u8; 16] = [2u8; 16];

            let a = aesni::AesNiEncryptor::new(key_size, &key);

            let mut tmp = [0u8; 16];

            bh.iter( || {
                a.encrypt_block(&plain, &mut tmp);
            });

            bh.bytes = (plain.len()) as u64;
        }
    }

    #[bench]
    pub fn aes_safe_bench(bh: &mut Bencher) {
        let key: [u8; 16] = [1u8; 16];
        let plain: [u8; 16] = [2u8; 16];

        let a = aessafe::AesSafe128Encryptor::new(&key);

        let mut tmp = [0u8; 16];

        bh.iter( || {
            a.encrypt_block(&plain, &mut tmp);
        });

        bh.bytes = (plain.len()) as u64;
    }

    #[bench]
    pub fn aes_safe_x8_bench(bh: &mut Bencher) {
        let key: [u8; 16] = [1u8; 16];
        let plain: [u8; 128] = [2u8; 128];

        let a = aessafe::AesSafe128EncryptorX8::new(&key);

        let mut tmp = [0u8; 128];

        bh.iter( || {
            a.encrypt_block_x8(&plain, &mut tmp);
        });

        bh.bytes = (plain.len()) as u64;
    }
}
