//! **arrayvec** provides the types `ArrayVec` and `ArrayString`: 
//! array-backed vector and string types, which store their contents inline.
//!
//! The arrayvec package has the following cargo features:
//!
//! - `std`
//!   - Optional, enabled by default
//!   - Use libstd; disable to use `no_std` instead.
//!
//! - `serde`
//!   - Optional
//!   - Enable serialization for ArrayVec and ArrayString using serde 1.x
//! - `array-sizes-33-128`, `array-sizes-129-255`
//!   - Optional
//!   - Enable more array sizes (see [Array] for more information)
//!
//! - `unstable-const-fn`
//!   - Optional
//!   - Makes [`ArrayVec::new`] and [`ArrayString::new`] `const fn`s,
//!     using the nightly `const_fn` feature.
//!   - Unstable and requires nightly.
//!
//! ## Rust Version
//!
//! This version of arrayvec requires Rust 1.36 or later.
//!
#![doc(html_root_url="https://docs.rs/arrayvec/0.4/")]
#![cfg_attr(not(feature="std"), no_std)]
#![cfg_attr(feature="unstable-const-fn", feature(const_fn))]

#[cfg(feature="serde")]
extern crate serde;

#[cfg(not(feature="std"))]
extern crate core as std;

use std::cmp;
use std::iter;
use std::mem;
use std::ops::{Bound, Deref, DerefMut, RangeBounds};
use std::ptr;
use std::slice;

// extra traits
use std::borrow::{Borrow, BorrowMut};
use std::hash::{Hash, Hasher};
use std::fmt;

#[cfg(feature="std")]
use std::io;


mod maybe_uninit;
use crate::maybe_uninit::MaybeUninit;

#[cfg(feature="serde")]
use serde::{Serialize, Deserialize, Serializer, Deserializer};

mod array;
mod array_string;
mod char;
mod errors;

pub use crate::array::Array;
use crate::array::Index;
pub use crate::array_string::ArrayString;
pub use crate::errors::CapacityError;


/// A vector with a fixed capacity.
///
/// The `ArrayVec` is a vector backed by a fixed size array. It keeps track of
/// the number of initialized elements.
///
/// The vector is a contiguous value that you can store directly on the stack
/// if needed.
///
/// It offers a simple API but also dereferences to a slice, so
/// that the full slice API is available.
///
/// ArrayVec can be converted into a by value iterator.
pub struct ArrayVec<A: Array> {
    xs: MaybeUninit<A>,
    len: A::Index,
}

impl<A: Array> Drop for ArrayVec<A> {
    fn drop(&mut self) {
        self.clear();

        // MaybeUninit inhibits array's drop
    }
}

macro_rules! panic_oob {
    ($method_name:expr, $index:expr, $len:expr) => {
        panic!(concat!("ArrayVec::", $method_name, ": index {} is out of bounds in vector of length {}"),
               $index, $len)
    }
}

impl<A: Array> ArrayVec<A> {
    /// Create a new empty `ArrayVec`.
    ///
    /// Capacity is inferred from the type parameter.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 16]>::new();
    /// array.push(1);
    /// array.push(2);
    /// assert_eq!(&array[..], &[1, 2]);
    /// assert_eq!(array.capacity(), 16);
    /// ```
    #[cfg(not(feature="unstable-const-fn"))]
    pub fn new() -> ArrayVec<A> {
        unsafe {
            ArrayVec { xs: MaybeUninit::uninitialized(), len: Index::ZERO }
        }
    }

    #[cfg(feature="unstable-const-fn")]
    pub const fn new() -> ArrayVec<A> {
        unsafe {
            ArrayVec { xs: MaybeUninit::uninitialized(), len: Index::ZERO }
        }
    }

    /// Return the number of elements in the `ArrayVec`.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3]);
    /// array.pop();
    /// assert_eq!(array.len(), 2);
    /// ```
    #[inline]
    pub fn len(&self) -> usize { self.len.to_usize() }

    /// Returns whether the `ArrayVec` is empty.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1]);
    /// array.pop();
    /// assert_eq!(array.is_empty(), true);
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Return the capacity of the `ArrayVec`.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let array = ArrayVec::from([1, 2, 3]);
    /// assert_eq!(array.capacity(), 3);
    /// ```
    #[inline(always)]
    pub fn capacity(&self) -> usize { A::CAPACITY }

