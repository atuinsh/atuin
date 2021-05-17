macro_rules! auth_module (($auth_name:ident,
                           $verify_name:ident,
                           $keybytes:expr,
                           $tagbytes:expr) => (

use libc::c_ulonglong;
use randombytes::randombytes_into;

/// Number of bytes in a `Key`.
pub const KEYBYTES: usize = $keybytes;

/// Number of bytes in a `Tag`.
pub const TAGBYTES: usize = $tagbytes;

new_type! {
    /// Authentication `Key`
    ///
    /// When a `Key` goes out of scope its contents
    /// will be zeroed out
    secret Key(KEYBYTES);
}

new_type! {
    /// Authentication `Tag`
    ///
    /// The tag implements the traits `PartialEq` and `Eq` using constant-time
    /// comparison functions. See `sodiumoxide::utils::memcmp`
    public Tag(TAGBYTES);
}

/// `gen_key()` randomly generates a key for authentication
///
/// THREAD SAFETY: `gen_key()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_key() -> Key {
    let mut k = [0; KEYBYTES];
    randombytes_into(&mut k);
    Key(k)
}

/// `authenticate()` authenticates a message `m` using a secret key `k`.
/// The function returns an authenticator tag.
pub fn authenticate(m: &[u8],
                    k: &Key) -> Tag {
    unsafe {
        let mut tag = [0; TAGBYTES];
        $auth_name(tag.as_mut_ptr(),
                   m.as_ptr(),
                   m.len() as c_ulonglong,
                   k.0.as_ptr());
        Tag(tag)
    }
}

/// `verify()` returns `true` if `tag` is a correct authenticator of message `m`
/// under a secret key `k`. Otherwise it returns false.
pub fn verify(tag: &Tag, m: &[u8],
              k: &Key) -> bool {
    unsafe {
        $verify_name(tag.0.as_ptr(),
                     m.as_ptr(),
                     m.len() as c_ulonglong,
                     k.0.as_ptr()) == 0
    }
}

#[cfg(test)]
mod test_m {
    use super::*;

    #[test]
    fn test_auth_verify() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let tag = authenticate(&m, &k);
            assert!(verify(&tag, &m, &k));
        }
    }

    #[test]
    fn test_auth_verify_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let k = gen_key();
            let mut m = randombytes(i);
            let Tag(mut tagbuf) = authenticate(&m, &k);
            for j in 0..m.len() {
                m[j] ^= 0x20;
                assert!(!verify(&Tag(tagbuf), &m, &k));
                m[j] ^= 0x20;
            }
            for j in 0..tagbuf.len() {
                tagbuf[j] ^= 0x20;
                assert!(!verify(&Tag(tagbuf), &m, &k));
                tagbuf[j] ^= 0x20;
            }
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serialisation() {
        use randombytes::randombytes;
        use test_utils::round_trip;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let tag = authenticate(&m, &k);
            round_trip(k);
            round_trip(tag);
        }
    }
}

#[cfg(feature = "benchmarks")]
#[cfg(test)]
mod bench_m {
    extern crate test;
    use randombytes::randombytes;
    use super::*;

    const BENCH_SIZES: [usize; 14] = [0, 1, 2, 4, 8, 16, 32, 64,
                                      128, 256, 512, 1024, 2048, 4096];

    #[bench]
    fn bench_auth(b: &mut test::Bencher) {
        let k = gen_key();
        let ms: Vec<Vec<u8>> = BENCH_SIZES.iter().map(|s| {
            randombytes(*s)
        }).collect();
        b.iter(|| {
            for m in ms.iter() {
                authenticate(&m, &k);
            }
        });
    }

    #[bench]
    fn bench_verify(b: &mut test::Bencher) {
        let k = gen_key();
        let ms: Vec<Vec<u8>> = BENCH_SIZES.iter().map(|s| {
            randombytes(*s)
        }).collect();
        let tags: Vec<Tag> = ms.iter().map(|m| {
            authenticate(&m, &k)
        }).collect();
        b.iter(|| {
            for (m, t) in ms.iter().zip(tags.iter()) {
                verify(t, &m, &k);
            }
        });
    }
}

));
