//! A hardened container for a single secret value.

use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;

use parking_lot::Mutex;
use subtle::{Choice, ConstantTimeEq};

use super::hardening;

/// A hardened, single-allocation container for a secret `T`.
///
/// Owns one `memsec` guarded allocation (guard pages + canary, `mlock`ed, and
/// `mprotect`ed no-access while idle) holding the secret `T`. Take a
/// [`SecretBoxRead`] via [`read`](Self::read) to access it; the pages are only
/// readable while a read handle is alive.
///
/// This is also the storage shared, behind an [`Arc`](std::sync::Arc), by
/// [`SecretArc`](super::SecretArc).
///
/// Note this only protects secrets stored *inline* in `T` (e.g. `[u8; 32]` or a
/// POD struct). If `T` owns out-of-line data (e.g. `Vec<u8>`), that data lives
/// on the normal heap and is *not* covered by these protections.
pub struct SecretBox<T: Sized> {
    /// Pointer to the secret, inside the guarded allocation.
    ptr: NonNull<T>,
    /// Count of live readers. Serializes the `mprotect` transitions so
    /// concurrent readers don't demote the pages out from under each other.
    readers: Mutex<usize>,
    /// We own a `T` (dropped in place on teardown); make that explicit for the
    /// drop checker, since `NonNull<T>` does not imply ownership.
    _owns: PhantomData<T>,
}

impl<T: Sized> SecretBox<T> {
    /// Move `value` into freshly-allocated, hardened memory, locked down
    /// (unreadable) until a [`read`](Self::read) handle is taken.
    pub fn new(value: T) -> Self {
        // SAFETY: `malloc::<T>` returns a guarded, `mlock`ed, read-write region
        // sized and aligned for `T`, or `None` on failure.
        let ptr = unsafe { memsec::malloc::<T>() }.expect("SecretBox: secure allocation failed");

        // SAFETY: the region is writable, sized for `T`, and unaliased.
        unsafe { ptr.as_ptr().write(value) };

        // Best-effort platform hardening while the pages are still writable:
        // exclude from crash dumps, wipe-on-fork, zero-wired-on-crash, and
        // (Windows only) cipher the value at rest.
        let base = ptr.as_ptr().cast::<u8>();
        hardening::exclude_from_dumps(base, size_of::<T>());
        hardening::advise_wipe_on_fork(base, size_of::<T>());
        hardening::advise_zero_wired_on_crash(base, size_of::<T>());
        if hardening::encrypts::<T>() {
            hardening::encrypt(base, size_of::<T>());
        }

        // SAFETY: `ptr` came from `memsec::malloc`. Lock the pages down until a
        // reader asks for them.
        let ok = unsafe { memsec::mprotect(ptr, memsec::Prot::NoAccess) };
        assert!(ok, "SecretBox: mprotect(NoAccess) failed");

        Self {
            ptr,
            readers: Mutex::new(0),
            _owns: PhantomData,
        }
    }

    /// Take a [`SecretBoxRead`] handle, making the secret readable for as long
    /// as the handle (or any other concurrent handle) is alive.
    ///
    /// Hold it for as little time as possible: while any handle is live the
    /// backing pages are readable rather than access-protected.
    pub fn read(&self) -> SecretBoxRead<'_, T> {
        self.acquire_read();
        SecretBoxRead { boxed: self }
    }

    /// Register a reader, promoting the pages to readable on the first one.
    pub(super) fn acquire_read(&self) {
        let mut readers = self.readers.lock();
        if *readers == 0 {
            if hardening::encrypts::<T>() {
                // Ciphered at rest (Windows): briefly go writable to decrypt.
                // SAFETY: `ptr` came from `memsec::malloc`.
                let ok = unsafe { memsec::mprotect(self.ptr, memsec::Prot::ReadWrite) };
                assert!(ok, "SecretBox: mprotect(ReadWrite) failed");
                hardening::decrypt(self.ptr.as_ptr().cast::<u8>(), size_of::<T>());
            }
            // SAFETY: `ptr` came from `memsec::malloc`.
            let ok = unsafe { memsec::mprotect(self.ptr, memsec::Prot::ReadOnly) };
            assert!(ok, "SecretBox: mprotect(ReadOnly) failed");
        }
        *readers += 1;
    }

    /// Deregister a reader, demoting the pages back to no-access on the last one.
    pub(super) fn release_read(&self) {
        let mut readers = self.readers.lock();
        *readers -= 1;
        if *readers == 0 {
            // Best-effort: this runs from a read handle's `drop`, so we must not
            // panic — a failed re-lock weakens protection but is not unsound
            // (unlike a failed promote in `acquire_read`, which would fault a
            // read).
            if hardening::encrypts::<T>() {
                // Re-cipher at rest (Windows): go writable, encrypt, re-lock.
                // SAFETY: `ptr` came from `memsec::malloc`.
                let _ = unsafe { memsec::mprotect(self.ptr, memsec::Prot::ReadWrite) };
                hardening::encrypt(self.ptr.as_ptr().cast::<u8>(), size_of::<T>());
            }
            // SAFETY: `ptr` came from `memsec::malloc`.
            let _relocked = unsafe { memsec::mprotect(self.ptr, memsec::Prot::NoAccess) };
        }
    }

    /// Borrow the secret.
    ///
    /// # Safety
    ///
    /// A read must be active (i.e. a live read handle acquired via
    /// [`acquire_read`](Self::acquire_read)), so the pages are readable, and the
    /// returned reference must not outlive that read window.
    pub(super) unsafe fn get(&self) -> &T {
        // SAFETY: upheld by the caller's contract — the pages are readable and
        // the returned lifetime is bounded by the read handle.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: Sized> Drop for SecretBox<T> {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `memsec::malloc`. Make the region writable so
        // `T`'s destructor can run, drop the value in place, then `free` — which
        // zeroes the bytes and unlocks the pages before releasing them.
        unsafe {
            memsec::mprotect(self.ptr, memsec::Prot::ReadWrite);
            // No readers remain at drop, so the value is ciphered at rest on
            // platforms that encrypt; decrypt so the destructor sees plaintext
            // and `free` zeroes plaintext.
            if hardening::encrypts::<T>() {
                hardening::decrypt(self.ptr.as_ptr().cast::<u8>(), size_of::<T>());
            }
            self.ptr.as_ptr().drop_in_place();
            memsec::free(self.ptr);
        }
    }
}