    /// Return if the `ArrayVec` is completely filled.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 1]>::new();
    /// assert!(!array.is_full());
    /// array.push(1);
    /// assert!(array.is_full());
    /// ```
    pub fn is_full(&self) -> bool { self.len() == self.capacity() }

    /// Returns the capacity left in the `ArrayVec`.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3]);
    /// array.pop();
    /// assert_eq!(array.remaining_capacity(), 1);
    /// ```
    pub fn remaining_capacity(&self) -> usize {
        self.capacity() - self.len()
    }

    /// Push `element` to the end of the vector.
    ///
    /// ***Panics*** if the vector is already full.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 2]>::new();
    ///
    /// array.push(1);
    /// array.push(2);
    ///
    /// assert_eq!(&array[..], &[1, 2]);
    /// ```
    pub fn push(&mut self, element: A::Item) {
        self.try_push(element).unwrap()
    }

    /// Push `element` to the end of the vector.
    ///
    /// Return `Ok` if the push succeeds, or return an error if the vector
    /// is already full.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 2]>::new();
    ///
    /// let push1 = array.try_push(1);
    /// let push2 = array.try_push(2);
    ///
    /// assert!(push1.is_ok());
    /// assert!(push2.is_ok());
    ///
    /// assert_eq!(&array[..], &[1, 2]);
    ///
    /// let overflow = array.try_push(3);
    ///
    /// assert!(overflow.is_err());
    /// ```
    pub fn try_push(&mut self, element: A::Item) -> Result<(), CapacityError<A::Item>> {
        if self.len() < A::CAPACITY {
            unsafe {
                self.push_unchecked(element);
            }
            Ok(())
        } else {
            Err(CapacityError::new(element))
        }
    }


    /// Push `element` to the end of the vector without checking the capacity.
    ///
    /// It is up to the caller to ensure the capacity of the vector is
    /// sufficiently large.
    ///
    /// This method uses *debug assertions* to check that the arrayvec is not full.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 2]>::new();
    ///
    /// if array.len() + 2 <= array.capacity() {
    ///     unsafe {
    ///         array.push_unchecked(1);
    ///         array.push_unchecked(2);
    ///     }
    /// }
    ///
    /// assert_eq!(&array[..], &[1, 2]);
    /// ```
    pub unsafe fn push_unchecked(&mut self, element: A::Item) {
        let len = self.len();
        debug_assert!(len < A::CAPACITY);
        ptr::write(self.get_unchecked_ptr(len), element);
        self.set_len(len + 1);
    }

    /// Get pointer to where element at `index` would be
    unsafe fn get_unchecked_ptr(&mut self, index: usize) -> *mut A::Item {
        self.xs.ptr_mut().add(index)
    }

    /// Insert `element` at position `index`.
    ///
    /// Shift up all elements after `index`.
    ///
    /// It is an error if the index is greater than the length or if the
    /// arrayvec is full.
    ///
    /// ***Panics*** if the array is full or the `index` is out of bounds. See
    /// `try_insert` for fallible version.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 2]>::new();
    ///
    /// array.insert(0, "x");
    /// array.insert(0, "y");
    /// assert_eq!(&array[..], &["y", "x"]);
    ///
    /// ```
    pub fn insert(&mut self, index: usize, element: A::Item) {
        self.try_insert(index, element).unwrap()
    }

