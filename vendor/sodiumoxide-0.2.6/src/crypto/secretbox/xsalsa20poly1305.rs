//! `crypto_secretbox_xsalsa20poly1305`, a particular
//! combination of Salsa20 and Poly1305 specified in
//! [Cryptography in `NaCl`](http://nacl.cr.yp.to/valid.html).
//!
//! This function is conjectured to meet the standard notions of privacy and
//! authenticity.

use crypto::nonce::gen_random_nonce;
use ffi;
#[cfg(not(feature = "std"))]
use prelude::*;
use randombytes::randombytes_into;

/// Number of bytes in `Key`.
pub const KEYBYTES: usize = ffi::crypto_secretbox_xsalsa20poly1305_KEYBYTES as usize;

/// Number of bytes in a `Nonce`.
pub const NONCEBYTES: usize = ffi::crypto_secretbox_xsalsa20poly1305_NONCEBYTES as usize;

new_type! {
    /// `Key` for symmetric authenticated encryption
    ///
    /// When a `Key` goes out of scope its contents
    /// will be zeroed out
    secret Key(KEYBYTES);
}

new_type! {
    /// Authentication `Tag` for the detached encryption mode
    ///
    /// In the combined mode, the tag occupies the first MACBYTES bytes of the ciphertext.
    public Tag(MACBYTES);
}

new_type! {
    /// `Nonce` for symmetric authenticated encryption
    nonce Nonce(NONCEBYTES);
}

/// Number of bytes in the authenticator tag of an encrypted message
/// i.e. the number of bytes by which the ciphertext is larger than the
/// plaintext.
pub const MACBYTES: usize = ffi::crypto_secretbox_xsalsa20poly1305_MACBYTES as usize;

/// `gen_key()` randomly generates a secret key
///
/// THREAD SAFETY: `gen_key()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_key() -> Key {
    let mut key = [0; KEYBYTES];
    randombytes_into(&mut key);
    Key(key)
}

/// `gen_nonce()` randomly generates a nonce
///
/// THREAD SAFETY: `gen_key()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_nonce() -> Nonce {
    gen_random_nonce()
}

/// `seal()` encrypts and authenticates a message `m` using a secret key `k` and a
/// nonce `n`.  It returns a ciphertext `c`.
pub fn seal(m: &[u8], n: &Nonce, k: &Key) -> Vec<u8> {
    let clen = m.len() + MACBYTES;
    let mut c = Vec::with_capacity(clen);
    unsafe {
        c.set_len(clen);
        ffi::crypto_secretbox_easy(
            c.as_mut_ptr(),
            m.as_ptr(),
            m.len() as u64,
            n.0.as_ptr(),
            k.0.as_ptr(),
        );
    }
    c
}

/// `seal_detached()` encrypts and authenticates a message `m` using a secret key `k` and a nonce
/// `n`.  `m` is encrypted in place, so after this function returns it will contain the ciphertext.
/// The detached authentication tag is returned by value.
pub fn seal_detached(m: &mut [u8], n: &Nonce, k: &Key) -> Tag {
    let mut tag = [0; MACBYTES];
    unsafe {
        ffi::crypto_secretbox_detached(
            m.as_mut_ptr(),
            tag.as_mut_ptr(),
            m.as_ptr(),
            m.len() as u64,
            n.0.as_ptr(),
            k.0.as_ptr(),
        );
    };
    Tag(tag)
}

/// `open()` verifies and decrypts a ciphertext `c` using a secret key `k` and a nonce `n`.
/// It returns a plaintext `Ok(m)`.
/// If the ciphertext fails verification, `open()` returns `Err(())`.
pub fn open(c: &[u8], n: &Nonce, k: &Key) -> Result<Vec<u8>, ()> {
    if c.len() < MACBYTES {
        return Err(());
    }
    let mlen = c.len() - MACBYTES;
    let mut m = Vec::with_capacity(mlen);
    let ret = unsafe {
        m.set_len(mlen);
        ffi::crypto_secretbox_open_easy(
            m.as_mut_ptr(),
            c.as_ptr(),
            c.len() as u64,
            n.0.as_ptr(),
            k.0.as_ptr(),
        )
    };
    if ret == 0 {
        Ok(m)
    } else {
        Err(())
    }
}

