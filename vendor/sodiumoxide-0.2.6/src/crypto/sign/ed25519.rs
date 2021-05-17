//! `ed25519`, a signature scheme specified in
//! [Ed25519](http://ed25519.cr.yp.to/). This function is conjectured to meet the
//! standard notion of unforgeability for a public-key signature scheme under
//! chosen-message attacks.

use ffi;
use libc::c_ulonglong;
#[cfg(not(feature = "std"))]
use prelude::*;
use std::fmt;
use std::mem;

/// Number of bytes in a `Seed`.
pub const SEEDBYTES: usize = ffi::crypto_sign_ed25519_SEEDBYTES as usize;

/// Number of bytes in a `SecretKey`.
pub const SECRETKEYBYTES: usize = ffi::crypto_sign_ed25519_SECRETKEYBYTES as usize;

/// Number of bytes in a `PublicKey`.
pub const PUBLICKEYBYTES: usize = ffi::crypto_sign_ed25519_PUBLICKEYBYTES as usize;

/// Number of bytes in a `Signature`.
pub const SIGNATUREBYTES: usize = ffi::crypto_sign_ed25519_BYTES as usize;

new_type! {
    /// `Seed` that can be used for keypair generation
    ///
    /// The `Seed` is used by `keypair_from_seed()` to generate
    /// a secret and public signature key.
    ///
    /// When a `Seed` goes out of scope its contents
    /// will be zeroed out
    secret Seed(SEEDBYTES);
}

new_type! {
    /// `SecretKey` for signatures
    ///
    /// When a `SecretKey` goes out of scope its contents
    /// will be zeroed out
    secret SecretKey(SECRETKEYBYTES);
}

impl SecretKey {
    /// `public_key()` computes the corresponding public key for a given secret key
    pub fn public_key(&self) -> PublicKey {
        let mut pk = PublicKey([0u8; PUBLICKEYBYTES]);
        unsafe {
            ffi::crypto_sign_ed25519_sk_to_pk(pk.0.as_mut_ptr(), self.0.as_ptr());
        }
        pk
    }
}

new_type! {
    /// `PublicKey` for signatures
    public PublicKey(PUBLICKEYBYTES);
}

new_type! {
    /// Detached signature
    public Signature(SIGNATUREBYTES);
}

/// `gen_keypair()` randomly generates a secret key and a corresponding public
/// key.
///
/// THREAD SAFETY: `gen_keypair()` is thread-safe provided that you have
/// called `sodiumoxide::init()` once before using any other function
/// from sodiumoxide.
pub fn gen_keypair() -> (PublicKey, SecretKey) {
    let mut pk = PublicKey([0u8; PUBLICKEYBYTES]);
    let mut sk = SecretKey([0u8; SECRETKEYBYTES]);
    unsafe {
        ffi::crypto_sign_ed25519_keypair(pk.0.as_mut_ptr(), sk.0.as_mut_ptr());
    }
    (pk, sk)
}

/// `keypair_from_seed()` computes a secret key and a corresponding public key
/// from a `Seed`.
pub fn keypair_from_seed(seed: &Seed) -> (PublicKey, SecretKey) {
    let mut pk = PublicKey([0u8; PUBLICKEYBYTES]);
    let mut sk = SecretKey([0u8; SECRETKEYBYTES]);
    unsafe {
        ffi::crypto_sign_ed25519_seed_keypair(
            pk.0.as_mut_ptr(),
            sk.0.as_mut_ptr(),
            seed.0.as_ptr(),
        );
    }
    (pk, sk)
}

/// `sign()` signs a message `m` using the signer's secret key `sk`.
/// `sign()` returns the resulting signed message `sm`.
pub fn sign(m: &[u8], sk: &SecretKey) -> Vec<u8> {
    let mut sm = vec![0u8; m.len() + SIGNATUREBYTES];
    let mut smlen = 0;
    unsafe {
        ffi::crypto_sign_ed25519(
            sm.as_mut_ptr(),
            &mut smlen,
            m.as_ptr(),
            m.len() as c_ulonglong,
            sk.0.as_ptr(),
        );
    }
    sm.truncate(smlen as usize);
    sm
}

