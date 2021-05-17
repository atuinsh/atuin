

use crate::array::Array;
use std::mem::MaybeUninit as StdMaybeUninit;

#[derive(Copy)]
pub struct MaybeUninit<T> {
    inner: StdMaybeUninit<T>,
}

impl<T> Clone for MaybeUninit<T>
    where T: Copy
{
    fn clone(&self) -> Self { *self }
}

impl<T> MaybeUninit<T> {
    /// Create a new MaybeUninit with uninitialized interior
    pub const unsafe fn uninitialized() -> Self {
        MaybeUninit { inner: StdMaybeUninit::uninit() }
    }

    /// Create a new MaybeUninit from the value `v`.
    pub fn from(v: T) -> Self {
        MaybeUninit { inner: StdMaybeUninit::new(v) }
    }

    // Raw pointer casts written so that we don't reference or access the
    // uninitialized interior value

    /// Return a raw pointer to the start of the interior array
    pub fn ptr(&self) -> *const T::Item
        where T: Array
    {
        self.inner.as_ptr() as *const T::Item
    }

    /// Return a mut raw pointer to the start of the interior array
    pub fn ptr_mut(&mut self) -> *mut T::Item
        where T: Array
    {
        self.inner.as_mut_ptr() as *mut T::Item
    }
}
