// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use aes::{ctr, KeySize};
use aead::{AeadEncryptor,AeadDecryptor};
use cryptoutil::copy_memory;
use symmetriccipher::SynchronousStreamCipher;
use ghash::{Ghash};
use util::fixed_time_eq;

pub struct AesGcm<'a> {
    cipher: Box<SynchronousStreamCipher + 'a>,
    mac: Ghash,
    finished: bool,
    end_tag: [u8; 16]
}

impl<'a> AesGcm<'a> {
    pub fn new (key_size: KeySize, key: &[u8], nonce: &[u8], aad: &[u8]) -> AesGcm<'a> {
        assert!(key.len() == 16 || key.len() == 24 || key.len() == 32);
        assert!(nonce.len() == 12);

        // GCM technically differs from CTR mode in how role overs are handled
        // GCM only touches the right most 4 bytes while CTR roles all 16 over
        // when the iv is only 96 bits (12 bytes) then 4 bytes of zeros are
        // appended to it meaning you have to encrypt 2^37 bytes (256 gigabytes)
        // of data before a difference crops up.
        // The GCM handles nonces of other lengths by hashing them once with ghash
        // this would cause the roleover behavior to potentially be triggered much
        // earlier preventing the use of generic CTR mode.

        let mut iv = [0u8; 16];
        copy_memory(nonce, &mut iv);
        iv[15] = 1u8;
        let mut cipher = ctr(key_size,key,&iv);
        let temp_block = [0u8; 16];
        let mut final_block = [0u8; 16];
        cipher.process(&temp_block, &mut final_block);
        let mut hash_key =  [0u8; 16];
        let mut encryptor = ctr(key_size,key,&temp_block);
        encryptor.process(&temp_block, &mut hash_key);
        AesGcm {
            cipher: cipher,
            mac:  Ghash::new(&hash_key).input_a(aad),
            finished: false,
            end_tag: final_block
        }
    }
    
}

impl<'a> AeadEncryptor for AesGcm<'static> {
    fn encrypt(&mut self, input: &[u8], output: &mut [u8], tag: &mut [u8]) {
        assert!(input.len() == output.len());
        assert!(!self.finished);
        self.cipher.process(input, output);
        let result = self.mac.input_c(output).result();
        self.finished = true;
        for i in 0..16 {
            tag[i] = result[i] ^ self.end_tag[i];
        }
    }
}