    /// Insert `element` at position `index`.
    ///
    /// Shift up all elements after `index`; the `index` must be less than
    /// or equal to the length.
    ///
    /// Returns an error if vector is already at full capacity.
    ///
    /// ***Panics*** `index` is out of bounds.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 2]>::new();
    ///
    /// assert!(array.try_insert(0, "x").is_ok());
    /// assert!(array.try_insert(0, "y").is_ok());
    /// assert!(array.try_insert(0, "z").is_err());
    /// assert_eq!(&array[..], &["y", "x"]);
    ///
    /// ```
    pub fn try_insert(&mut self, index: usize, element: A::Item) -> Result<(), CapacityError<A::Item>> {
        if index > self.len() {
            panic_oob!("try_insert", index, self.len())
        }
        if self.len() == self.capacity() {
            return Err(CapacityError::new(element));
        }
        let len = self.len();

        // follows is just like Vec<T>
        unsafe { // infallible
            // The spot to put the new value
            {
                let p: *mut _ = self.get_unchecked_ptr(index);
                // Shift everything over to make space. (Duplicating the
                // `index`th element into two consecutive places.)
                ptr::copy(p, p.offset(1), len - index);
                // Write it in, overwriting the first copy of the `index`th
                // element.
                ptr::write(p, element);
            }
            self.set_len(len + 1);
        }
        Ok(())
    }

    /// Remove the last element in the vector and return it.
    ///
    /// Return `Some(` *element* `)` if the vector is non-empty, else `None`.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::<[_; 2]>::new();
    ///
    /// array.push(1);
    ///
    /// assert_eq!(array.pop(), Some(1));
    /// assert_eq!(array.pop(), None);
    /// ```
    pub fn pop(&mut self) -> Option<A::Item> {
        if self.len() == 0 {
            return None;
        }
        unsafe {
            let new_len = self.len() - 1;
            self.set_len(new_len);
            Some(ptr::read(self.get_unchecked_ptr(new_len)))
        }
    }

    /// Remove the element at `index` and swap the last element into its place.
    ///
    /// This operation is O(1).
    ///
    /// Return the *element* if the index is in bounds, else panic.
    ///
    /// ***Panics*** if the `index` is out of bounds.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3]);
    ///
    /// assert_eq!(array.swap_remove(0), 1);
    /// assert_eq!(&array[..], &[3, 2]);
    ///
    /// assert_eq!(array.swap_remove(1), 2);
    /// assert_eq!(&array[..], &[3]);
    /// ```
    pub fn swap_remove(&mut self, index: usize) -> A::Item {
        self.swap_pop(index)
            .unwrap_or_else(|| {
                panic_oob!("swap_remove", index, self.len())
            })
    }

    /// Remove the element at `index` and swap the last element into its place.
    ///
    /// This is a checked version of `.swap_remove`.  
    /// This operation is O(1).
    ///
    /// Return `Some(` *element* `)` if the index is in bounds, else `None`.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3]);
    ///
    /// assert_eq!(array.swap_pop(0), Some(1));
    /// assert_eq!(&array[..], &[3, 2]);
    ///
    /// assert_eq!(array.swap_pop(10), None);
    /// ```
    pub fn swap_pop(&mut self, index: usize) -> Option<A::Item> {
        let len = self.len();
        if index >= len {
            return None;
        }
        self.swap(index, len - 1);
        self.pop()
    }

    /// Remove the element at `index` and shift down the following elements.
    ///
    /// The `index` must be strictly less than the length of the vector.
    ///
    /// ***Panics*** if the `index` is out of bounds.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3]);
    ///
    /// let removed_elt = array.remove(0);
    /// assert_eq!(removed_elt, 1);
    /// assert_eq!(&array[..], &[2, 3]);
    /// ```
    pub fn remove(&mut self, index: usize) -> A::Item {
        self.pop_at(index)
            .unwrap_or_else(|| {
                panic_oob!("remove", index, self.len())
            })
    }

    /// Remove the element at `index` and shift down the following elements.
    ///
    /// This is a checked version of `.remove(index)`. Returns `None` if there
    /// is no element at `index`. Otherwise, return the element inside `Some`.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3]);
    ///
    /// assert!(array.pop_at(0).is_some());
    /// assert_eq!(&array[..], &[2, 3]);
    ///
    /// assert!(array.pop_at(2).is_none());
    /// assert!(array.pop_at(10).is_none());
    /// ```
    pub fn pop_at(&mut self, index: usize) -> Option<A::Item> {
        if index >= self.len() {
            None
        } else {
            self.drain(index..index + 1).next()
        }
    }

