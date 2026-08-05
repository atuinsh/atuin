//! Memory-utilities within atuin.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use subtle::{Choice, ConstantTimeEq};

use super::secret_box::SecretBox;

/// Read handle to the value held in a [`SecretArc`].
///
/// Ensure you do not hold this for more than necessary, as it keeps the memory unsafe. See
/// [`SecretArc`].
pub struct SecretArcRead<T: Sized> {
    inner: Arc<SecretBox<T>>,
}

/// A [`std::sync::Arc`] equivalent which is intended to be used for secret data.
///
/// There's a couple things this features over the standard [`std::sync::Arc`]:
///
/// Note that the access patterns are slightly different, requiring you grab a [`SecretArcRead`]
/// handle.
///
/// ## POSIX-specific
///
/// The pages backing the data are:
///
///   - Not visible to other processes.
///   - Guarded by `mprotect`ed canaries preventing buffer overflows poisoning the data.
///   - `mlock`ed to prevent flushing to swap.
///   - Marked `mprotect(PROT_NONE)`, promoting to `PROT_READ` for the lifetime of the read.
///
/// ### Linux
///
/// The pages backing the data are:
///
///   - `madvise`d against being flushed during core dumps.
///   - `madvise`d to be zeroed out before `fork` syscalls in the child process.
///      **Note this may cause unexpected behaviour.**
///
/// ### macOS
///
/// The pages backing the data are:
///
///   - `madvise(MADV_ZERO_WIRED_PAGES)`d to zero-out memory on crash.
///   - `madvise`d to be zeroed out before `fork` syscalls in the child process.
///      **Note this may cause unexpected behaviour.**
///
/// ## Windows
///
/// As always, Windows has the kitchen sink so we have better utilities for this memory. The pages
/// backing this data are:
///
///   - `VirtualLock`ed so they do not get paged to disk.
///   - `VirtualProtect`ed `PAGE_NOACCESS` and promoted to `PAGE_READONLY` only for the duration of
///      a read.
///   - `WerRegisterExcludedMemoryBlock`ed to tell windows not to dump memory.
///   - **Encrypted in memory** until a read handle is created.
pub struct SecretArc<T: Sized> {
    inner: Arc<SecretBox<T>>,
}

impl<T: Sized> SecretArc<T> {
    /// Move `value` into freshly-allocated, hardened memory.
    ///
    /// See [`SecretArc`] for what "hardened" means on each platform. The value
    /// is locked down (unreadable) until a [`read`](Self::read) handle is taken.
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(SecretBox::new(value)),
        }
    }

    /// Take a [`SecretArcRead`] handle, making the secret readable for as long
    /// as the handle (or any other concurrent handle) is alive.
    ///
    /// Hold it for as little time as possible: while any handle is live the
    /// backing pages are readable rather than access-protected.
    pub fn read(&self) -> SecretArcRead<T> {
        self.inner.acquire_read();
        SecretArcRead {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: Sized> Clone for SecretArc<T> {
    /// Cheaply share ownership of the same underlying secret, like
    /// [`Arc::clone`]. No secret bytes are copied.
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: Default> Default for SecretArc<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for SecretArc<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: ConstantTimeEq> ConstantTimeEq for SecretArc<T> {
    /// Constant-time comparison of the two secrets. Preferred over a derived
    /// `PartialEq`, which would short-circuit and leak equality via timing.
    fn ct_eq(&self, other: &Self) -> Choice {
        let this = self.read();
        let that = other.read();
        ConstantTimeEq::ct_eq(&*this, &*that)
    }
}

impl<T: Sized> Deref for SecretArcRead<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: while this handle is alive the reader count is >= 1, so the
        // pages are readable, and the `Arc` keeps the allocation alive. The
        // reference is bounded by `&self`, so it cannot outlive the handle (and
        // thus the readable window).
        unsafe { self.inner.get() }
    }
}

impl<T: Sized> Drop for SecretArcRead<T> {
    fn drop(&mut self) {
        self.inner.release_read();
    }
}

impl<T: Sized> fmt::Debug for SecretArc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretArc([redacted])")
    }
}

impl<T: Sized> fmt::Debug for SecretArcRead<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretArcRead([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::SecretArc;

    #[test]
    fn read_round_trips() {
        let s = SecretArc::new([7u8; 32]);
        assert_eq!(*s.read(), [7u8; 32]);
    }

    #[test]
    fn concurrent_reads_coexist() {
        let s = SecretArc::new(1234u64);
        let a = s.read();
        let b = s.read();
        assert_eq!(*a, 1234);
        assert_eq!(*b, 1234);
        // dropping one reader must not revoke access for the other
        drop(a);
        assert_eq!(*b, 1234);
    }

    #[test]
    fn reads_work_again_after_all_handles_drop() {
        let s = SecretArc::new(99u32);
        assert_eq!(*s.read(), 99); // handle dropped here -> pages re-locked
        assert_eq!(*s.read(), 99); // must re-promote cleanly
    }

    #[test]
    fn clone_shares_the_same_secret() {
        let s = SecretArc::new([0xABu8; 16]);
        let s2 = s.clone();
        assert_eq!(*s.read(), [0xABu8; 16]);
        assert_eq!(*s2.read(), [0xABu8; 16]);
    }

    #[test]
    fn shared_across_threads() {
        let s = SecretArc::new([0xCDu8; 16]);
        let s2 = s.clone();
        let handle = std::thread::spawn(move || *s2.read());
        assert_eq!(*s.read(), [0xCDu8; 16]);
        assert_eq!(handle.join().unwrap(), [0xCDu8; 16]);
    }

    #[test]
    fn debug_is_redacted() {
        let s = SecretArc::new([1u8; 8]);
        assert_eq!(format!("{s:?}"), "SecretArc([redacted])");
        assert_eq!(format!("{:?}", s.read()), "SecretArcRead([redacted])");
    }
}
