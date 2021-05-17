macro_rules! stream_module (($stream_name:ident,
                             $xor_name:ident,
                             $xor_ic_name:ident,
                             $keybytes:expr,
                             $noncebytes:expr) => (

#[cfg(not(feature = "std"))] use prelude::*;
use libc::c_ulonglong;
use randombytes::randombytes_into;

/// Number of bytes in a `Key`.
pub const KEYBYTES: usize = $keybytes;

/// Number of bytes in a `Nonce`.
pub const NONCEBYTES: usize = $noncebytes;

new_type! {
    /// `Key` for symmetric encryption
    ///
    /// When a `Key` goes out of scope its contents
    /// will be zeroed out
    secret Key(KEYBYTES);
}

new_type! {
    /// `Nonce` for symmetric encryption
    nonce Nonce(NONCEBYTES);
}

/// `gen_key()` randomly generates a key for symmetric encryption
///
/// THREAD SAFETY: `gen_key()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_key() -> Key {
    let mut key = [0; KEYBYTES];
    randombytes_into(&mut key);
    Key(key)
}

/// `stream()` produces a `len`-byte stream `c` as a function of a
/// secret key `k` and a nonce `n`.
pub fn stream(len: usize,
              n: &Nonce,
              k: &Key) -> Vec<u8> {
    unsafe {
        let mut c = vec![0u8; len];
        $stream_name(c.as_mut_ptr(),
                     c.len() as c_ulonglong,
                     n.0.as_ptr(),
                     k.0.as_ptr());
        c
    }
}

/// `stream_xor()` encrypts a message `m` using a secret key `k` and a nonce `n`.
/// The `stream_xor()` function returns the ciphertext `c`.
///
/// `stream_xor()` guarantees that the ciphertext has the same length as the plaintext,
/// and is the plaintext xor the output of `stream()`.
/// Consequently `stream_xor()` can also be used to decrypt.
pub fn stream_xor(m: &[u8],
                  n: &Nonce,
                  k: &Key) -> Vec<u8> {
    unsafe {
        let mut c = vec![0u8; m.len()];
        $xor_name(c.as_mut_ptr(),
                  m.as_ptr(),
                  m.len() as c_ulonglong,
                  n.0.as_ptr(),
                  k.0.as_ptr());
        c
    }
}

/// `stream_xor_inplace()` encrypts a message `m` using a secret key `k` and a nonce `n`.
/// The `stream_xor_inplace()` function encrypts the message in place.
///
/// `stream_xor_inplace()` guarantees that the ciphertext has the same length as
/// the plaintext, and is the plaintext xor the output of `stream_inplace()`.
/// Consequently `stream_xor_inplace()` can also be used to decrypt.
pub fn stream_xor_inplace(m: &mut [u8],
                          n: &Nonce,
                          k: &Key) {
    unsafe {
        $xor_name(m.as_mut_ptr(),
                  m.as_ptr(),
                  m.len() as c_ulonglong,
                  n.0.as_ptr(),
                  k.0.as_ptr());
    }
}

/// `stream_xor_ic()` encrypts a message `m` using a secret key `k` and a nonce `n`,
/// it is similar to `stream_xor()` but allows the caller to set the value of the initial
/// block counter `ic`.
///
/// `stream_xor()` guarantees that the ciphertext has the same length as the plaintext,
/// and is the plaintext xor the output of `stream()`.
/// Consequently `stream_xor()` can also be used to decrypt.
pub fn stream_xor_ic(m: &[u8],
                     n: &Nonce,
                     ic: u64,
                     k: &Key) -> Vec<u8> {
    unsafe {
        let mut c = vec![0u8; m.len()];
        $xor_ic_name(c.as_mut_ptr(),
                     m.as_ptr(),
                     m.len() as c_ulonglong,
                     n.0.as_ptr(),
                     ic as u64,
                     k.0.as_ptr());
        c
    }
}

/// `stream_xor_ic_inplace()` encrypts a message `m` using a secret key `k` and a nonce `n`,
/// it is similar to `stream_xor_inplace()` but allows the caller to set the value of the initial
/// block counter `ic`.
/// The `stream_xor_ic_inplace()` function encrypts the message in place.
///
/// `stream_xor_ic_inplace()` guarantees that the ciphertext has the same length as
/// the plaintext, and is the plaintext xor the output of `stream_inplace()`.
/// Consequently `stream_xor_ic_inplace()` can also be used to decrypt.
pub fn stream_xor_ic_inplace(m: &mut [u8],
                             n: &Nonce,
                             ic: u64,
                             k: &Key) {
    unsafe {
        $xor_ic_name(m.as_mut_ptr(),
                     m.as_ptr(),
                     m.len() as c_ulonglong,
                     n.0.as_ptr(),
                     ic as u64,
                     k.0.as_ptr());
    }
}


#[cfg(test)]
mod test_m {
    use super::*;
    use crypto::nonce::gen_random_nonce;

    #[test]
    fn test_encrypt_decrypt() {
        use randombytes::randombytes;
        for i in 0..1024usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let m = randombytes(i);
            let c = stream_xor(&m, &n, &k);
            let m2 = stream_xor(&c, &n, &k);
            assert!(m == m2);
        }
    }

    #[test]
    fn test_stream_xor() {
        use randombytes::randombytes;
        for i in 0..1024usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let m = randombytes(i);
            let mut c = m.clone();
            let s = stream(c.len(), &n, &k);
            for (e, v) in c.iter_mut().zip(s.iter()) {
                *e ^= *v;
            }
            let c2 = stream_xor(&m, &n, &k);
            assert!(c == c2);
        }
    }

    #[test]
    fn test_stream_xor_inplace() {
        use randombytes::randombytes;
        for i in 0..1024usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let mut m = randombytes(i);
            let mut c = m.clone();
            let s = stream(c.len(), &n, &k);
            for (e, v) in c.iter_mut().zip(s.iter()) {
                *e ^= *v;
            }
            stream_xor_inplace(&mut m, &n, &k);
            assert!(c == m);
        }
    }

    #[test]
    fn test_stream_xor_ic_same() {
        use randombytes::randombytes;
        for i in 0..1024usize {
            let k = gen_key();
            let n = gen_random_nonce();
            let m = randombytes(i);
            let c = stream_xor(&m, &n, &k);
            let c_ic = stream_xor_ic(&m, &n, 0, &k);
            assert_eq!(c, c_ic);
        }
    }

    #[test]
    fn test_stream_xor_ic_inplace() {
        use randombytes::randombytes;
        for i in 0..1024usize {
            let k = gen_key();
            let n = gen_random_nonce();
            for j in 0..10 {
                let mut m = randombytes(i);
                let c = stream_xor_ic(&m, &n, j, &k);
                stream_xor_ic_inplace(&mut m, &n, j, &k);
                assert_eq!(m, c);
            }
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serialisation() {
        use test_utils::round_trip;
        for _ in 0..1024usize {
            let k = gen_key();
            let n: Nonce = gen_random_nonce();
            round_trip(k);
            round_trip(n);
        }
    }
}

#[cfg(feature = "benchmarks")]
#[cfg(test)]
mod bench_m {
    extern crate test;
    use super::*;

    const BENCH_SIZES: [usize; 14] = [0, 1, 2, 4, 8, 16, 32, 64,
                                      128, 256, 512, 1024, 2048, 4096];

    #[bench]
    fn bench_stream(b: &mut test::Bencher) {
        let k = gen_key();
        let n = gen_random_nonce();
        b.iter(|| {
            for size in BENCH_SIZES.iter() {
                stream(*size, &n, &k);
            }
        });
    }
}

));