    /// Shortens the vector, keeping the first `len` elements and dropping
    /// the rest.
    ///
    /// If `len` is greater than the vector’s current length this has no
    /// effect.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3, 4, 5]);
    /// array.truncate(3);
    /// assert_eq!(&array[..], &[1, 2, 3]);
    /// array.truncate(4);
    /// assert_eq!(&array[..], &[1, 2, 3]);
    /// ```
    pub fn truncate(&mut self, new_len: usize) {
        unsafe {
            if new_len < self.len() {
                let tail: *mut [_] = &mut self[new_len..];
                self.len = Index::from(new_len);
                ptr::drop_in_place(tail);
            }
        }
    }

    /// Remove all elements in the vector.
    pub fn clear(&mut self) {
        self.truncate(0)
    }

    /// Retains only the elements specified by the predicate.
    ///
    /// In other words, remove all elements `e` such that `f(&mut e)` returns false.
    /// This method operates in place and preserves the order of the retained
    /// elements.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut array = ArrayVec::from([1, 2, 3, 4]);
    /// array.retain(|x| *x & 1 != 0 );
    /// assert_eq!(&array[..], &[1, 3]);
    /// ```
    pub fn retain<F>(&mut self, mut f: F)
        where F: FnMut(&mut A::Item) -> bool
    {
        let len = self.len();
        let mut del = 0;
        {
            let v = &mut **self;

            for i in 0..len {
                if !f(&mut v[i]) {
                    del += 1;
                } else if del > 0 {
                    v.swap(i - del, i);
                }
            }
        }
        if del > 0 {
            self.drain(len - del..);
        }
    }

    /// Set the vector’s length without dropping or moving out elements
    ///
    /// This method is `unsafe` because it changes the notion of the
    /// number of “valid” elements in the vector. Use with care.
    ///
    /// This method uses *debug assertions* to check that `length` is
    /// not greater than the capacity.
    pub unsafe fn set_len(&mut self, length: usize) {
        debug_assert!(length <= self.capacity());
        self.len = Index::from(length);
    }

    /// Copy and appends all elements in a slice to the `ArrayVec`.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut vec: ArrayVec<[usize; 10]> = ArrayVec::new();
    /// vec.push(1);
    /// vec.try_extend_from_slice(&[2, 3]).unwrap();
    /// assert_eq!(&vec[..], &[1, 2, 3]);
    /// ```
    ///
    /// # Errors
    ///
    /// This method will return an error if the capacity left (see
    /// [`remaining_capacity`]) is smaller then the length of the provided
    /// slice.
    ///
    /// [`remaining_capacity`]: #method.remaining_capacity
    pub fn try_extend_from_slice(&mut self, other: &[A::Item]) -> Result<(), CapacityError>
        where A::Item: Copy,
    {
        if self.remaining_capacity() < other.len() {
            return Err(CapacityError::new(()));
        }

        let self_len = self.len();
        let other_len = other.len();

        unsafe {
            let dst = self.xs.ptr_mut().add(self_len);
            ptr::copy_nonoverlapping(other.as_ptr(), dst, other_len);
            self.set_len(self_len + other_len);
        }
        Ok(())
    }