/// `open_detached()` verifies and decrypts a ciphertext `c` and and authentication tag `tag`,
/// using a secret key `k` and a nonce `n`. `c` is decrypted in place, so if this function is
/// successful it will contain the plaintext. If the ciphertext fails verification,
/// `open_detached()` returns `Err(())`, and the ciphertext is not modified.
pub fn open_detached(c: &mut [u8], tag: &Tag, n: &Nonce, k: &Key) -> Result<(), ()> {
    let ret = unsafe {
        ffi::crypto_secretbox_open_detached(
            c.as_mut_ptr(),
            c.as_ptr(),
            tag.0.as_ptr(),
            c.len() as u64,
            n.0.as_ptr(),
            k.0.as_ptr(),
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_seal_open() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let n = gen_nonce();
            let c = seal(&m, &n, &k);
            let opened = open(&c, &n, &k);
            assert_eq!(Ok(m), opened);
        }
    }

    #[test]
    fn test_seal_open_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let k = gen_key();
            let m = randombytes(i);
            let n = gen_nonce();
            let mut c = seal(&m, &n, &k);
            for i in 0..c.len() {
                c[i] ^= 0x20;
                // Test the combined mode.
                assert_eq!(Err(()), open(&c, &n, &k));
                // Test the detached mode.
                let tag = Tag::from_slice(&c[..MACBYTES]).unwrap();
                assert_eq!(Err(()), open_detached(&mut c[MACBYTES..], &tag, &n, &k));
                c[i] ^= 0x20;
            }
        }
    }

    #[test]
    fn test_seal_open_detached() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let n = gen_nonce();
            let mut buf = m.clone();
            let tag = seal_detached(&mut buf, &n, &k);
            open_detached(&mut buf, &tag, &n, &k).unwrap();
            assert_eq!(m, buf);
        }
    }

    #[test]
    fn test_seal_combined_then_open_detached() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let n = gen_nonce();
            let mut c = seal(&m, &n, &k);
            let tag = Tag::from_slice(&c[..MACBYTES]).unwrap();
            let buf = &mut c[MACBYTES..];
            open_detached(buf, &tag, &n, &k).unwrap();
            assert_eq!(buf, &*m);
        }
    }

    #[test]
    fn test_seal_detached_then_open_combined() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let n = gen_nonce();
            let mut buf = vec![0; MACBYTES];
            buf.extend_from_slice(&m);
            let tag = seal_detached(&mut buf[MACBYTES..], &n, &k);
            buf[..MACBYTES].copy_from_slice(&tag.0[..]);
            let opened = open(&buf, &n, &k);
            assert_eq!(Ok(m), opened);
        }
    }

    #[test]
    fn test_seal_open_detached_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let k = gen_key();
            let mut m = randombytes(i);
            let n = gen_nonce();
            let mut tag = seal_detached(&mut m, &n, &k);
            for j in 0..m.len() {
                m[j] ^= 0x20;
                assert_eq!(Err(()), open_detached(&mut m, &tag, &n, &k));
                m[j] ^= 0x20;
            }
            for j in 0..tag.0.len() {
                tag.0[j] ^= 0x20;
                assert_eq!(Err(()), open_detached(&mut m, &tag, &n, &k));
                tag.0[j] ^= 0x20;
            }
        }
    }

    #[test]
    fn test_open_detached_failure_does_not_modify() {
        let mut buf = b"hello world".to_vec();
        let k = gen_key();
        let n = gen_nonce();
        let tag = seal_detached(&mut buf, &n, &k);
        // Flip the last bit in the ciphertext, to break authentication.
        *buf.last_mut().unwrap() ^= 1;
        // Make a copy that we can compare against after the failure below.
        let copy = buf.clone();
        // Now try to open the message. This will fail.
        let failure = open_detached(&mut buf, &tag, &n, &k);
        assert!(failure.is_err());
        // Make sure the input hasn't been touched.
        assert_eq!(
            buf, copy,
            "input should not be modified if authentication fails"
        );
    }

    #[test]
    fn test_vector_1() {
        let firstkey = Key([
            0x1b, 0x27, 0x55, 0x64, 0x73, 0xe9, 0x85, 0xd4, 0x62, 0xcd, 0x51, 0x19, 0x7a, 0x9a,
            0x46, 0xc7, 0x60, 0x09, 0x54, 0x9e, 0xac, 0x64, 0x74, 0xf2, 0x06, 0xc4, 0xee, 0x08,
            0x44, 0xf6, 0x83, 0x89,
        ]);
        let nonce = Nonce([
            0x69, 0x69, 0x6e, 0xe9, 0x55, 0xb6, 0x2b, 0x73, 0xcd, 0x62, 0xbd, 0xa8, 0x75, 0xfc,
            0x73, 0xd6, 0x82, 0x19, 0xe0, 0x03, 0x6b, 0x7a, 0x0b, 0x37,
        ]);
        let m = vec![
            0xbe, 0x07, 0x5f, 0xc5, 0x3c, 0x81, 0xf2, 0xd5, 0xcf, 0x14, 0x13, 0x16, 0xeb, 0xeb,
            0x0c, 0x7b, 0x52, 0x28, 0xc5, 0x2a, 0x4c, 0x62, 0xcb, 0xd4, 0x4b, 0x66, 0x84, 0x9b,
            0x64, 0x24, 0x4f, 0xfc, 0xe5, 0xec, 0xba, 0xaf, 0x33, 0xbd, 0x75, 0x1a, 0x1a, 0xc7,
            0x28, 0xd4, 0x5e, 0x6c, 0x61, 0x29, 0x6c, 0xdc, 0x3c, 0x01, 0x23, 0x35, 0x61, 0xf4,
            0x1d, 0xb6, 0x6c, 0xce, 0x31, 0x4a, 0xdb, 0x31, 0x0e, 0x3b, 0xe8, 0x25, 0x0c, 0x46,
            0xf0, 0x6d, 0xce, 0xea, 0x3a, 0x7f, 0xa1, 0x34, 0x80, 0x57, 0xe2, 0xf6, 0x55, 0x6a,
            0xd6, 0xb1, 0x31, 0x8a, 0x02, 0x4a, 0x83, 0x8f, 0x21, 0xaf, 0x1f, 0xde, 0x04, 0x89,
            0x77, 0xeb, 0x48, 0xf5, 0x9f, 0xfd, 0x49, 0x24, 0xca, 0x1c, 0x60, 0x90, 0x2e, 0x52,
            0xf0, 0xa0, 0x89, 0xbc, 0x76, 0x89, 0x70, 0x40, 0xe0, 0x82, 0xf9, 0x37, 0x76, 0x38,
            0x48, 0x64, 0x5e, 0x07, 0x05,
        ];

        let c_expected = vec![
            0xf3, 0xff, 0xc7, 0x70, 0x3f, 0x94, 0x00, 0xe5, 0x2a, 0x7d, 0xfb, 0x4b, 0x3d, 0x33,
            0x05, 0xd9, 0x8e, 0x99, 0x3b, 0x9f, 0x48, 0x68, 0x12, 0x73, 0xc2, 0x96, 0x50, 0xba,
            0x32, 0xfc, 0x76, 0xce, 0x48, 0x33, 0x2e, 0xa7, 0x16, 0x4d, 0x96, 0xa4, 0x47, 0x6f,
            0xb8, 0xc5, 0x31, 0xa1, 0x18, 0x6a, 0xc0, 0xdf, 0xc1, 0x7c, 0x98, 0xdc, 0xe8, 0x7b,
            0x4d, 0xa7, 0xf0, 0x11, 0xec, 0x48, 0xc9, 0x72, 0x71, 0xd2, 0xc2, 0x0f, 0x9b, 0x92,
            0x8f, 0xe2, 0x27, 0x0d, 0x6f, 0xb8, 0x63, 0xd5, 0x17, 0x38, 0xb4, 0x8e, 0xee, 0xe3,
            0x14, 0xa7, 0xcc, 0x8a, 0xb9, 0x32, 0x16, 0x45, 0x48, 0xe5, 0x26, 0xae, 0x90, 0x22,
            0x43, 0x68, 0x51, 0x7a, 0xcf, 0xea, 0xbd, 0x6b, 0xb3, 0x73, 0x2b, 0xc0, 0xe9, 0xda,
            0x99, 0x83, 0x2b, 0x61, 0xca, 0x01, 0xb6, 0xde, 0x56, 0x24, 0x4a, 0x9e, 0x88, 0xd5,
            0xf9, 0xb3, 0x79, 0x73, 0xf6, 0x22, 0xa4, 0x3d, 0x14, 0xa6, 0x59, 0x9b, 0x1f, 0x65,
            0x4c, 0xb4, 0x5a, 0x74, 0xe3, 0x55, 0xa5,
        ];
        let c = seal(&m, &nonce, &firstkey);
        assert!(c == c_expected);
        let m2 = open(&c, &nonce, &firstkey);
        assert!(Ok(m) == m2);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serialisation() {
        use test_utils::round_trip;
        for _ in 0..256usize {
            let k = gen_key();
            let n = gen_nonce();
            round_trip(k);
            round_trip(n);
        }
    }

    #[test]
    fn test_nonce_length() {
        assert_eq!(192 / 8, gen_nonce().as_ref().len());
    }
}

#[cfg(feature = "benchmarks")]
#[cfg(test)]
mod bench {
    extern crate test;
    use super::*;
    use randombytes::randombytes;

    const BENCH_SIZES: [usize; 14] = [0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

    #[bench]
    fn bench_seal_open(b: &mut test::Bencher) {
        let k = gen_key();
        let n = gen_nonce();
        let ms: Vec<Vec<u8>> = BENCH_SIZES.iter().map(|s| randombytes(*s)).collect();
        b.iter(|| {
            for m in ms.iter() {
                open(&seal(&m, &n, &k), &n, &k).unwrap();
            }
        });
    }
}