/// `verify()` verifies the signature in `sm` using the signer's public key `pk`.
/// `verify()` returns the message `Ok(m)`.
/// If the signature fails verification, `verify()` returns `Err(())`.
pub fn verify(sm: &[u8], pk: &PublicKey) -> Result<Vec<u8>, ()> {
    let mut m = vec![0u8; sm.len()];
    let mut mlen = 0;
    let ret = unsafe {
        ffi::crypto_sign_ed25519_open(
            m.as_mut_ptr(),
            &mut mlen,
            sm.as_ptr(),
            sm.len() as c_ulonglong,
            pk.0.as_ptr(),
        )
    };
    if ret == 0 {
        m.truncate(mlen as usize);
        Ok(m)
    } else {
        Err(())
    }
}

/// `sign_detached()` signs a message `m` using the signer's secret key `sk`.
/// `sign_detached()` returns the resulting signature `sig`.
pub fn sign_detached(m: &[u8], sk: &SecretKey) -> Signature {
    let mut sig = Signature([0u8; SIGNATUREBYTES]);
    let mut siglen: c_ulonglong = 0;
    unsafe {
        ffi::crypto_sign_ed25519_detached(
            sig.0.as_mut_ptr(),
            &mut siglen,
            m.as_ptr(),
            m.len() as c_ulonglong,
            sk.0.as_ptr(),
        );
    }
    assert_eq!(siglen, SIGNATUREBYTES as c_ulonglong);
    sig
}

/// `verify_detached()` verifies the signature in `sig` against the message `m`
/// and the signer's public key `pk`.
/// `verify_detached()` returns true if the signature is valid, false otherwise.
pub fn verify_detached(sig: &Signature, m: &[u8], pk: &PublicKey) -> bool {
    let ret = unsafe {
        ffi::crypto_sign_ed25519_verify_detached(
            sig.0.as_ptr(),
            m.as_ptr(),
            m.len() as c_ulonglong,
            pk.0.as_ptr(),
        )
    };
    ret == 0
}

/// State for multi-part (streaming) computation of signature.
#[derive(Copy, Clone)]
pub struct State(ffi::crypto_sign_ed25519ph_state);

impl State {
    /// `init()` initialize a streaming signing state.
    pub fn init() -> State {
        let mut s = mem::MaybeUninit::uninit();
        let state = unsafe {
            ffi::crypto_sign_ed25519ph_init(s.as_mut_ptr());
            s.assume_init() // s is definitely initialized
        };
        State(state)
    }

    /// `update()` can be called more than once in order to compute the digest
    /// from sequential chunks of the message.
    pub fn update(&mut self, m: &[u8]) {
        unsafe {
            ffi::crypto_sign_ed25519ph_update(&mut self.0, m.as_ptr(), m.len() as c_ulonglong);
        }
    }

    /// `finalize()` finalizes the hashing computation and returns a `Signature`.
    // Moves self becuase libsodium says the state should not be used
    // anymore after final().
    pub fn finalize(mut self, &SecretKey(ref sk): &SecretKey) -> Signature {
        let mut sig = [0u8; SIGNATUREBYTES];
        let mut siglen: c_ulonglong = 0;
        unsafe {
            ffi::crypto_sign_ed25519ph_final_create(
                &mut self.0,
                sig.as_mut_ptr(),
                &mut siglen,
                sk.as_ptr(),
            );
        }
        assert_eq!(siglen, SIGNATUREBYTES as c_ulonglong);
        Signature(sig)
    }

    /// `veriry` verifies the signature in `sm` using the signer's public key `pk`.
    pub fn verify(
        &mut self,
        &Signature(ref sig): &Signature,
        &PublicKey(ref pk): &PublicKey,
    ) -> bool {
        let mut sig = *sig;
        let ret = unsafe {
            ffi::crypto_sign_ed25519ph_final_verify(&mut self.0, sig.as_mut_ptr(), pk.as_ptr())
        };
        ret == 0
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ed25519 state")
    }
}