impl<'a> AeadDecryptor for AesGcm<'static> {
    fn decrypt(&mut self, input: &[u8], output: &mut [u8], tag: &[u8])  -> bool {
        assert!(input.len() == output.len());
        assert!(!self.finished);
        self.finished = true;
        let mut calc_tag = self.mac.input_c(input).result();
        for i in 0..16 {
            calc_tag[i] ^= self.end_tag[i];
        }
        if fixed_time_eq(&calc_tag, tag) {
            self.cipher.process(input, output);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod test {
    use aes::KeySize;
    use aes_gcm::AesGcm;
    use aead::{AeadEncryptor, AeadDecryptor};
    use serialize::hex::FromHex;
    use std::iter::repeat;
    fn hex_to_bytes(raw_hex: &str) -> Vec<u8> {
        raw_hex.from_hex().ok().unwrap()
    }
    struct TestVector {
                key:  Vec<u8>,
                iv:  Vec<u8>,
                plain_text: Vec<u8>,
                cipher_text:  Vec<u8>,
                aad: Vec<u8>,
                tag:  Vec<u8>,
            }

    fn get_test_vectors()-> [TestVector; 5]{
      [
        TestVector {
                key: hex_to_bytes("00000000000000000000000000000000"),
                iv: hex_to_bytes("000000000000000000000000"),
                plain_text: hex_to_bytes(""),
                cipher_text: hex_to_bytes(""),
                aad: hex_to_bytes(""),
                tag: hex_to_bytes("58e2fccefa7e3061367f1d57a4e7455a")
            },
            TestVector {
                key: hex_to_bytes("00000000000000000000000000000000"),
                iv: hex_to_bytes("000000000000000000000000"),
                plain_text: hex_to_bytes("00000000000000000000000000000000"),
                cipher_text: hex_to_bytes("0388dace60b6a392f328c2b971b2fe78"),
                aad: hex_to_bytes(""),
                tag: hex_to_bytes("ab6e47d42cec13bdf53a67b21257bddf")
            },
            TestVector {
                key: hex_to_bytes("feffe9928665731c6d6a8f9467308308"),
                iv: hex_to_bytes("cafebabefacedbaddecaf888"),
                plain_text: hex_to_bytes("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39"),
                cipher_text: hex_to_bytes("42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091"),
                aad: hex_to_bytes("feedfacedeadbeeffeedfacedeadbeefabaddad2"),
                tag: hex_to_bytes("5bc94fbc3221a5db94fae95ae7121a47")
            },
            TestVector {
                key: hex_to_bytes("feffe9928665731c6d6a8f9467308308feffe9928665731c"),
                iv: hex_to_bytes("cafebabefacedbaddecaf888"),
                plain_text: hex_to_bytes("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39"),
                cipher_text: hex_to_bytes("3980ca0b3c00e841eb06fac4872a2757859e1ceaa6efd984628593b40ca1e19c7d773d00c144c525ac619d18c84a3f4718e2448b2fe324d9ccda2710"),
                aad: hex_to_bytes("feedfacedeadbeeffeedfacedeadbeefabaddad2"),
                tag: hex_to_bytes("2519498e80f1478f37ba55bd6d27618c")
            },
            TestVector {
                key: hex_to_bytes("feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308"),
                iv: hex_to_bytes("cafebabefacedbaddecaf888"),
                plain_text: hex_to_bytes("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39"),
                cipher_text: hex_to_bytes("522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662"),
                aad: hex_to_bytes("feedfacedeadbeeffeedfacedeadbeefabaddad2"),
                tag: hex_to_bytes("76fc6ece0f4e1768cddf8853bb2d551b")
            },
    ]
}
    #[test]
    fn aes_gcm_test() {
            
        for item in get_test_vectors().iter() {
            let key_size = match item.key.len() {
                16 => KeySize::KeySize128,
                24 => KeySize::KeySize192,
                32 => KeySize::KeySize256,
                _ => unreachable!()
            };
            let mut cipher = AesGcm::new(key_size, &item.key[..], &item.iv[..], &item.aad[..]);
            let mut out: Vec<u8> = repeat(0).take(item.plain_text.len()).collect();
            
            let mut out_tag: Vec<u8> = repeat(0).take(16).collect();
            
            cipher.encrypt(&item.plain_text[..], &mut out[..],&mut out_tag[..]);
            assert_eq!(out, item.cipher_text);
            assert_eq!(out_tag, item.tag);
        }
    }

    #[test]
    fn aes_gcm_decrypt_test() {
            
        for item in get_test_vectors().iter() {
            let key_size = match item.key.len() {
                16 => KeySize::KeySize128,
                24 => KeySize::KeySize192,
                32 => KeySize::KeySize256,
                _ => unreachable!()
            };
            let mut decipher = AesGcm::new(key_size, &item.key[..], &item.iv[..], &item.aad[..]);
            let mut out: Vec<u8> = repeat(0).take(item.plain_text.len()).collect();
                        
            let result = decipher.decrypt(&item.cipher_text[..], &mut out[..], &item.tag[..]);
            assert_eq!(out, item.plain_text);
            assert!(result);
        }
    }
    #[test]
    fn aes_gcm_decrypt_fail_test() {
            
        for item in get_test_vectors().iter() {
            let key_size = match item.key.len() {
                16 => KeySize::KeySize128,
                24 => KeySize::KeySize192,
                32 => KeySize::KeySize256,
                _ => unreachable!()
            };
            let mut decipher = AesGcm::new(key_size, &item.key[..], &item.iv[..], &item.aad[..]);
            let tag: Vec<u8> = repeat(0).take(16).collect();
            let mut out1: Vec<u8> = repeat(0).take(item.plain_text.len()).collect();
            let out2: Vec<u8> = repeat(0).take(item.plain_text.len()).collect();
            let result = decipher.decrypt(&item.cipher_text[..], &mut out1[..], &tag[..]);
            assert_eq!(out1, out2);
            assert!(!result);
        }
    }

}

#[cfg(all(test, feature = "with-bench"))]
mod bench {
    use test::Bencher;
    use aes::KeySize;
    use aes_gcm::AesGcm;
    use aead::{AeadEncryptor, AeadDecryptor};

    #[bench]
    pub fn gsm_10(bh: & mut Bencher) {
    	let input = [1u8; 10];
    	let aad = [3u8; 10];
    	bh.iter( || {
	        let mut cipher = AesGcm::new(KeySize::KeySize256, &[0; 32], &[0; 12], &aad);
	        let mut decipher = AesGcm::new(KeySize::KeySize256, &[0; 32], &[0; 12], &aad);
	        
	        let mut output = [0u8; 10];
	        let mut tag = [0u8; 16];
	        let mut output2 = [0u8; 10];
            cipher.encrypt(&input, &mut output, &mut tag);
            decipher.decrypt(&output, &mut output2, &tag);
            
        });
        bh.bytes = 10u64;
    }
        

    #[bench]
    pub fn gsm_1k(bh: & mut Bencher) {
    	let input = [1u8; 1024];
    	let aad = [3u8; 1024];
    	bh.iter( || {
        let mut cipher = AesGcm::new(KeySize::KeySize256, &[0; 32], &[0; 12], &aad);
        let mut decipher = AesGcm::new(KeySize::KeySize256, &[0; 32], &[0; 12], &aad);
        
        let mut output = [0u8; 1024];
        let mut tag = [0u8; 16];
        let mut output2 = [0u8; 1024];
        
            cipher.encrypt(&input, &mut output, &mut tag);
            decipher.decrypt(&output, &mut output2, &tag);
        });
    	bh.bytes = 1024u64;
        
    }

    #[bench]
    pub fn gsm_64k(bh: & mut Bencher) {
    	let input = [1u8; 65536];
    	let aad = [3u8; 65536];
    	  bh.iter( || {
        let mut cipher = AesGcm::new(KeySize::KeySize256, &[0; 32], &[0; 12], &aad);
        let mut decipher = AesGcm::new(KeySize::KeySize256, &[0; 32], &[0; 12], &aad);
        
        let mut output = [0u8; 65536];
        let mut tag = [0u8; 16];
        let mut output2 = [0u8; 65536];
      
            cipher.encrypt(&input, &mut output, &mut tag);
            decipher.decrypt(&output, &mut output2, &tag);

        });
    	   bh.bytes = 65536u64;
        
    }
}
