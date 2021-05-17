/// Macro for defining streaming authenticator tag computation types and functions.
///
/// Parameters:
/// $state_name - The authenticator state type.
///               SAFETY NOTE: This needs to be a type that does not define a `Drop`
///               implementation, otherwise undefined behaviour will occur.
/// $init_name - A function `f(s: *mut $state_name, k: *u8, klen: size_t)` that initializes
///              a state with a key.
/// $update_name - A function `f(s: *mut $state_name, m: *u8, mlen: size_t)` that updates
///                a state with a message chunk.
/// $final_name - A function `f(s: *mut $state_name, t: *u8)` that computes an authenticator tag of length $tagbytes from a $state_name.
/// $tagbytes   - The number of bytes in an authenticator tag.
macro_rules! auth_state (($state_name:ident,
                          $init_name:ident,
                          $update_name:ident,
                          $final_name:ident,
                          $tagbytes:expr) => (

use std::mem;
use ffi;

/// Authentication `State`
///
/// State for multi-part (streaming) authenticator tag (HMAC) computation.
///
/// When a `State` goes out of scope its contents will be zeroed out.
///
/// NOTE: the streaming interface takes variable length keys, as opposed to the
/// simple interface which takes a fixed length key. The streaming interface also does not
/// define its own `Key` type, instead using slices for its `init()` method.
/// The caller of the functions is responsible for zeroing out the key after it's been used
/// (in contrast to the simple interface which defines a `Drop` implementation for `Key`).
///
/// NOTE: these functions are specific to `libsodium` and do not exist in `NaCl`.

#[must_use]
pub struct State($state_name);

impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            ffi::sodium_memzero(&mut self.0 as *mut $state_name as *mut _, mem::size_of_val(&self.0));
        }
    }
}

impl State {
    /// `init()` initializes an authentication structure using a secret key 'k'.
    pub fn init(k: &[u8]) -> State {
        let mut s = mem::MaybeUninit::uninit();
        let state = unsafe {
            $init_name(s.as_mut_ptr(), k.as_ptr(), k.len());
            s.assume_init() // s is definitely initialized
        };
        State(state)
    }

    /// `update()` can be called more than once in order to compute the authenticator
    /// from sequential chunks of the message.
    pub fn update(&mut self, in_: &[u8]) {
        unsafe {
            $update_name(&mut self.0, in_.as_ptr(), in_.len() as c_ulonglong);
        }
    }

    /// `finalize()` finalizes the authenticator computation and returns a `Tag`. `finalize`
    /// consumes the `State` so that it cannot be accidentally reused.
    pub fn finalize(mut self) -> Tag {
        unsafe {
            let mut tag = [0; $tagbytes];
            $final_name(&mut self.0, tag.as_mut_ptr());
            Tag(tag)
        }
    }
}

#[cfg(test)]
mod test_s {
    use super::*;

    #[test]
    fn test_auth_eq_auth_state() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let tag = authenticate(&m, &k);
            let mut state = State::init(k.as_ref());
            state.update(&m);
            let tag2 = state.finalize();
            assert_eq!(tag, tag2);
        }
    }

    #[test]
    fn test_auth_eq_auth_state_chunked() {
        use randombytes::randombytes;
        for i in 0..256usize {
            let k = gen_key();
            let m = randombytes(i);
            let tag = authenticate(&m, &k);
            let mut state = State::init(k.as_ref());
            for c in m.chunks(1) {
                state.update(c);
            }
            let tag2 = state.finalize();
            assert_eq!(tag, tag2);
        }
    }
}
));
