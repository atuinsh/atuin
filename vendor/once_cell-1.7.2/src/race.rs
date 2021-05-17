//! "First one wins" flavor of `OnceCell`.
//!
//! If two threads race to initialize a type from the `race` module, they
//! don't block, execute initialization function together, but only one of
//! them stores the result.
//!
//! This module does not require `std` feature.

use core::{
    num::NonZeroUsize,
    sync::atomic::{AtomicUsize, Ordering},
};

/// A thread-safe cell which can be written to only once.
#[derive(Default, Debug)]
pub struct OnceNonZeroUsize {
    inner: AtomicUsize,
}

impl OnceNonZeroUsize {
    /// Creates a new empty cell.
    #[inline]
    pub const fn new() -> OnceNonZeroUsize {
        OnceNonZeroUsize { inner: AtomicUsize::new(0) }
    }

    /// Gets the underlying value.
    #[inline]
    pub fn get(&self) -> Option<NonZeroUsize> {
        let val = self.inner.load(Ordering::Acquire);
        NonZeroUsize::new(val)
    }

    /// Sets the contents of this cell to `value`.
    ///
    /// Returns `Ok(())` if the cell was empty and `Err(())` if it was
    /// full.
    #[inline]
    pub fn set(&self, value: NonZeroUsize) -> Result<(), ()> {
        let exchange =
            self.inner.compare_exchange(0, value.get(), Ordering::AcqRel, Ordering::Acquire);
        match exchange {
            Ok(_) => Ok(()),
            Err(_) => Err(()),
        }
    }

    /// Gets the contents of the cell, initializing it with `f` if the cell was
    /// empty.
    ///
    /// If several threads concurrently run `get_or_init`, more than one `f` can
    /// be called. However, all threads will return the same value, produced by
    /// some `f`.
    pub fn get_or_init<F>(&self, f: F) -> NonZeroUsize
    where
        F: FnOnce() -> NonZeroUsize,
    {
        enum Void {}
        match self.get_or_try_init(|| Ok::<NonZeroUsize, Void>(f())) {
            Ok(val) => val,
            Err(void) => match void {},
        }
    }

    /// Gets the contents of the cell, initializing it with `f` if
    /// the cell was empty. If the cell was empty and `f` failed, an
    /// error is returned.
    ///
    /// If several threads concurrently run `get_or_init`, more than one `f` can
    /// be called. However, all threads will return the same value, produced by
    /// some `f`.
    pub fn get_or_try_init<F, E>(&self, f: F) -> Result<NonZeroUsize, E>
    where
        F: FnOnce() -> Result<NonZeroUsize, E>,
    {
        let val = self.inner.load(Ordering::Acquire);
        let res = match NonZeroUsize::new(val) {
            Some(it) => it,
            None => {
                let mut val = f()?.get();
                let exchange =
                    self.inner.compare_exchange(0, val, Ordering::AcqRel, Ordering::Acquire);
                if let Err(old) = exchange {
                    val = old;
                }
                unsafe { NonZeroUsize::new_unchecked(val) }
            }
        };
        Ok(res)
    }
}

/// A thread-safe cell which can be written to only once.
#[derive(Default, Debug)]
pub struct OnceBool {
    inner: OnceNonZeroUsize,
}

impl OnceBool {
    /// Creates a new empty cell.
    #[inline]
    pub const fn new() -> OnceBool {
        OnceBool { inner: OnceNonZeroUsize::new() }
    }

    /// Gets the underlying value.
    #[inline]
    pub fn get(&self) -> Option<bool> {
        self.inner.get().map(OnceBool::from_usize)
    }

    /// Sets the contents of this cell to `value`.
    ///
    /// Returns `Ok(())` if the cell was empty and `Err(())` if it was
    /// full.
    #[inline]
    pub fn set(&self, value: bool) -> Result<(), ()> {
        self.inner.set(OnceBool::to_usize(value))
    }

    /// Gets the contents of the cell, initializing it with `f` if the cell was
    /// empty.
    ///
    /// If several threads concurrently run `get_or_init`, more than one `f` can
    /// be called. However, all threads will return the same value, produced by
    /// some `f`.
    pub fn get_or_init<F>(&self, f: F) -> bool
    where
        F: FnOnce() -> bool,
    {
        OnceBool::from_usize(self.inner.get_or_init(|| OnceBool::to_usize(f())))
    }