    /// Create a draining iterator that removes the specified range in the vector
    /// and yields the removed items from start to end. The element range is
    /// removed even if the iterator is not consumed until the end.
    ///
    /// Note: It is unspecified how many elements are removed from the vector,
    /// if the `Drain` value is leaked.
    ///
    /// **Panics** if the starting point is greater than the end point or if
    /// the end point is greater than the length of the vector.
    ///
    /// ```
    /// use arrayvec::ArrayVec;
    ///
    /// let mut v = ArrayVec::from([1, 2, 3]);
    /// let u: ArrayVec<[_; 3]> = v.drain(0..2).collect();
    /// assert_eq!(&v[..], &[3]);
    /// assert_eq!(&u[..], &[1, 2]);
    /// ```
    pub fn drain<R>(&mut self, range: R) -> Drain<A>
        where R: RangeBounds<usize>
    {
        // Memory safety
        //
        // When the Drain is first created, it shortens the length of
        // the source vector to make sure no uninitialized or moved-from elements
        // are accessible at all if the Drain's destructor never gets to run.
        //
        // Drain will ptr::read out the values to remove.
        // When finished, remaining tail of the vec is copied back to cover
        // the hole, and the vector length is restored to the new length.
        //
        let len = self.len();
        let start = match range.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i.saturating_add(1),
        };
        let end = match range.end_bound() {
            Bound::Excluded(&j) => j,
            Bound::Included(&j) => j.saturating_add(1),
            Bound::Unbounded => len,
        };
        self.drain_range(start, end)
    }

    fn drain_range(&mut self, start: usize, end: usize) -> Drain<A>
    {
        let len = self.len();

        // bounds check happens here (before length is changed!)
        let range_slice: *const _ = &self[start..end];

        // Calling `set_len` creates a fresh and thus unique mutable references, making all
        // older aliases we created invalid. So we cannot call that function.
        self.len = Index::from(start);

        unsafe {
            Drain {
                tail_start: end,
                tail_len: len - end,
                iter: (*range_slice).iter(),
                vec: self as *mut _,
            }
        }
    }

    /// Return the inner fixed size array, if it is full to its capacity.
    ///
    /// Return an `Ok` value with the array if length equals capacity,
    /// return an `Err` with self otherwise.
    pub fn into_inner(self) -> Result<A, Self> {
        if self.len() < self.capacity() {
            Err(self)
        } else {
            unsafe {
                let array = ptr::read(self.xs.ptr() as *const A);
                mem::forget(self);
                Ok(array)
            }
        }
    }

    /// Dispose of `self` (same as drop)
    #[deprecated="Use std::mem::drop instead, if at all needed."]
    pub fn dispose(mut self) {
        self.clear();
        mem::forget(self);
    }

    /// Return a slice containing all elements of the vector.
    pub fn as_slice(&self) -> &[A::Item] {
        self
    }

    /// Return a mutable slice containing all elements of the vector.
    pub fn as_mut_slice(&mut self) -> &mut [A::Item] {
        self
    }

    /// Return a raw pointer to the vector's buffer.
    pub fn as_ptr(&self) -> *const A::Item {
        self.xs.ptr()
    }

    /// Return a raw mutable pointer to the vector's buffer.
    pub fn as_mut_ptr(&mut self) -> *mut A::Item {
        self.xs.ptr_mut()
    }
}

impl<A: Array> Deref for ArrayVec<A> {
    type Target = [A::Item];
    #[inline]
    fn deref(&self) -> &[A::Item] {
        unsafe {
            slice::from_raw_parts(self.xs.ptr(), self.len())
        }
    }
}

impl<A: Array> DerefMut for ArrayVec<A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [A::Item] {
        let len = self.len();
        unsafe {
            slice::from_raw_parts_mut(self.xs.ptr_mut(), len)
        }
    }
}

/// Create an `ArrayVec` from an array.
///
/// ```
/// use arrayvec::ArrayVec;
///
/// let mut array = ArrayVec::from([1, 2, 3]);
/// assert_eq!(array.len(), 3);
/// assert_eq!(array.capacity(), 3);
/// ```
impl<A: Array> From<A> for ArrayVec<A> {
    fn from(array: A) -> Self {
        ArrayVec { xs: MaybeUninit::from(array), len: Index::from(A::CAPACITY) }
    }
}


/// Try to create an `ArrayVec` from a slice. This will return an error if the slice was too big to
/// fit.
///
/// ```
/// use arrayvec::ArrayVec;
/// use std::convert::TryInto as _;
///
/// let array: ArrayVec<[_; 4]> = (&[1, 2, 3] as &[_]).try_into().unwrap();
/// assert_eq!(array.len(), 3);
/// assert_eq!(array.capacity(), 4);
/// ```
impl<A: Array> std::convert::TryFrom<&[A::Item]> for ArrayVec<A>
    where
        A::Item: Clone,
{
    type Error = CapacityError;

    fn try_from(slice: &[A::Item]) -> Result<Self, Self::Error> {
        if A::CAPACITY < slice.len() {
            Err(CapacityError::new(()))
        } else {
            let mut array = Self::new();
            array.extend(slice.iter().cloned());
            Ok(array)
        }
    }
}


