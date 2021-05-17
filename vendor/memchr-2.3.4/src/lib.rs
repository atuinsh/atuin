/*!
The `memchr` crate provides heavily optimized routines for searching bytes.

The `memchr` function is traditionally provided by libc, however, the
performance of `memchr` can vary significantly depending on the specific
implementation of libc that is used. They can range from manually tuned
Assembly implementations (like that found in GNU's libc) all the way to
non-vectorized C implementations (like that found in MUSL).

To smooth out the differences between implementations of libc, at least
on `x86_64` for Rust 1.27+, this crate provides its own implementation of
`memchr` that should perform competitively with the one found in GNU's libc.
The implementation is in pure Rust and has no dependency on a C compiler or an
Assembler.

Additionally, GNU libc also provides an extension, `memrchr`. This crate
provides its own implementation of `memrchr` as well, on top of `memchr2`,
`memchr3`, `memrchr2` and `memrchr3`. The difference between `memchr` and
`memchr2` is that that `memchr2` permits finding all occurrences of two bytes
instead of one. Similarly for `memchr3`.
*/

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![doc(html_root_url = "https://docs.rs/memchr/2.0.0")]

// Supporting 8-bit (or others) would be fine. If you need it, please submit a
// bug report at https://github.com/BurntSushi/rust-memchr
#[cfg(not(any(
    target_pointer_width = "16",
    target_pointer_width = "32",
    target_pointer_width = "64"
)))]
compile_error!("memchr currently not supported on non-32 or non-64 bit");

#[cfg(feature = "std")]
extern crate core;

#[cfg(all(test, all(not(miri), feature = "std")))]
#[macro_use]
extern crate quickcheck;

use core::iter::Rev;

pub use iter::{Memchr, Memchr2, Memchr3};

// N.B. If you're looking for the cfg knobs for libc, see build.rs.
#[cfg(memchr_libc)]
mod c;
#[allow(dead_code)]
mod fallback;
mod iter;
mod naive;
#[cfg(all(test, all(not(miri), feature = "std")))]
mod tests;
#[cfg(all(test, any(miri, not(feature = "std"))))]
#[path = "tests/miri.rs"]
mod tests;
#[cfg(all(not(miri), target_arch = "x86_64", memchr_runtime_simd))]
mod x86;

/// An iterator over all occurrences of the needle in a haystack.
#[inline]
pub fn memchr_iter(needle: u8, haystack: &[u8]) -> Memchr {
    Memchr::new(needle, haystack)
}

/// An iterator over all occurrences of the needles in a haystack.
#[inline]
pub fn memchr2_iter(needle1: u8, needle2: u8, haystack: &[u8]) -> Memchr2 {
    Memchr2::new(needle1, needle2, haystack)
}

/// An iterator over all occurrences of the needles in a haystack.
#[inline]
pub fn memchr3_iter(
    needle1: u8,
    needle2: u8,
    needle3: u8,
    haystack: &[u8],
) -> Memchr3 {
    Memchr3::new(needle1, needle2, needle3, haystack)
}

/// An iterator over all occurrences of the needle in a haystack, in reverse.
#[inline]
pub fn memrchr_iter(needle: u8, haystack: &[u8]) -> Rev<Memchr> {
    Memchr::new(needle, haystack).rev()
}

/// An iterator over all occurrences of the needles in a haystack, in reverse.
#[inline]
pub fn memrchr2_iter(
    needle1: u8,
    needle2: u8,
    haystack: &[u8],
) -> Rev<Memchr2> {
    Memchr2::new(needle1, needle2, haystack).rev()
}

/// An iterator over all occurrences of the needles in a haystack, in reverse.
#[inline]
pub fn memrchr3_iter(
    needle1: u8,
    needle2: u8,
    needle3: u8,
    haystack: &[u8],
) -> Rev<Memchr3> {
    Memchr3::new(needle1, needle2, needle3, haystack).rev()
}