    /// Gets the contents of the cell, initializing it with `f` if
    /// the cell was empty. If the cell was empty and `f` failed, an
    /// error is returned.
    ///
    /// If several threads concurrently run `get_or_init`, more than one `f` can
    /// be called. However, all threads will return the same value, produced by
    /// some `f`.
    pub fn get_or_try_init<F, E>(&self, f: F) -> Result<bool, E>
    where
        F: FnOnce() -> Result<bool, E>,
    {
        self.inner.get_or_try_init(|| f().map(OnceBool::to_usize)).map(OnceBool::from_usize)
    }

    #[inline]
    fn from_usize(value: NonZeroUsize) -> bool {
        value.get() == 1
    }
    #[inline]
    fn to_usize(value: bool) -> NonZeroUsize {
        unsafe { NonZeroUsize::new_unchecked(if value { 1 } else { 2 }) }
    }
}

#[cfg(feature = "alloc")]
pub use self::once_box::OnceBox;

#[cfg(feature = "alloc")]
mod once_box {
    use core::{
        marker::PhantomData,
        ptr,
        sync::atomic::{AtomicPtr, Ordering},
    };

    use alloc::boxed::Box;

    /// A thread-safe cell which can be written to only once.
    #[derive(Debug)]
    pub struct OnceBox<T> {
        inner: AtomicPtr<T>,
        ghost: PhantomData<Option<Box<T>>>,
    }

    impl<T> Default for OnceBox<T> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<T> Drop for OnceBox<T> {
        fn drop(&mut self) {
            let ptr = *self.inner.get_mut();
            if !ptr.is_null() {
                drop(unsafe { Box::from_raw(ptr) })
            }
        }
    }

    impl<T> OnceBox<T> {
        /// Creates a new empty cell.
        pub const fn new() -> OnceBox<T> {
            OnceBox { inner: AtomicPtr::new(ptr::null_mut()), ghost: PhantomData }
        }

        /// Gets a reference to the underlying value.
        pub fn get(&self) -> Option<&T> {
            let ptr = self.inner.load(Ordering::Acquire);
            if ptr.is_null() {
                return None;
            }
            Some(unsafe { &*ptr })
        }

        /// Sets the contents of this cell to `value`.
        ///
        /// Returns `Ok(())` if the cell was empty and `Err(value)` if it was
        /// full.
        pub fn set(&self, value: Box<T>) -> Result<(), Box<T>> {
            let ptr = Box::into_raw(value);
            let exchange = self.inner.compare_exchange(
                ptr::null_mut(),
                ptr,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            if let Err(_) = exchange {
                let value = unsafe { Box::from_raw(ptr) };
                return Err(value);
            }
            Ok(())
        }

        /// Gets the contents of the cell, initializing it with `f` if the cell was
        /// empty.
        ///
        /// If several threads concurrently run `get_or_init`, more than one `f` can
        /// be called. However, all threads will return the same value, produced by
        /// some `f`.
        pub fn get_or_init<F>(&self, f: F) -> &T
        where
            F: FnOnce() -> Box<T>,
        {
            enum Void {}
            match self.get_or_try_init(|| Ok::<Box<T>, Void>(f())) {
                Ok(val) => val,
                Err(void) => match void {},
            }
        }

        /// Gets the contents of the cell, initializing it with `f` if
        /// the cell was empty. If the cell was empty and `f` failed, an
        /// error is returned.
        ///
        /// If several threads concurrently run `get_or_init`, more than one `f` can
        /// be called. However, all threads will return the same value, produced by
        /// some `f`.
        pub fn get_or_try_init<F, E>(&self, f: F) -> Result<&T, E>
        where
            F: FnOnce() -> Result<Box<T>, E>,
        {
            let mut ptr = self.inner.load(Ordering::Acquire);

            if ptr.is_null() {
                let val = f()?;
                ptr = Box::into_raw(val);
                let exchange = self.inner.compare_exchange(
                    ptr::null_mut(),
                    ptr,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                if let Err(old) = exchange {
                    drop(unsafe { Box::from_raw(ptr) });
                    ptr = old;
                }
            };
            Ok(unsafe { &*ptr })
        }
    }

    unsafe impl<T: Sync + Send> Sync for OnceBox<T> {}

    /// ```compile_fail
    /// struct S(*mut ());
    /// unsafe impl Sync for S {}
    ///
    /// fn share<T: Sync>(_: &T) {}
    /// share(&once_cell::race::OnceBox::<S>::new());
    /// ```
    fn _dummy() {}
}