/// Iterate the `ArrayVec` with references to each element.
///
/// ```
/// use arrayvec::ArrayVec;
///
/// let array = ArrayVec::from([1, 2, 3]);
///
/// for elt in &array {
///     // ...
/// }
/// ```
impl<'a, A: Array> IntoIterator for &'a ArrayVec<A> {
    type Item = &'a A::Item;
    type IntoIter = slice::Iter<'a, A::Item>;
    fn into_iter(self) -> Self::IntoIter { self.iter() }
}

/// Iterate the `ArrayVec` with mutable references to each element.
///
/// ```
/// use arrayvec::ArrayVec;
///
/// let mut array = ArrayVec::from([1, 2, 3]);
///
/// for elt in &mut array {
///     // ...
/// }
/// ```
impl<'a, A: Array> IntoIterator for &'a mut ArrayVec<A> {
    type Item = &'a mut A::Item;
    type IntoIter = slice::IterMut<'a, A::Item>;
    fn into_iter(self) -> Self::IntoIter { self.iter_mut() }
}

/// Iterate the `ArrayVec` with each element by value.
///
/// The vector is consumed by this operation.
///
/// ```
/// use arrayvec::ArrayVec;
///
/// for elt in ArrayVec::from([1, 2, 3]) {
///     // ...
/// }
/// ```
impl<A: Array> IntoIterator for ArrayVec<A> {
    type Item = A::Item;
    type IntoIter = IntoIter<A>;
    fn into_iter(self) -> IntoIter<A> {
        IntoIter { index: Index::from(0), v: self, }
    }
}


/// By-value iterator for `ArrayVec`.
pub struct IntoIter<A: Array> {
    index: A::Index,
    v: ArrayVec<A>,
}

impl<A: Array> Iterator for IntoIter<A> {
    type Item = A::Item;

    fn next(&mut self) -> Option<A::Item> {
        if self.index == self.v.len {
            None
        } else {
            unsafe {
                let index = self.index.to_usize();
                self.index = Index::from(index + 1);
                Some(ptr::read(self.v.get_unchecked_ptr(index)))
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.v.len() - self.index.to_usize();
        (len, Some(len))
    }
}

impl<A: Array> DoubleEndedIterator for IntoIter<A> {
    fn next_back(&mut self) -> Option<A::Item> {
        if self.index == self.v.len {
            None
        } else {
            unsafe {
                let new_len = self.v.len() - 1;
                self.v.set_len(new_len);
                Some(ptr::read(self.v.get_unchecked_ptr(new_len)))
            }
        }
    }
}

impl<A: Array> ExactSizeIterator for IntoIter<A> { }

impl<A: Array> Drop for IntoIter<A> {
    fn drop(&mut self) {
        // panic safety: Set length to 0 before dropping elements.
        let index = self.index.to_usize();
        let len = self.v.len();
        unsafe {
            self.v.set_len(0);
            let elements = slice::from_raw_parts_mut(
                self.v.get_unchecked_ptr(index),
                len - index);
            ptr::drop_in_place(elements);
        }
    }
}

impl<A: Array> Clone for IntoIter<A>
where
    A::Item: Clone,
{
    fn clone(&self) -> IntoIter<A> {
        self.v[self.index.to_usize()..]
            .iter()
            .cloned()
            .collect::<ArrayVec<A>>()
            .into_iter()
    }
}

impl<A: Array> fmt::Debug for IntoIter<A>
where
    A::Item: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_list()
            .entries(&self.v[self.index.to_usize()..])
            .finish()
    }
}

/// A draining iterator for `ArrayVec`.
pub struct Drain<'a, A> 
    where A: Array,
          A::Item: 'a,
{
    /// Index of tail to preserve
    tail_start: usize,
    /// Length of tail
    tail_len: usize,
    /// Current remaining range to remove
    iter: slice::Iter<'a, A::Item>,
    vec: *mut ArrayVec<A>,
}

unsafe impl<'a, A: Array + Sync> Sync for Drain<'a, A> {}
unsafe impl<'a, A: Array + Send> Send for Drain<'a, A> {}

impl<'a, A: Array> Iterator for Drain<'a, A>
    where A::Item: 'a,
{
    type Item = A::Item;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|elt|
            unsafe {
                ptr::read(elt as *const _)
            }
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, A: Array> DoubleEndedIterator for Drain<'a, A>
    where A::Item: 'a,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back().map(|elt|
            unsafe {
                ptr::read(elt as *const _)
            }
        )
    }
}