/// Search for the first occurrence of a byte in a slice.
///
/// This returns the index corresponding to the first occurrence of `needle` in
/// `haystack`, or `None` if one is not found.
///
/// While this is operationally the same as something like
/// `haystack.iter().position(|&b| b == needle)`, `memchr` will use a highly
/// optimized routine that can be up to an order of magnitude faster in some
/// cases.
///
/// # Example
///
/// This shows how to find the first position of a byte in a byte string.
///
/// ```
/// use memchr::memchr;
///
/// let haystack = b"the quick brown fox";
/// assert_eq!(memchr(b'k', haystack), Some(8));
/// ```
#[inline]
pub fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    #[cfg(miri)]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        naive::memchr(n1, haystack)
    }

    #[cfg(all(target_arch = "x86_64", memchr_runtime_simd, not(miri)))]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        x86::memchr(n1, haystack)
    }

    #[cfg(all(
        memchr_libc,
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri),
    ))]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        c::memchr(n1, haystack)
    }

    #[cfg(all(
        not(memchr_libc),
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri),
    ))]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        fallback::memchr(n1, haystack)
    }

    if haystack.is_empty() {
        None
    } else {
        imp(needle, haystack)
    }
}

/// Like `memchr`, but searches for either of two bytes instead of just one.
///
/// This returns the index corresponding to the first occurrence of `needle1`
/// or the first occurrence of `needle2` in `haystack` (whichever occurs
/// earlier), or `None` if neither one is found.
///
/// While this is operationally the same as something like
/// `haystack.iter().position(|&b| b == needle1 || b == needle2)`, `memchr2`
/// will use a highly optimized routine that can be up to an order of magnitude
/// faster in some cases.
///
/// # Example
///
/// This shows how to find the first position of either of two bytes in a byte
/// string.
///
/// ```
/// use memchr::memchr2;
///
/// let haystack = b"the quick brown fox";
/// assert_eq!(memchr2(b'k', b'q', haystack), Some(4));
/// ```
#[inline]
pub fn memchr2(needle1: u8, needle2: u8, haystack: &[u8]) -> Option<usize> {
    #[cfg(miri)]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, haystack: &[u8]) -> Option<usize> {
        naive::memchr2(n1, n2, haystack)
    }

    #[cfg(all(target_arch = "x86_64", memchr_runtime_simd, not(miri)))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, haystack: &[u8]) -> Option<usize> {
        x86::memchr2(n1, n2, haystack)
    }

    #[cfg(all(
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri),
    ))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, haystack: &[u8]) -> Option<usize> {
        fallback::memchr2(n1, n2, haystack)
    }

    if haystack.is_empty() {
        None
    } else {
        imp(needle1, needle2, haystack)
    }
}

/// Like `memchr`, but searches for any of three bytes instead of just one.
///
/// This returns the index corresponding to the first occurrence of `needle1`,
/// the first occurrence of `needle2`, or the first occurrence of `needle3` in
/// `haystack` (whichever occurs earliest), or `None` if none are found.
///
/// While this is operationally the same as something like
/// `haystack.iter().position(|&b| b == needle1 || b == needle2 ||
/// b == needle3)`, `memchr3` will use a highly optimized routine that can be
/// up to an order of magnitude faster in some cases.
///
/// # Example
///
/// This shows how to find the first position of any of three bytes in a byte
/// string.
///
/// ```
/// use memchr::memchr3;
///
/// let haystack = b"the quick brown fox";
/// assert_eq!(memchr3(b'k', b'q', b'e', haystack), Some(2));
/// ```
#[inline]
pub fn memchr3(
    needle1: u8,
    needle2: u8,
    needle3: u8,
    haystack: &[u8],
) -> Option<usize> {
    #[cfg(miri)]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, n3: u8, haystack: &[u8]) -> Option<usize> {
        naive::memchr3(n1, n2, n3, haystack)
    }

    #[cfg(all(target_arch = "x86_64", memchr_runtime_simd, not(miri)))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, n3: u8, haystack: &[u8]) -> Option<usize> {
        x86::memchr3(n1, n2, n3, haystack)
    }

    #[cfg(all(
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri),
    ))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, n3: u8, haystack: &[u8]) -> Option<usize> {
        fallback::memchr3(n1, n2, n3, haystack)
    }

    if haystack.is_empty() {
        None
    } else {
        imp(needle1, needle2, needle3, haystack)
    }
}