// Impl Default becuase `State` does have a sensible default: State::init()
impl Default for State {
    fn default() -> State {
        State::init()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use hex;

    #[test]
    fn test_sk_to_pk() {
        let (pk, sk) = gen_keypair();
        assert_eq!(sk.public_key(), pk);
    }

    #[test]
    fn test_sign_verify() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let (pk, sk) = gen_keypair();
            let m = randombytes(i);
            let sm = sign(&m, &sk);
            let m2 = verify(&sm, &pk);
            assert!(Ok(m) == m2);
        }
    }

    #[test]
    fn test_sign_verify_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let (pk, sk) = gen_keypair();
            let m = randombytes(i);
            let mut sm = sign(&m, &sk);
            for j in 0..sm.len() {
                sm[j] ^= 0x20;
                assert!(Err(()) == verify(&sm, &pk));
                sm[j] ^= 0x20;
            }
        }
    }

    #[test]
    fn test_sign_verify_detached() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let (pk, sk) = gen_keypair();
            let m = randombytes(i);
            let sig = sign_detached(&m, &sk);
            assert!(verify_detached(&sig, &m, &pk));
        }
    }

    #[test]
    fn test_sign_verify_detached_tamper() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let (pk, sk) = gen_keypair();
            let m = randombytes(i);
            let Signature(mut sig) = sign_detached(&m, &sk);
            for j in 0..SIGNATUREBYTES {
                sig[j] ^= 0x20;
                assert!(!verify_detached(&Signature(sig), &m, &pk));
                sig[j] ^= 0x20;
            }
        }
    }

    #[test]
    fn test_sign_verify_seed() {
        use randombytes::{randombytes, randombytes_into};
        for i in 0..256usize {
            let mut seedbuf = [0; 32];
            randombytes_into(&mut seedbuf);
            let seed = Seed(seedbuf);
            let (pk, sk) = keypair_from_seed(&seed);
            let m = randombytes(i);
            let sm = sign(&m, &sk);
            let m2 = verify(&sm, &pk);
            assert!(Ok(m) == m2);
        }
    }

    #[test]
    fn test_sign_verify_tamper_seed() {
        use randombytes::{randombytes, randombytes_into};
        for i in 0..32usize {
            let mut seedbuf = [0; 32];
            randombytes_into(&mut seedbuf);
            let seed = Seed(seedbuf);
            let (pk, sk) = keypair_from_seed(&seed);
            let m = randombytes(i);
            let mut sm = sign(&m, &sk);
            for j in 0..sm.len() {
                sm[j] ^= 0x20;
                assert!(Err(()) == verify(&sm, &pk));
                sm[j] ^= 0x20;
            }
        }
    }

    #[test]
    fn test_vectors() {
        // test vectors from the Python implementation
        // from the [Ed25519 Homepage](http://ed25519.cr.yp.to/software.html)
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let r = BufReader::new(File::open("testvectors/ed25519.input").unwrap());
        for mline in r.lines() {
            let line = mline.unwrap();
            let mut x = line.split(':');
            let x0 = x.next().unwrap();
            let x1 = x.next().unwrap();
            let x2 = x.next().unwrap();
            let x3 = x.next().unwrap();
            let seed_bytes = hex::decode(&x0[..64]).unwrap();
            let mut seed = Seed([0u8; SEEDBYTES]);
            seed.0.copy_from_slice(&seed_bytes);
            let (pk, sk) = keypair_from_seed(&seed);
            let m = hex::decode(x2).unwrap();
            let sm = sign(&m, &sk);
            verify(&sm, &pk).unwrap();
            assert!(x1 == hex::encode(pk));
            assert!(x3 == hex::encode(sm));
        }
    }

    #[test]
    fn test_vectors_detached() {
        // test vectors from the Python implementation
        // from the [Ed25519 Homepage](http://ed25519.cr.yp.to/software.html)
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let r = BufReader::new(File::open("testvectors/ed25519.input").unwrap());
        for mline in r.lines() {
            let line = mline.unwrap();
            let mut x = line.split(':');
            let x0 = x.next().unwrap();
            let x1 = x.next().unwrap();
            let x2 = x.next().unwrap();
            let x3 = x.next().unwrap();
            let seed_bytes = hex::decode(&x0[..64]).unwrap();
            assert!(seed_bytes.len() == SEEDBYTES);
            let mut seed = Seed([0u8; SEEDBYTES]);
            for (s, b) in seed.0.iter_mut().zip(seed_bytes.iter()) {
                *s = *b
            }
            let (pk, sk) = keypair_from_seed(&seed);
            let m = hex::decode(x2).unwrap();
            let sig = sign_detached(&m, &sk);
            assert!(verify_detached(&sig, &m, &pk));
            assert!(x1 == hex::encode(pk));
            let sm = hex::encode(sig) + x2; // x2 is m hex encoded
            assert!(x3 == sm);
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serialisation() {
        use randombytes::randombytes;
        use test_utils::round_trip;
        for i in 0..256usize {
            let (pk, sk) = gen_keypair();
            let m = randombytes(i);
            let sig = sign_detached(&m, &sk);
            round_trip(pk);
            round_trip(sk);
            round_trip(sig);
        }
    }

    #[test]
    fn test_streaming_sign() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let (pk, sk) = gen_keypair();
            let m = randombytes(i);
            let mut creation_state = State::init();
            creation_state.update(&m);
            let sig = creation_state.finalize(&sk);
            let mut validator_state = State::init();
            validator_state.update(&m);
            assert!(validator_state.verify(&sig, &pk));
        }
    }

    #[test]
    fn test_streaming_empty_sign() {
        let (pk, sk) = gen_keypair();
        let creation_state = State::init();
        let sig = creation_state.finalize(&sk);
        let mut validator_state = State::init();
        assert!(validator_state.verify(&sig, &pk));
    }

    #[test]
    fn test_streaming_vectors() {
        // test vectors from the Python implementation
        // from the [Ed25519 Homepage](http://ed25519.cr.yp.to/software.html)
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let r = BufReader::new(File::open("testvectors/ed25519.input").unwrap());
        for mline in r.lines() {
            let line = mline.unwrap();
            let mut x = line.split(':');
            let x0 = x.next().unwrap();
            let x1 = x.next().unwrap();
            let x2 = x.next().unwrap();
            let seed_bytes = hex::decode(&x0[..64]).unwrap();
            assert!(seed_bytes.len() == SEEDBYTES);
            let mut seed = Seed([0u8; SEEDBYTES]);
            for (s, b) in seed.0.iter_mut().zip(seed_bytes.iter()) {
                *s = *b
            }
            let (pk, sk) = keypair_from_seed(&seed);

            let m = hex::decode(x2).unwrap();

            let mut creation_state = State::init();
            creation_state.update(&m);
            let sig = creation_state.finalize(&sk);

            let mut validator_state = State::init();
            validator_state.update(&m);

            assert!(validator_state.verify(&sig, &pk));

            assert_eq!(x1, hex::encode(pk));
        }
    }

    #[test]
    fn test_streaming_copy() {
        use randombytes::randombytes;
        let i = 256;
        let (pk, sk) = gen_keypair();
        let m = randombytes(i);
        let mut creation_state = State::init();
        creation_state.update(&m);

        let creation_state_copy = creation_state;
        let sig = creation_state_copy.finalize(&sk);
        let mut validator_state = State::init();
        validator_state.update(&m);
        assert!(validator_state.verify(&sig, &pk));
    }

    #[test]
    fn test_streaming_default() {
        use randombytes::randombytes;
        let i = 256;
        let (pk, sk) = gen_keypair();
        let m = randombytes(i);
        let mut creation_state = State::default();
        creation_state.update(&m);

        let sig = creation_state.finalize(&sk);
        let mut validator_state = State::init();
        validator_state.update(&m);
        assert!(validator_state.verify(&sig, &pk));
    }

    #[test]
    fn test_streaming_format() {
        let creation_state = State::init();
        let creation_state_fmt = format!("{:?}", creation_state);
        assert_eq!(creation_state_fmt, "ed25519 state");
    }

    #[test]
    fn test_chunks_sign() {
        use randombytes::randombytes;
        let (pk, sk) = gen_keypair();
        let mut creation_state = State::init();
        let mut validator_state = State::init();
        for i in 0..64usize {
            let chunk = randombytes(i);
            creation_state.update(&chunk);
            validator_state.update(&chunk);
        }
        let sig = creation_state.finalize(&sk);
        assert!(validator_state.verify(&sig, &pk));
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
    fn bench_sign(b: &mut test::Bencher) {
        let (_, sk) = gen_keypair();
        let ms: Vec<Vec<u8>> = BENCH_SIZES.iter().map(|s| randombytes(*s)).collect();
        b.iter(|| {
            for m in ms.iter() {
                sign(m, &sk);
            }
        });
    }

    #[bench]
    fn bench_verify(b: &mut test::Bencher) {
        let (pk, sk) = gen_keypair();
        let sms: Vec<Vec<u8>> = BENCH_SIZES
            .iter()
            .map(|s| {
                let m = randombytes(*s);
                sign(&m, &sk)
            })
            .collect();
        b.iter(|| {
            for sm in sms.iter() {
                verify(sm, &pk);
            }
        });
    }
}