impl<'a, A: Array> ExactSizeIterator for Drain<'a, A> where A::Item: 'a {}

impl<'a, A: Array> Drop for Drain<'a, A> 
    where A::Item: 'a
{
    fn drop(&mut self) {
        // len is currently 0 so panicking while dropping will not cause a double drop.

        // exhaust self first
        while let Some(_) = self.next() { }

        if self.tail_len > 0 {
            unsafe {
                let source_vec = &mut *self.vec;
                // memmove back untouched tail, update to new length
                let start = source_vec.len();
                let tail = self.tail_start;
                let src = source_vec.as_ptr().add(tail);
                let dst = source_vec.as_mut_ptr().add(start);
                ptr::copy(src, dst, self.tail_len);
                source_vec.set_len(start + self.tail_len);
            }
        }
    }
}

struct ScopeExitGuard<T, Data, F>
    where F: FnMut(&Data, &mut T)
{
    value: T,
    data: Data,
    f: F,
}

impl<T, Data, F> Drop for ScopeExitGuard<T, Data, F>
    where F: FnMut(&Data, &mut T)
{
    fn drop(&mut self) {
        (self.f)(&self.data, &mut self.value)
    }
}



/// Extend the `ArrayVec` with an iterator.
/// 
/// Does not extract more items than there is space for. No error
/// occurs if there are more iterator elements.
impl<A: Array> Extend<A::Item> for ArrayVec<A> {
    fn extend<T: IntoIterator<Item=A::Item>>(&mut self, iter: T) {
        let take = self.capacity() - self.len();
        unsafe {
            let len = self.len();
            let mut ptr = raw_ptr_add(self.as_mut_ptr(), len);
            let end_ptr = raw_ptr_add(ptr, take);
            // Keep the length in a separate variable, write it back on scope
            // exit. To help the compiler with alias analysis and stuff.
            // We update the length to handle panic in the iteration of the
            // user's iterator, without dropping any elements on the floor.
            let mut guard = ScopeExitGuard {
                value: &mut self.len,
                data: len,
                f: move |&len, self_len| {
                    **self_len = Index::from(len);
                }
            };
            let mut iter = iter.into_iter();
            loop {
                if ptr == end_ptr { break; }
                if let Some(elt) = iter.next() {
                    raw_ptr_write(ptr, elt);
                    ptr = raw_ptr_add(ptr, 1);
                    guard.data += 1;
                } else {
                    break;
                }
            }
        }
    }
}

/// Rawptr add but uses arithmetic distance for ZST
unsafe fn raw_ptr_add<T>(ptr: *mut T, offset: usize) -> *mut T {
    if mem::size_of::<T>() == 0 {
        // Special case for ZST
        (ptr as usize).wrapping_add(offset) as _
    } else {
        ptr.add(offset)
    }
}

unsafe fn raw_ptr_write<T>(ptr: *mut T, value: T) {
    if mem::size_of::<T>() == 0 {
        /* nothing */
    } else {
        ptr::write(ptr, value)
    }
}

/// Create an `ArrayVec` from an iterator.
/// 
/// Does not extract more items than there is space for. No error
/// occurs if there are more iterator elements.
impl<A: Array> iter::FromIterator<A::Item> for ArrayVec<A> {
    fn from_iter<T: IntoIterator<Item=A::Item>>(iter: T) -> Self {
        let mut array = ArrayVec::new();
        array.extend(iter);
        array
    }
}

impl<A: Array> Clone for ArrayVec<A>
    where A::Item: Clone
{
    fn clone(&self) -> Self {
        self.iter().cloned().collect()
    }

    fn clone_from(&mut self, rhs: &Self) {
        // recursive case for the common prefix
        let prefix = cmp::min(self.len(), rhs.len());
        self[..prefix].clone_from_slice(&rhs[..prefix]);

        if prefix < self.len() {
            // rhs was shorter
            for _ in 0..self.len() - prefix {
                self.pop();
            }
        } else {
            let rhs_elems = rhs[self.len()..].iter().cloned();
            self.extend(rhs_elems);
        }
    }
}