/// Search for the last occurrence of a byte in a slice.
///
/// This returns the index corresponding to the last occurrence of `needle` in
/// `haystack`, or `None` if one is not found.
///
/// While this is operationally the same as something like
/// `haystack.iter().rposition(|&b| b == needle)`, `memrchr` will use a highly
/// optimized routine that can be up to an order of magnitude faster in some
/// cases.
///
/// # Example
///
/// This shows how to find the last position of a byte in a byte string.
///
/// ```
/// use memchr::memrchr;
///
/// let haystack = b"the quick brown fox";
/// assert_eq!(memrchr(b'o', haystack), Some(17));
/// ```
#[inline]
pub fn memrchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    #[cfg(miri)]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        naive::memrchr(n1, haystack)
    }

    #[cfg(all(target_arch = "x86_64", memchr_runtime_simd, not(miri)))]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        x86::memrchr(n1, haystack)
    }

    #[cfg(all(
        memchr_libc,
        target_os = "linux",
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri)
    ))]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        c::memrchr(n1, haystack)
    }

    #[cfg(all(
        not(all(memchr_libc, target_os = "linux")),
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri),
    ))]
    #[inline(always)]
    fn imp(n1: u8, haystack: &[u8]) -> Option<usize> {
        fallback::memrchr(n1, haystack)
    }

    if haystack.is_empty() {
        None
    } else {
        imp(needle, haystack)
    }
}

/// Like `memrchr`, but searches for either of two bytes instead of just one.
///
/// This returns the index corresponding to the last occurrence of `needle1`
/// or the last occurrence of `needle2` in `haystack` (whichever occurs later),
/// or `None` if neither one is found.
///
/// While this is operationally the same as something like
/// `haystack.iter().rposition(|&b| b == needle1 || b == needle2)`, `memrchr2`
/// will use a highly optimized routine that can be up to an order of magnitude
/// faster in some cases.
///
/// # Example
///
/// This shows how to find the last position of either of two bytes in a byte
/// string.
///
/// ```
/// use memchr::memrchr2;
///
/// let haystack = b"the quick brown fox";
/// assert_eq!(memrchr2(b'k', b'q', haystack), Some(8));
/// ```
#[inline]
pub fn memrchr2(needle1: u8, needle2: u8, haystack: &[u8]) -> Option<usize> {
    #[cfg(miri)]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, haystack: &[u8]) -> Option<usize> {
        naive::memrchr2(n1, n2, haystack)
    }

    #[cfg(all(target_arch = "x86_64", memchr_runtime_simd, not(miri)))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, haystack: &[u8]) -> Option<usize> {
        x86::memrchr2(n1, n2, haystack)
    }

    #[cfg(all(
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri),
    ))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, haystack: &[u8]) -> Option<usize> {
        fallback::memrchr2(n1, n2, haystack)
    }

    if haystack.is_empty() {
        None
    } else {
        imp(needle1, needle2, haystack)
    }
}

/// Like `memrchr`, but searches for any of three bytes instead of just one.
///
/// This returns the index corresponding to the last occurrence of `needle1`,
/// the last occurrence of `needle2`, or the last occurrence of `needle3` in
/// `haystack` (whichever occurs later), or `None` if none are found.
///
/// While this is operationally the same as something like
/// `haystack.iter().rposition(|&b| b == needle1 || b == needle2 ||
/// b == needle3)`, `memrchr3` will use a highly optimized routine that can be
/// up to an order of magnitude faster in some cases.
///
/// # Example
///
/// This shows how to find the last position of any of three bytes in a byte
/// string.
///
/// ```
/// use memchr::memrchr3;
///
/// let haystack = b"the quick brown fox";
/// assert_eq!(memrchr3(b'k', b'q', b'e', haystack), Some(8));
/// ```
#[inline]
pub fn memrchr3(
    needle1: u8,
    needle2: u8,
    needle3: u8,
    haystack: &[u8],
) -> Option<usize> {
    #[cfg(miri)]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, n3: u8, haystack: &[u8]) -> Option<usize> {
        naive::memrchr3(n1, n2, n3, haystack)
    }

    #[cfg(all(target_arch = "x86_64", memchr_runtime_simd, not(miri)))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, n3: u8, haystack: &[u8]) -> Option<usize> {
        x86::memrchr3(n1, n2, n3, haystack)
    }

    #[cfg(all(
        not(all(target_arch = "x86_64", memchr_runtime_simd)),
        not(miri),
    ))]
    #[inline(always)]
    fn imp(n1: u8, n2: u8, n3: u8, haystack: &[u8]) -> Option<usize> {
        fallback::memrchr3(n1, n2, n3, haystack)
    }

    if haystack.is_empty() {
        None
    } else {
        imp(needle1, needle2, needle3, haystack)
    }
}
