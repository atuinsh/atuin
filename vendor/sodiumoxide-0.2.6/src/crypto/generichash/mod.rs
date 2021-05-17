//! `GenericHash`.
//!
use ffi::{
    crypto_generichash_BYTES_MAX, crypto_generichash_BYTES_MIN, crypto_generichash_KEYBYTES_MAX,
    crypto_generichash_KEYBYTES_MIN, crypto_generichash_final, crypto_generichash_init,
    crypto_generichash_state, crypto_generichash_update,
};

use libc::c_ulonglong;
use std::mem;
use std::ptr;

mod digest;
pub use self::digest::Digest;

/// Minimium of allowed bytes in a `Digest`
pub const DIGEST_MIN: usize = crypto_generichash_BYTES_MIN as usize;

/// Maximum of allowed bytes in a `Digest`
pub const DIGEST_MAX: usize = crypto_generichash_BYTES_MAX as usize;

/// Minimium of allowed bytes in a key
pub const KEY_MIN: usize = crypto_generichash_KEYBYTES_MIN as usize;

/// Maximum of allowed bytes in a key
pub const KEY_MAX: usize = crypto_generichash_KEYBYTES_MAX as usize;

/// `State` contains the state for multi-part (streaming) hash computations. This allows the caller
/// to process a message as a sequence of multiple chunks.
pub struct State {
    out_len: usize,
    state: crypto_generichash_state,
}

impl State {
    /// `new` constructs and initializes a new `State` with the given parameters.
    ///
    /// `out_len` specifies the resulting hash size.
    /// Only values in the interval [`DIGEST_MIN`, `DIGEST_MAX`] are allowed.
    ///
    /// `key` is an optional parameter, which when given,
    /// a custom key can be used for the computation of the hash.
    /// The size of the key must be in the interval [`KEY_MIN`, `KEY_MAX`].
    pub fn new(out_len: usize, key: Option<&[u8]>) -> Result<State, ()> {
        if out_len < DIGEST_MIN || out_len > DIGEST_MAX {
            return Err(());
        }

        if let Some(key) = key {
            let len = key.len();
            if len < KEY_MIN || len > KEY_MAX {
                return Err(());
            }
        }

        let mut state = mem::MaybeUninit::uninit();

        let result = unsafe {
            if let Some(key) = key {
                crypto_generichash_init(state.as_mut_ptr(), key.as_ptr(), key.len(), out_len)
            } else {
                crypto_generichash_init(state.as_mut_ptr(), ptr::null(), 0, out_len)
            }
        };

        if result == 0 {
            // result == 0 and state is initialized
            let state = unsafe { state.assume_init() };
            Ok(State { out_len, state })
        } else {
            Err(())
        }
    }

    /// `update` updates the `State` with `data`. `update` can be called multiple times in order
    /// to compute the hash from sequential chunks of the message.
    pub fn update(&mut self, data: &[u8]) -> Result<(), ()> {
        let rc = unsafe {
            crypto_generichash_update(&mut self.state, data.as_ptr(), data.len() as c_ulonglong)
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(())
        }
    }

    /// `finalize` finalizes the state and returns the digest value. `finalize` consumes the
    /// `State` so that it cannot be accidentally reused.
    pub fn finalize(mut self) -> Result<Digest, ()> {
        let mut result = Digest {
            len: self.out_len,
            data: [0u8; crypto_generichash_BYTES_MAX as usize],
        };
        let rc = unsafe {
            crypto_generichash_final(&mut self.state, result.data.as_mut_ptr(), result.len)
        };
        if rc == 0 {
            Ok(result)
        } else {
            Err(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use hex;
    #[cfg(not(feature = "std"))]
    use prelude::*;

    #[test]
    fn test_vector_1() {
        // hash of empty string
        let x = [];
        let h_expected = [
            0x0e, 0x57, 0x51, 0xc0, 0x26, 0xe5, 0x43, 0xb2, 0xe8, 0xab, 0x2e, 0xb0, 0x60, 0x99,
            0xda, 0xa1, 0xd1, 0xe5, 0xdf, 0x47, 0x77, 0x8f, 0x77, 0x87, 0xfa, 0xab, 0x45, 0xcd,
            0xf1, 0x2f, 0xe3, 0xa8,
        ];
        let mut hasher = State::new(32, None).unwrap();
        hasher.update(&x).unwrap();
        let h = hasher.finalize().unwrap();
        assert!(h.as_ref() == h_expected);
    }

    #[test]
    fn test_vector_2() {
        // The quick brown fox jumps over the lazy dog
        let x = [
            0x54, 0x68, 0x65, 0x20, 0x71, 0x75, 0x69, 0x63, 0x6b, 0x20, 0x62, 0x72, 0x6f, 0x77,
            0x6e, 0x20, 0x66, 0x6f, 0x78, 0x20, 0x6a, 0x75, 0x6d, 0x70, 0x73, 0x20, 0x6f, 0x76,
            0x65, 0x72, 0x20, 0x74, 0x68, 0x65, 0x20, 0x6c, 0x61, 0x7a, 0x79, 0x20, 0x64, 0x6f,
            0x67,
        ];
        let h_expected = [
            0x01, 0x71, 0x8c, 0xec, 0x35, 0xcd, 0x3d, 0x79, 0x6d, 0xd0, 0x00, 0x20, 0xe0, 0xbf,
            0xec, 0xb4, 0x73, 0xad, 0x23, 0x45, 0x7d, 0x06, 0x3b, 0x75, 0xef, 0xf2, 0x9c, 0x0f,
            0xfa, 0x2e, 0x58, 0xa9,
        ];
        let mut hasher = State::new(32, None).unwrap();
        hasher.update(&x).unwrap();
        let h = hasher.finalize().unwrap();
        assert!(h.as_ref() == h_expected);
    }

    #[test]
    fn test_blake2b_vectors() {
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let mut r = BufReader::new(File::open("testvectors/blake2b-kat.txt").unwrap());
        let mut line = String::new();

        loop {
            let msg = {
                line.clear();
                r.read_line(&mut line).unwrap();
                if line.is_empty() {
                    break;
                }

                match line.len() {
                    0 => break,
                    1..=3 => continue,
                    _ => {}
                }

                assert!(line.starts_with("in:"));
                hex::decode(line[3..].trim()).unwrap()
            };

            let key = {
                line.clear();
                r.read_line(&mut line).unwrap();
                assert!(line.starts_with("key:"));
                hex::decode(line[4..].trim()).unwrap()
            };

            let expected_hash = {
                line.clear();
                r.read_line(&mut line).unwrap();
                assert!(line.starts_with("hash:"));
                hex::decode(&line[5..].trim()).unwrap()
            };

            let mut hasher = State::new(64, Some(&key)).unwrap();
            hasher.update(&msg).unwrap();

            let result_hash = hasher.finalize().unwrap();
            assert!(result_hash.as_ref() == expected_hash.as_slice());
        }
    }

    #[test]
    fn test_digest_equality() {
        let data1 = [1, 2];
        let data2 = [3, 4];

        let h1 = {
            let mut hasher = State::new(32, None).unwrap();
            hasher.update(&data1).unwrap();
            hasher.finalize().unwrap()
        };

        let h2 = {
            let mut hasher = State::new(32, None).unwrap();
            hasher.update(&data2).unwrap();
            hasher.finalize().unwrap()
        };

        assert_eq!(h1, h1);
        assert_ne!(h1, h2);
    }
}