impl<A: Array> Hash for ArrayVec<A>
    where A::Item: Hash
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&**self, state)
    }
}

impl<A: Array> PartialEq for ArrayVec<A>
    where A::Item: PartialEq
{
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<A: Array> PartialEq<[A::Item]> for ArrayVec<A>
    where A::Item: PartialEq
{
    fn eq(&self, other: &[A::Item]) -> bool {
        **self == *other
    }
}

impl<A: Array> Eq for ArrayVec<A> where A::Item: Eq { }

impl<A: Array> Borrow<[A::Item]> for ArrayVec<A> {
    fn borrow(&self) -> &[A::Item] { self }
}

impl<A: Array> BorrowMut<[A::Item]> for ArrayVec<A> {
    fn borrow_mut(&mut self) -> &mut [A::Item] { self }
}

impl<A: Array> AsRef<[A::Item]> for ArrayVec<A> {
    fn as_ref(&self) -> &[A::Item] { self }
}

impl<A: Array> AsMut<[A::Item]> for ArrayVec<A> {
    fn as_mut(&mut self) -> &mut [A::Item] { self }
}

impl<A: Array> fmt::Debug for ArrayVec<A> where A::Item: fmt::Debug {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { (**self).fmt(f) }
}

impl<A: Array> Default for ArrayVec<A> {
    /// Return an empty array
    fn default() -> ArrayVec<A> {
        ArrayVec::new()
    }
}

impl<A: Array> PartialOrd for ArrayVec<A> where A::Item: PartialOrd {
    fn partial_cmp(&self, other: &ArrayVec<A>) -> Option<cmp::Ordering> {
        (**self).partial_cmp(other)
    }

    fn lt(&self, other: &Self) -> bool {
        (**self).lt(other)
    }

    fn le(&self, other: &Self) -> bool {
        (**self).le(other)
    }

    fn ge(&self, other: &Self) -> bool {
        (**self).ge(other)
    }

    fn gt(&self, other: &Self) -> bool {
        (**self).gt(other)
    }
}

impl<A: Array> Ord for ArrayVec<A> where A::Item: Ord {
    fn cmp(&self, other: &ArrayVec<A>) -> cmp::Ordering {
        (**self).cmp(other)
    }
}

#[cfg(feature="std")]
/// `Write` appends written data to the end of the vector.
///
/// Requires `features="std"`.
impl<A: Array<Item=u8>> io::Write for ArrayVec<A> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let len = cmp::min(self.remaining_capacity(), data.len());
        let _result = self.try_extend_from_slice(&data[..len]);
        debug_assert!(_result.is_ok());
        Ok(len)
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

#[cfg(feature="serde")]
/// Requires crate feature `"serde"`
impl<T: Serialize, A: Array<Item=T>> Serialize for ArrayVec<A> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer
    {
        serializer.collect_seq(self)
    }
}

#[cfg(feature="serde")]
/// Requires crate feature `"serde"`
impl<'de, T: Deserialize<'de>, A: Array<Item=T>> Deserialize<'de> for ArrayVec<A> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where D: Deserializer<'de>
    {
        use serde::de::{Visitor, SeqAccess, Error};
        use std::marker::PhantomData;

        struct ArrayVecVisitor<'de, T: Deserialize<'de>, A: Array<Item=T>>(PhantomData<(&'de (), T, A)>);

        impl<'de, T: Deserialize<'de>, A: Array<Item=T>> Visitor<'de> for ArrayVecVisitor<'de, T, A> {
            type Value = ArrayVec<A>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "an array with no more than {} items", A::CAPACITY)
            }

            fn visit_seq<SA>(self, mut seq: SA) -> Result<Self::Value, SA::Error>
                where SA: SeqAccess<'de>,
            {
                let mut values = ArrayVec::<A>::new();

                while let Some(value) = seq.next_element()? {
                    if let Err(_) = values.try_push(value) {
                        return Err(SA::Error::invalid_length(A::CAPACITY + 1, &self));
                    }
                }

                Ok(values)
            }
        }

        deserializer.deserialize_seq(ArrayVecVisitor::<T, A>(PhantomData))
    }
}
