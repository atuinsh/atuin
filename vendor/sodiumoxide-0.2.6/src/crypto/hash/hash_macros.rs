macro_rules! hash_module (($hash_name:ident,
                           $hash_state:ident,
                           $hash_init:ident,
                           $hash_update:ident,
                           $hash_final:ident,
                           $hashbytes:expr,
                           $blockbytes:expr) => (

use std::mem;
use libc::c_ulonglong;

/// Number of bytes in a `Digest`.
pub const DIGESTBYTES: usize = $hashbytes as usize;

/// Block size of the hash function.
pub const BLOCKBYTES: usize = $blockbytes as usize;

new_type! {
    /// Digest-structure
    public Digest(DIGESTBYTES);
}

/// `hash` hashes a message `m`. It returns a hash `h`.
pub fn hash(m: &[u8]) -> Digest {
    unsafe {
        let mut h = [0; DIGESTBYTES];
        $hash_name(h.as_mut_ptr(), m.as_ptr(), m.len() as c_ulonglong);
        Digest(h)
    }
}

/// `State` contains the state for multi-part (streaming) hash computations. This allows the caller
/// to process a message as a sequence of multiple chunks.
#[derive(Copy, Clone)]
pub struct State($hash_state);

impl State {
    /// `new` constructs and initializes a new `State`.
    pub fn new() -> Self {
        let mut st = mem::MaybeUninit::uninit();
        let state = unsafe {
            $hash_init(st.as_mut_ptr());
            st.assume_init() // st is definitely initialized
        };
        State(state)
    }

    /// `update` updates the `State` with `data`. `update` can be called multiple times in order
    /// to compute the hash from sequential chunks of the message.
    pub fn update(&mut self, data: &[u8]) {
        unsafe {
            $hash_update(&mut self.0, data.as_ptr(), data.len() as c_ulonglong);
        }
    }

    /// `finalize` finalizes the state and returns the digest value. `finalize` consumes the
    /// `State` so that it cannot be accidentally reused.
    pub fn finalize(mut self) -> Digest {
        unsafe {
            let mut digest = Digest([0u8; DIGESTBYTES]);
            $hash_final(&mut self.0, digest.0.as_mut_ptr());
            digest
        }
    }
}

impl Default for State {
    fn default() -> State {
        State::new()
    }
}

#[cfg(test)]
mod test_m {
    use super::*;

    #[test]
    fn test_hash_multipart() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let m = randombytes(i);
            let h = hash(&m);
            let mut state = State::new();
            for b in m.chunks(3) {
                state.update(b);
            }
            let h2 = state.finalize();
            assert_eq!(h, h2);
        }
    }
}

#[cfg(feature = "serde")]
#[cfg(test)]
mod test_encode {
    use super::*;
    use test_utils::round_trip;

    #[test]
    fn test_serialisation() {
        use randombytes::randombytes;
        for i in 0..32usize {
            let m = randombytes(i);
            let d = hash(&m[..]);
            round_trip(d);
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
    fn bench_hash(b: &mut test::Bencher) {
        let ms: Vec<Vec<u8>> = BENCH_SIZES.iter().map(|s| {
            randombytes(*s)
        }).collect();
        b.iter(|| {
            for m in ms.iter() {
                hash(&m);
            }
        });
    }
}

));
