use std::{
    cell::UnsafeCell,
    hint,
    panic::{RefUnwindSafe, UnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
};

use parking_lot::Mutex;

use crate::take_unchecked;

pub(crate) struct OnceCell<T> {
    mutex: Mutex<()>,
    is_initialized: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

// Why do we need `T: Send`?
// Thread A creates a `OnceCell` and shares it with
// scoped thread B, which fills the cell, which is
// then destroyed by A. That is, destructor observes
// a sent value.
unsafe impl<T: Sync + Send> Sync for OnceCell<T> {}
unsafe impl<T: Send> Send for OnceCell<T> {}

impl<T: RefUnwindSafe + UnwindSafe> RefUnwindSafe for OnceCell<T> {}
impl<T: UnwindSafe> UnwindSafe for OnceCell<T> {}

impl<T> OnceCell<T> {
    pub(crate) const fn new() -> OnceCell<T> {
        OnceCell {
            mutex: parking_lot::const_mutex(()),
            is_initialized: AtomicBool::new(false),
            value: UnsafeCell::new(None),
        }
    }

    /// Safety: synchronizes with store to value via Release/Acquire.
    #[inline]
    pub(crate) fn is_initialized(&self) -> bool {
        self.is_initialized.load(Ordering::Acquire)
    }

    /// Safety: synchronizes with store to value via `is_initialized` or mutex
    /// lock/unlock, writes value only once because of the mutex.
    #[cold]
    pub(crate) fn initialize<F, E>(&self, f: F) -> Result<(), E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let mut f = Some(f);
        let mut res: Result<(), E> = Ok(());
        let slot: *mut Option<T> = self.value.get();
        initialize_inner(&self.mutex, &self.is_initialized, &mut || {
            // We are calling user-supplied function and need to be careful.
            // - if it returns Err, we unlock mutex and return without touching anything
            // - if it panics, we unlock mutex and propagate panic without touching anything
            // - if it calls `set` or `get_or_try_init` re-entrantly, we get a deadlock on
            //   mutex, which is important for safety. We *could* detect this and panic,
            //   but that is more complicated
            // - finally, if it returns Ok, we store the value and store the flag with
            //   `Release`, which synchronizes with `Acquire`s.
            let f = unsafe { take_unchecked(&mut f) };
            match f() {
                Ok(value) => unsafe {
                    // Safe b/c we have a unique access and no panic may happen
                    // until the cell is marked as initialized.
                    debug_assert!((*slot).is_none());
                    *slot = Some(value);
                    true
                },
                Err(err) => {
                    res = Err(err);
                    false
                }
            }
        });
        res
    }

    /// Get the reference to the underlying value, without checking if the cell
    /// is initialized.
    ///
    /// # Safety
    ///
    /// Caller must ensure that the cell is in initialized state, and that
    /// the contents are acquired by (synchronized to) this thread.
    pub(crate) unsafe fn get_unchecked(&self) -> &T {
        debug_assert!(self.is_initialized());
        let slot: &Option<T> = &*self.value.get();
        match slot {
            Some(value) => value,
            // This unsafe does improve performance, see `examples/bench`.
            None => {
                debug_assert!(false);
                hint::unreachable_unchecked()
            }
        }
    }

    /// Gets the mutable reference to the underlying value.
    /// Returns `None` if the cell is empty.
    pub(crate) fn get_mut(&mut self) -> Option<&mut T> {
        // Safe b/c we have an exclusive access
        let slot: &mut Option<T> = unsafe { &mut *self.value.get() };
        slot.as_mut()
    }

    /// Consumes this `OnceCell`, returning the wrapped value.
    /// Returns `None` if the cell was empty.
    pub(crate) fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }
}

// Note: this is intentionally monomorphic
#[inline(never)]
fn initialize_inner(mutex: &Mutex<()>, is_initialized: &AtomicBool, init: &mut dyn FnMut() -> bool) {
    let _guard = mutex.lock();

    if !is_initialized.load(Ordering::Acquire) {
        if init() {
            is_initialized.store(true, Ordering::Release);
        }
    }
}

#[test]
fn test_size() {
    use std::mem::size_of;

    assert_eq!(size_of::<OnceCell<bool>>(), 2 * size_of::<bool>() + size_of::<u8>());
}