impl<T: Sized> fmt::Debug for SecretBox<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBox([redacted])")
    }
}

// SAFETY: the secret is owned by the box. Moving it across threads is sound when
// `T: Send`; sharing `&SecretBox` (and the `&T` a read hands out) across threads
// is sound when `T: Sync`. The `mprotect` transitions are serialized by
// `readers`. These bounds mirror `Arc<T>`'s.
unsafe impl<T: Sized + Send> Send for SecretBox<T> {}
unsafe impl<T: Sized + Sync> Sync for SecretBox<T> {}

impl<T: Clone> Clone for SecretBox<T> {
    /// Deep-clone: allocate a *new* hardened box holding a clone of the secret.
    /// (Unlike [`SecretArc`](super::SecretArc), whose `Clone` shares.)
    fn clone(&self) -> Self {
        Self::new((*self.read()).clone())
    }
}

impl<T: Default> Default for SecretBox<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for SecretBox<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: ConstantTimeEq> ConstantTimeEq for SecretBox<T> {
    /// Constant-time comparison of the two secrets. Preferred over a derived
    /// `PartialEq`, which would short-circuit and leak equality via timing.
    fn ct_eq(&self, other: &Self) -> Choice {
        let this = self.read();
        let that = other.read();
        ConstantTimeEq::ct_eq(&*this, &*that)
    }
}

/// Read handle to the value held in a [`SecretBox`].
///
/// Ensure you do not hold this for more than necessary: while it (or any other
/// concurrent handle) is alive the backing pages are readable rather than
/// access-protected.
pub struct SecretBoxRead<'a, T: Sized> {
    boxed: &'a SecretBox<T>,
}

impl<T: Sized> Deref for SecretBoxRead<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: this handle is alive, so the reader count is >= 1 and the
        // pages are readable. The reference is bounded by `&self`, so it cannot
        // outlive the handle (and thus the readable window).
        unsafe { self.boxed.get() }
    }
}

impl<T: Sized> Drop for SecretBoxRead<'_, T> {
    fn drop(&mut self) {
        self.boxed.release_read();
    }
}

impl<T: Sized> fmt::Debug for SecretBoxRead<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBoxRead([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::SecretBox;

    #[test]
    fn read_round_trips() {
        let s = SecretBox::new([7u8; 32]);
        assert_eq!(*s.read(), [7u8; 32]);
    }

    #[test]
    fn concurrent_reads_coexist() {
        let s = SecretBox::new(1234u64);
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
        let s = SecretBox::new(99u32);
        assert_eq!(*s.read(), 99); // handle dropped here -> pages re-locked
        assert_eq!(*s.read(), 99); // must re-promote cleanly
    }

    #[test]
    fn debug_is_redacted() {
        let s = SecretBox::new([1u8; 8]);
        assert_eq!(format!("{s:?}"), "SecretBox([redacted])");
        assert_eq!(format!("{:?}", s.read()), "SecretBoxRead([redacted])");
    }

    #[test]
    fn clone_is_a_deep_copy() {
        let a = SecretBox::new([9u8; 32]);
        let b = a.clone();
        assert_eq!(*a.read(), *b.read());
    }

    #[test]
    fn from_and_default() {
        let a = SecretBox::from([3u8; 16]);
        assert_eq!(*a.read(), [3u8; 16]);
        let d: SecretBox<u64> = SecretBox::default();
        assert_eq!(*d.read(), 0);
    }

    #[test]
    fn ct_eq_matches_value_equality() {
        use subtle::ConstantTimeEq;
        let a = SecretBox::new(0xABCD_u64);
        let b = SecretBox::new(0xABCD_u64);
        let c = SecretBox::new(0x1234_u64);
        assert!(bool::from(a.ct_eq(&b)));
        assert!(!bool::from(a.ct_eq(&c)));
    }
}
