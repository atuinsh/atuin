//! Helper traits for sequences.

#![allow(dead_code)]

use crate::lib::{cmp, iter, marker, ops, ptr, slice};
use arrayvec;

#[cfg(all(feature = "correct", feature = "radix"))]
use crate::lib::Vec;

// ARRVEC

/// Macro to automate simplify the creation of an ArrayVec.
#[macro_export]
macro_rules! arrvec {
    // This only works if the ArrayVec is the same size as the input array.
    ($elem:expr; $n:expr) => ({
        $crate::arrayvec::ArrayVec::from([$elem; $n])
    });
    // This just repeatedly calls `push`. I don't believe there's a concise way to count the number of expressions.
    ($($x:expr),*$(,)*) => ({
        // Allow an unused mut variable, since if the sequence is empty,
        // the vec will never be mutated.
        #[allow(unused_mut)] {
            let mut vec = $crate::arrayvec::ArrayVec::new();
            $(vec.push($x);)*
            vec
        }
    });
}

// INSERT MANY

/// Insert multiple elements at position `index`.
///
/// Shifts all elements before index to the back of the iterator.
/// It uses size hints to try to minimize the number of moves,
/// however, it does not rely on them. We cannot internally allocate, so
/// if we overstep the lower_size_bound, we have to do extensive
/// moves to shift each item back incrementally.
///
/// This implementation is adapted from [`smallvec`], which has a battle-tested
/// implementation that has been revised for at least a security advisory
/// warning. Smallvec is similarly licensed under an MIT/Apache dual license.
///
/// [`smallvec`]: https://github.com/servo/rust-smallvec
pub fn insert_many<V, T, I>(vec: &mut V, index: usize, iterable: I)
    where V: VecLike<T>,
          I: iter::IntoIterator<Item=T>
{
    let iter = iterable.into_iter();
    if index == vec.len() {
        return vec.extend(iter);
    }

    let (lower_size_bound, _) = iter.size_hint();
    assert!(lower_size_bound <= isize::max_value() as usize);   // Ensure offset is indexable
    assert!(index + lower_size_bound >= index);                 // Protect against overflow
    vec.reserve(lower_size_bound);

    unsafe {
        let old_len = vec.len();
        assert!(index <= old_len);
        let mut ptr = vec.as_mut_ptr().add(index);

        // Move the trailing elements.
        ptr::copy(ptr, ptr.add(lower_size_bound), old_len - index);

        // In case the iterator panics, don't double-drop the items we just copied above.
        vec.set_len(index);

        let mut num_added = 0;
        for element in iter {
            let mut cur = ptr.add(num_added);
            if num_added >= lower_size_bound {
                // Iterator provided more elements than the hint.  Move trailing items again.
                vec.reserve(1);
                ptr = vec.as_mut_ptr().add(index);
                cur = ptr.add(num_added);
                ptr::copy(cur, cur.add(1), old_len - index);
            }
            ptr::write(cur, element);
            num_added += 1;
        }
        if num_added < lower_size_bound {
            // Iterator provided fewer elements than the hint
            ptr::copy(ptr.add(lower_size_bound), ptr.add(num_added), old_len - index);
        }

        vec.set_len(old_len + num_added);
    }
}

// REMOVE_MANY

/// Remove many elements from a vec-like container.
///
/// Does not change the size of the vector, and may leak
/// if the destructor panics. **Must** call `set_len` after,
/// and ideally before (to 0).
fn remove_many<V, T, R>(vec: &mut V, range: R)
    where V: VecLike<T>,
          R: ops::RangeBounds<usize>
{
    // Get the bounds on the items we're removing.
    let len = vec.len();
    let start = match range.start_bound() {
        ops::Bound::Included(&n) => n,
        ops::Bound::Excluded(&n) => n + 1,
        ops::Bound::Unbounded    => 0,
    };
    let end = match range.end_bound() {
        ops::Bound::Included(&n) => n + 1,
        ops::Bound::Excluded(&n) => n,
        ops::Bound::Unbounded    => len,
    };
    assert!(start <= end);
    assert!(end <= len);

    // Drop the existing items.
    unsafe {
        // Set len temporarily to the start, in case we panic on a drop.
        // This means we leak memory, but we don't allow any double freeing,
        // or use after-free.
        vec.set_len(start);
        // Iteratively drop the range.
        let mut first = vec.as_mut_ptr().add(start);
        let last = vec.as_mut_ptr().add(end);
        while first < last {
            ptr::drop_in_place(first);
            first = first.add(1);
        }

        // Now we need to copy the end range into the buffer.
        let count = len - end;
        if count != 0 {
            let src = vec.as_ptr().add(end);
            let dst = vec.as_mut_ptr().add(start);
            ptr::copy(src, dst, count);
        }

        // Set the proper length, now that we've moved items in.
        vec.set_len(start + count);
    }
}

// HELPERS
// -------

// RSLICE INDEX

/// A trait for reversed-indexing operations.
pub trait RSliceIndex<T: ?Sized> {
    /// Output type for the index.
    type Output: ?Sized;

    /// Get reference to element or subslice.
    fn rget(self, slc: &T) -> Option<&Self::Output>;

    /// Get mutable reference to element or subslice.
    fn rget_mut(self, slc: &mut T) -> Option<&mut Self::Output>;

    /// Get reference to element or subslice without bounds checking.
    unsafe fn rget_unchecked(self, slc: &T) -> &Self::Output;

    /// Get mutable reference to element or subslice without bounds checking.
    unsafe fn rget_unchecked_mut(self, slc: &mut T) -> &mut Self::Output;

    /// Get reference to element or subslice, panic if out-of-bounds.
    fn rindex(self, slc: &T) -> &Self::Output;

    /// Get mutable reference to element or subslice, panic if out-of-bounds.
    fn rindex_mut(self, slc: &mut T) -> &mut Self::Output;
}

impl<T> RSliceIndex<[T]> for usize {
    type Output = T;

    #[inline]
    fn rget(self, slc: &[T]) -> Option<&T> {
        let len = slc.len();
        slc.get(len - self - 1)
    }

    #[inline]
    fn rget_mut(self, slc: &mut [T]) -> Option<&mut T> {
        let len = slc.len();
        slc.get_mut(len - self - 1)
    }

    #[inline]
    unsafe fn rget_unchecked(self, slc: &[T]) -> &T {
        let len = slc.len();
        slc.get_unchecked(len - self - 1)
    }

    #[inline]
    unsafe fn rget_unchecked_mut(self, slc: &mut [T]) -> &mut T {
        let len = slc.len();
        slc.get_unchecked_mut(len - self - 1)
    }

    #[inline]
    fn rindex(self, slc: &[T]) -> &T {
        let len = slc.len();
        &(*slc)[len - self - 1]
    }

    #[inline]
    fn rindex_mut(self, slc: &mut [T]) -> &mut T {
        let len = slc.len();
        &mut (*slc)[len - self - 1]
    }
}

/// REVERSE VIEW

/// Reverse, immutable view of a sequence.
pub struct ReverseView<'a, T: 'a> {
    inner: &'a [T],
}

impl<'a, T> ops::Index<usize> for ReverseView<'a, T> {
    type Output = T;

    #[inline]
    fn index(&self, index: usize) -> &T {
        self.inner.rindex(index)
    }
}

/// REVERSE VIEW MUT

/// Reverse, mutable view of a sequence.
pub struct ReverseViewMut<'a, T: 'a> {
    inner: &'a mut [T],
}

impl<'a, T: 'a> ops::Index<usize> for ReverseViewMut<'a, T> {
    type Output = T;

    #[inline]
    fn index(&self, index: usize) -> &T {
        self.inner.rindex(index)
    }
}

impl<'a, T: 'a> ops::IndexMut<usize> for ReverseViewMut<'a, T> {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.inner.rindex_mut(index)
    }
}

// SLICELIKE

/// Implied base trait for slice-like types.
///
/// Used to provide specializations since it requires no generic function parameters.
pub trait SliceLikeImpl<T> {
    // AS SLICE

    /// Get slice of immutable elements.
    fn as_slice(&self) -> &[T];

    /// Get slice of mutable elements.
    fn as_mut_slice(&mut self) -> &mut [T];
}

impl<T> SliceLikeImpl<T> for [T] {
    // AS SLICE

    #[inline]
    fn as_slice(&self) -> &[T] {
        self
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [T] {
        self
    }
}

#[cfg(all(feature = "correct", feature = "radix"))]
impl<T> SliceLikeImpl<T> for Vec<T> {
    // AS SLICE

    #[inline]
    fn as_slice(&self) -> &[T] {
        Vec::as_slice(self)
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [T] {
        Vec::as_mut_slice(self)
    }
}

impl<A: arrayvec::Array> SliceLikeImpl<A::Item> for arrayvec::ArrayVec<A> {
    // AS SLICE

    #[inline]
    fn as_slice(&self) -> &[A::Item] {
        arrayvec::ArrayVec::as_slice(self)
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [A::Item] {
        arrayvec::ArrayVec::as_mut_slice(self)
    }
}

/// Collection that has a `contains()` method.
pub trait Contains<T: PartialEq> {
    /// Check if slice contains element.
    fn contains(&self, x: &T) -> bool;
}

impl<T: PartialEq> Contains<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn contains(&self, x: &T) -> bool {
        <[T]>::contains(self.as_slice(), x)
    }
}

/// Collection that has a `starts_with()` method.
pub trait StartsWith<T: PartialEq> {
    /// Check if slice starts_with subslice.
    fn starts_with(&self, x: &[T]) -> bool;
}

impl<T: PartialEq> StartsWith<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn starts_with(&self, x: &[T]) -> bool {
        <[T]>::starts_with(self.as_slice(), x)
    }
}

/// Collection that has a `ends_with()` method.
pub trait EndsWith<T: PartialEq> {
    /// Check if slice ends_with subslice.
    fn ends_with(&self, x: &[T]) -> bool;
}

impl<T: PartialEq> EndsWith<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn ends_with(&self, x: &[T]) -> bool {
        <[T]>::ends_with(self.as_slice(), x)
    }
}

/// Collection that has a `binary_search()` method.
pub trait BinarySearch<T: Ord> {
    /// Perform binary search for value.
    fn binary_search(&self, x: &T) -> Result<usize, usize>;
}

impl<T: Ord> BinarySearch<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn binary_search(&self, x: &T) -> Result<usize, usize> {
        <[T]>::binary_search(self.as_slice(), x)
    }
}

/// Collection that has a `sort()` method.
pub trait Sort<T: Ord> {
    /// Sort sequence.
    fn sort(&mut self);
}

impl<T: Ord> Sort<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn sort(&mut self) {
        <[T]>::sort(self.as_mut_slice())
    }
}

/// Collection that has a `sort_unstable()` method.
pub trait SortUnstable<T: Ord> {
    /// Sort sequence without preserving order of equal elements.
    fn sort_unstable(&mut self);
}

impl<T: Ord> SortUnstable<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn sort_unstable(&mut self) {
        <[T]>::sort_unstable(self.as_mut_slice())
    }
}

/// Collection that has a `clone_from_slice()` method.
pub trait CloneFromSlice<T: Clone> {
    /// Clone items from src into self.
    fn clone_from_slice(&mut self, src: &[T]);
}

impl<T: Clone> CloneFromSlice<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn clone_from_slice(&mut self, src: &[T]) {
        <[T]>::clone_from_slice(self.as_mut_slice(), src)
    }
}

/// Collection that has a `copy_from_slice()` method.
pub trait CopyFromSlice<T: Copy> {
    /// Copy items from src into self.
    fn copy_from_slice(&mut self, src: &[T]);
}

impl<T: Copy> CopyFromSlice<T> for dyn SliceLikeImpl<T> {
    #[inline]
    fn copy_from_slice(&mut self, src: &[T]) {
        <[T]>::copy_from_slice(self.as_mut_slice(), src)
    }
}

/// Slice-like container.
pub trait SliceLike<T>: SliceLikeImpl<T> {
    // CORE
    // ----

    // GET

    /// Get an immutable reference to item at index.
    fn get<I: slice::SliceIndex<[T]>>(&self, index: I) -> Option<&I::Output>;

    /// Get a mutable reference to item at index.
    fn get_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> Option<&mut I::Output>;

    /// Get an immutable reference to item at index.
    unsafe fn get_unchecked<I: slice::SliceIndex<[T]>>(&self, index: I) -> &I::Output;

    /// Get a mutable reference to item at index.
    unsafe fn get_unchecked_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output;

    // INDEX

    /// Get immutable element(s) via indexing.
    fn index<I: slice::SliceIndex<[T]>>(&self, index: I) -> &I::Output;

    /// Get mutable element(s) via indexing.
    fn index_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output;

    // RGET

    /// Get reference to element or subslice.
    fn rget<I: RSliceIndex<[T]>>(&self, index: I) -> Option<&I::Output>;

    /// Get mutable reference to element or subslice.
    fn rget_mut<I: RSliceIndex<[T]>>(&mut self, index: I) -> Option<&mut I::Output>;

    /// Get reference to element or subslice without bounds checking.
    unsafe fn rget_unchecked<I: RSliceIndex<[T]>>(&self, index: I) -> &I::Output;

    /// Get mutable reference to element or subslice without bounds checking.
    unsafe fn rget_unchecked_mut<I: RSliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output;

    // RINDEX

    /// Get reference to element or subslice.
    fn rindex<I: RSliceIndex<[T]>>(&self, index: I) -> &I::Output;

    /// Get mutable reference to element or subslice.
    fn rindex_mut<I: RSliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output;

    // DERIVATIVE
    // ----------

    // AS PTR

    /// Get pointer to start of contiguous collection.
    #[inline]
    fn as_ptr(&self) -> *const T {
        <[T]>::as_ptr(self.as_slice())
    }

    /// Get mutable pointer to start of contiguous collection.
    #[inline]
    fn as_mut_ptr(&mut self) -> *mut T {
        <[T]>::as_mut_ptr(self.as_mut_slice())
    }

    // BINARY SEARCH BY

    /// Perform binary search with a predicate.
    #[inline]
    fn binary_search_by<F>(&self, func: F)
        -> Result<usize, usize>
        where F: FnMut(&T) -> cmp::Ordering
    {
        <[T]>::binary_search_by(self.as_slice(), func)
    }

    /// Perform binary search by key with key extractor.
    #[inline]
    fn binary_search_by_key<K, F>(&self, key: &K, func: F)
        -> Result<usize, usize>
        where K: Ord,
              F: FnMut(&T) -> K
    {
        <[T]>::binary_search_by_key(self.as_slice(), key, func)
    }

    // CHUNKS

    /// Get iterator over `size`-length immutable elements in sequence.
    #[inline]
    fn chunks(&self, size: usize) -> slice::Chunks<T> {
        <[T]>::chunks(self.as_slice(), size)
    }

    /// Get iterator over `size`-length mutable elements in sequence.
    #[inline]
    fn chunks_mut(&mut self, size: usize) -> slice::ChunksMut<T> {
        <[T]>::chunks_mut(self.as_mut_slice(), size)
    }

    // CHUNKS EXACT
    // Currently unused, restore and add default implementation if required
    // later. Requires rustc >= 1.31.0.
//
//    /// Get iterator over exactly `size`-length immutable elements in sequence.
//    #[inline]
//    fn chunks_exact(&self, size: usize) -> slice::ChunksExact<T> {
//        <[T]>::chunks_exact(self.as_slice(), size)
//    }
//
//    /// Get iterator over exactly `size`-length mutable elements in sequence.
//    #[inline]
//    fn chunks_exact_mut(&mut self, size: usize) -> slice::ChunksExactMut<T> {
//        <[T]>::chunks_exact_mut(self.as_mut_slice(), size)
//    }

    // FIRST

    /// Get an immutable reference to the first item.
    #[inline]
    fn first(&self) -> Option<&T> {
        self.as_slice().get(0)
    }

    /// Get a mutable reference to the first item.
    #[inline]
    fn first_mut(&mut self) -> Option<&mut T> {
        self.as_mut_slice().get_mut(0)
    }

    /// Get an immutable reference to the first item without bounds checking.
    #[inline]
    unsafe fn first_unchecked(&self) -> &T {
        self.as_slice().get_unchecked(0)
    }

    /// Get a mutable reference to the first item without bounds checking.
    #[inline]
    unsafe fn first_unchecked_mut(&mut self) -> &mut T  {
        self.as_mut_slice().get_unchecked_mut(0)
    }

    // ITER

    /// Iterate over immutable elements in the collection.
    #[inline]
    fn iter(&self) -> slice::Iter<T> {
        <[T]>::iter(self.as_slice())
    }

    /// Iterate over mutable elements in the collection.
    #[inline]
    fn iter_mut(&mut self) -> slice::IterMut<T> {
        <[T]>::iter_mut(self.as_mut_slice())
    }

    // LAST

    /// Get an immutable reference to the last item.
    #[inline]
    fn last(&self) -> Option<&T> {
        self.rget(0)
    }

    /// Get a mutable reference to the last item.
    #[inline]
    fn last_mut(&mut self) -> Option<&mut T> {
        self.rget_mut(0)
    }

    /// Get an immutable reference to the last item without bounds checking.
    #[inline]
    unsafe fn last_unchecked(&self) -> &T {
        debug_assert!(self.len() > 0);
        self.rget_unchecked(0)
    }

    /// Get a mutable reference to the last item without bounds checking.
    #[inline]
    unsafe fn last_unchecked_mut(&mut self) -> &mut T  {
        debug_assert!(self.len() > 0);
        self.rget_unchecked_mut(0)
    }

    // LEN

    /// Get if the collection is empty.
    #[inline]
    fn is_empty(&self) -> bool {
        <[T]>::is_empty(self.as_slice())
    }

    /// Get the length of the collection.
    #[inline]
    fn len(&self) -> usize {
        <[T]>::len(self.as_slice())
    }

    // Currently unused, restore and add default implementation if required
    // later. Requires rustc >= 1.31.0.
//    // RCHUNKS
//
//    /// Get iterator over `size`-length immutable elements in sequence.
//    #[inline]
//    fn rchunks(&self, size: usize) -> slice::RChunks<T> {
//        <[T]>::rchunks(self.as_slice(), size)
//    }
//
//    /// Get iterator over `size`-length mutable elements in sequence.
//    #[inline]
//    fn rchunks_mut(&mut self, size: usize) -> slice::RChunksMut<T> {
//        <[T]>::rchunks_mut(self.as_mut_slice(), size)
//    }
//
//    // RCHUNKS EXACT
//
//    /// Get iterator over exactly `size`-length immutable elements in sequence.
//    #[inline]
//    fn rchunks_exact(&self, size: usize) -> slice::RChunksExact<T> {
//        <[T]>::rchunks_exact(self.as_slice(), size)
//    }
//
//    /// Get iterator over exactly `size`-length mutable elements in sequence.
//    #[inline]
//    fn rchunks_exact_mut(&mut self, size: usize) -> slice::RChunksExactMut<T> {
//        <[T]>::rchunks_exact_mut(self.as_mut_slice(), size)
//    }

    // REVERSE

    /// Reverse elements in collection.
    #[inline]
    fn reverse(&mut self) {
        <[T]>::reverse(self.as_mut_slice())
    }

    // ROTATE

    // Currently unused, restore and add default implementation if required
    // later. Requires rustc >= 1.26.0.
//    /// Rotate elements of slice left.
//    #[inline]
//    fn rotate_left(&mut self, mid: usize) {
//        <[T]>::rotate_left(self.as_mut_slice(), mid)
//    }
//
//    /// Rotate elements of slice right.
//    #[inline]
//    fn rotate_right(&mut self, mid: usize) {
//        <[T]>::rotate_right(self.as_mut_slice(), mid)
//    }

    // RSPLIT

    // Currently unused, restore and add default implementation if required
    // later. Requires rustc >= 1.27.0.
//    /// Split on condition into immutable subslices, start from the back of the slice.
//    #[inline]
//    fn rsplit<F: FnMut(&T) -> bool>(&self, func: F) -> slice::RSplit<T, F> {
//        <[T]>::rsplit(self.as_slice(), func)
//    }
//
//    /// Split on condition into mutable subslices, start from the back of the slice.
//    #[inline]
//    fn rsplit_mut<F: FnMut(&T) -> bool>(&mut self, func: F) -> slice::RSplitMut<T, F> {
//        <[T]>::rsplit_mut(self.as_mut_slice(), func)
//    }

    // RSPLITN

    /// `rsplit()` with n subslices.
    #[inline]
    fn rsplitn<F: FnMut(&T) -> bool>(&self, n: usize, func: F) -> slice::RSplitN<T, F> {
        <[T]>::rsplitn(self.as_slice(), n, func)
    }

    /// `rsplit_mut()` with n subslices.
    #[inline]
    fn rsplitn_mut<F: FnMut(&T) -> bool>(&mut self, n: usize, func: F) -> slice::RSplitNMut<T, F> {
        <[T]>::rsplitn_mut(self.as_mut_slice(), n, func)
    }

    // SORT BY

    /// Perform sort with a predicate.
    #[inline]
    fn sort_by<F>(&mut self, func: F)
        where F: FnMut(&T, &T) -> cmp::Ordering
    {
        <[T]>::sort_by(self.as_mut_slice(), func)
    }

    /// Perform sort by key with key extractor.
    #[inline]
    fn sort_by_key<K, F>(&mut self, func: F)
        where K: Ord,
              F: FnMut(&T) -> K
    {
        <[T]>::sort_by_key(self.as_mut_slice(), func)
    }

    // SORT UNSTABLE BY

    /// Perform untable sort with a predicate.
    #[inline]
    fn sort_unstable_by<F>(&mut self, func: F)
        where F: FnMut(&T, &T) -> cmp::Ordering
    {
        <[T]>::sort_unstable_by(self.as_mut_slice(), func)
    }

    /// Perform untable sort by key with key extractor.
    #[inline]
    fn sort_unstable_by_key<K, F>(&mut self, func: F)
        where K: Ord,
              F: FnMut(&T) -> K
    {
        <[T]>::sort_unstable_by_key(self.as_mut_slice(), func)
    }

    // SPLIT

    /// Split on condition into immutable subslices, start from the front of the slice.
    #[inline]
    fn split<F: FnMut(&T) -> bool>(&self, func: F) -> slice::Split<T, F> {
        <[T]>::split(self.as_slice(), func)
    }

    /// Split on condition into mutable subslices, start from the front of the slice.
    #[inline]
    fn split_mut<F: FnMut(&T) -> bool>(&mut self, func: F) -> slice::SplitMut<T, F> {
        <[T]>::split_mut(self.as_mut_slice(), func)
    }

    // SPLIT AT

    /// Split at index, return immutable values for the values before and after.
    #[inline]
    fn split_at(&self, index: usize) -> (&[T], &[T]) {
        <[T]>::split_at(self.as_slice(), index)
    }

    /// Split at index, return immutable values for the values before and after.
    #[inline]
    fn split_at_mut(&mut self, index: usize) -> (&mut [T], &mut [T]) {
        <[T]>::split_at_mut(self.as_mut_slice(), index)
    }

    // SPLIT FIRST

    /// Split at first item, returning values or None if empty.
    #[inline]
    fn split_first(&self) -> Option<(&T, &[T])> {
        <[T]>::split_first(self.as_slice())
    }

    /// Split at first item, returning values or None if empty.
    #[inline]
    fn split_first_mut(&mut self) -> Option<(&mut T, &mut [T])> {
        <[T]>::split_first_mut(self.as_mut_slice())
    }

    // SPLIT LAST

    /// Split at last item, returning values or None if empty.
    #[inline]
    fn split_last(&self) -> Option<(&T, &[T])> {
        <[T]>::split_last(self.as_slice())
    }

    /// Split at last item, returning values or None if empty.
    #[inline]
    fn split_last_mut(&mut self) -> Option<(&mut T, &mut [T])> {
        <[T]>::split_last_mut(self.as_mut_slice())
    }

    // SPLIT N

    /// `split()` with n subslices.
    #[inline]
    fn splitn<F: FnMut(&T) -> bool>(&self, n: usize, func: F) -> slice::SplitN<T, F> {
        <[T]>::splitn(self.as_slice(), n, func)
    }

    /// `split_mut()` with n subslices.
    #[inline]
    fn splitn_mut<F: FnMut(&T) -> bool>(&mut self, n: usize, func: F) -> slice::SplitNMut<T, F> {
        <[T]>::splitn_mut(self.as_mut_slice(), n, func)
    }

    // SWAP

    /// Swap two elements in the container by index.
    #[inline]
    fn swap(&mut self, x: usize, y: usize) {
        <[T]>::swap(self.as_mut_slice(), x, y)
    }

    /// Swap all elements in `self` with `other`.
    #[inline]
    fn swap_with_slice(&mut self, other: &mut [T]) {
        <[T]>::swap_with_slice(self.as_mut_slice(), other)
    }

    // WINDOWS

    /// View windows of `n`-length contiguous subslices.
    #[inline]
    fn windows(&self, size: usize) -> slice::Windows<T> {
        <[T]>::windows(self.as_slice(), size)
    }

    // RVIEW

    /// Create a reverse view of the vector for indexing.
    #[inline]
    fn rview<'a>(&'a self) -> ReverseView<'a, T> {
        ReverseView { inner: self.as_slice() }
    }

    /// Create a reverse, mutable view of the vector for indexing.
    #[inline]
    fn rview_mut<'a>(&'a mut self) -> ReverseViewMut<'a, T> {
        ReverseViewMut { inner: self.as_mut_slice() }
    }
}

impl<T> SliceLike<T> for [T] {
    // GET

    /// Get an immutable reference to item at index.
    #[inline]
    fn get<I: slice::SliceIndex<[T]>>(&self, index: I) -> Option<&I::Output> {
        return <[T]>::get(self, index);
    }

    /// Get an mutable reference to item at index.
    #[inline]
    fn get_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> Option<&mut I::Output> {
        return <[T]>::get_mut(self, index);
    }

    /// Get an immutable reference to item at index.
    #[inline]
    unsafe fn get_unchecked<I: slice::SliceIndex<[T]>>(&self, index: I) -> &I::Output {
        return <[T]>::get_unchecked(self, index);
    }

    /// Get an mutable reference to item at index.
    #[inline]
    unsafe fn get_unchecked_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        return <[T]>::get_unchecked_mut(self, index);
    }

    // INDEX

    #[inline]
    fn index<I: slice::SliceIndex<[T]>>(&self, index: I) -> &I::Output {
        return <[T] as ops::Index<I>>::index(self, index);
    }

    #[inline]
    fn index_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        return <[T] as ops::IndexMut<I>>::index_mut(self, index);
    }

    // RGET

    #[inline]
    fn rget<I: RSliceIndex<[T]>>(&self, index: I)
        -> Option<&I::Output>
    {
        index.rget(self)
    }

    #[inline]
    fn rget_mut<I: RSliceIndex<[T]>>(&mut self, index: I)
        -> Option<&mut I::Output>
    {
        index.rget_mut(self)
    }

    #[inline]
    unsafe fn rget_unchecked<I: RSliceIndex<[T]>>(&self, index: I)
        -> &I::Output
    {
        index.rget_unchecked(self)
    }

    #[inline]
    unsafe fn rget_unchecked_mut<I: RSliceIndex<[T]>>(&mut self, index: I)
        -> &mut I::Output
    {
        index.rget_unchecked_mut(self)
    }

    // RINDEX

    #[inline]
    fn rindex<I: RSliceIndex<[T]>>(&self, index: I) -> &I::Output {
        index.rindex(self)
    }

    #[inline]
    fn rindex_mut<I: RSliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        index.rindex_mut(self)
    }
}

#[cfg(all(feature = "correct", feature = "radix"))]
impl<T> SliceLike<T> for Vec<T> {
    // GET

    /// Get an immutable reference to item at index.
    #[inline]
    fn get<I: slice::SliceIndex<[T]>>(&self, index: I) -> Option<&I::Output> {
        return self.as_slice().get(index);
    }

    /// Get an mutable reference to item at index.
    #[inline]
    fn get_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> Option<&mut I::Output> {
        return self.as_mut_slice().get_mut(index);
    }

    /// Get an immutable reference to item at index.
    #[inline]
    unsafe fn get_unchecked<I: slice::SliceIndex<[T]>>(&self, index: I) -> &I::Output {
        return self.as_slice().get_unchecked(index);
    }

    /// Get an mutable reference to item at index.
    #[inline]
    unsafe fn get_unchecked_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        return self.as_mut_slice().get_unchecked_mut(index);
    }

    // INDEX

    #[inline]
    fn index<I: slice::SliceIndex<[T]>>(&self, index: I) -> &I::Output {
        return self.as_slice().index(index);
    }

    #[inline]
    fn index_mut<I: slice::SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        return self.as_mut_slice().index_mut(index);
    }

    // RGET

    #[inline]
    fn rget<I: RSliceIndex<[T]>>(&self, index: I)
        -> Option<&I::Output>
    {
        index.rget(self.as_slice())
    }

    #[inline]
    fn rget_mut<I: RSliceIndex<[T]>>(&mut self, index: I)
        -> Option<&mut I::Output>
    {
        index.rget_mut(self.as_mut_slice())
    }

    #[inline]
    unsafe fn rget_unchecked<I: RSliceIndex<[T]>>(&self, index: I)
        -> &I::Output
    {
        index.rget_unchecked(self.as_slice())
    }

    #[inline]
    unsafe fn rget_unchecked_mut<I: RSliceIndex<[T]>>(&mut self, index: I)
        -> &mut I::Output
    {
        index.rget_unchecked_mut(self.as_mut_slice())
    }

    // RINDEX

    #[inline]
    fn rindex<I: RSliceIndex<[T]>>(&self, index: I) -> &I::Output {
        index.rindex(self.as_slice())
    }

    #[inline]
    fn rindex_mut<I: RSliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        index.rindex_mut(self.as_mut_slice())
    }
}

impl<A: arrayvec::Array> SliceLike<A::Item> for arrayvec::ArrayVec<A> {
    // GET

    /// Get an immutable reference to item at index.
    #[inline]
    fn get<I: slice::SliceIndex<[A::Item]>>(&self, index: I) -> Option<&I::Output> {
        return self.as_slice().get(index);
    }

    /// Get an mutable reference to item at index.
    #[inline]
    fn get_mut<I: slice::SliceIndex<[A::Item]>>(&mut self, index: I) -> Option<&mut I::Output> {
        return self.as_mut_slice().get_mut(index);
    }

    /// Get an immutable reference to item at index.
    #[inline]
    unsafe fn get_unchecked<I: slice::SliceIndex<[A::Item]>>(&self, index: I) -> &I::Output {
        return self.as_slice().get_unchecked(index);
    }

    /// Get an mutable reference to item at index.
    #[inline]
    unsafe fn get_unchecked_mut<I: slice::SliceIndex<[A::Item]>>(&mut self, index: I) -> &mut I::Output {
        return self.as_mut_slice().get_unchecked_mut(index);
    }

    // INDEX

    #[inline]
    fn index<I: slice::SliceIndex<[A::Item]>>(&self, index: I) -> &I::Output {
        return self.as_slice().index(index);
    }

    #[inline]
    fn index_mut<I: slice::SliceIndex<[A::Item]>>(&mut self, index: I) -> &mut I::Output {
        return self.as_mut_slice().index_mut(index);
    }

    // RGET

    #[inline]
    fn rget<I: RSliceIndex<[A::Item]>>(&self, index: I)
        -> Option<&I::Output>
    {
        index.rget(self.as_slice())
    }

    #[inline]
    fn rget_mut<I: RSliceIndex<[A::Item]>>(&mut self, index: I)
        -> Option<&mut I::Output>
    {
        index.rget_mut(self.as_mut_slice())
    }

    #[inline]
    unsafe fn rget_unchecked<I: RSliceIndex<[A::Item]>>(&self, index: I)
        -> &I::Output
    {
        index.rget_unchecked(self.as_slice())
    }

    #[inline]
    unsafe fn rget_unchecked_mut<I: RSliceIndex<[A::Item]>>(&mut self, index: I)
        -> &mut I::Output
    {
        index.rget_unchecked_mut(self.as_mut_slice())
    }

    // RINDEX

    #[inline]
    fn rindex<I: RSliceIndex<[A::Item]>>(&self, index: I) -> &I::Output {
        index.rindex(self.as_slice())
    }

    #[inline]
    fn rindex_mut<I: RSliceIndex<[A::Item]>>(&mut self, index: I) -> &mut I::Output {
        index.rindex_mut(self.as_mut_slice())
    }
}

// VECTOR
// ------

// VECLIKE

/// Vector-like container.
pub trait VecLike<T>:
    Default +
    iter::FromIterator<T> +
    iter::IntoIterator +
    ops::DerefMut<Target = [T]> +
    Extend<T> +
    SliceLike<T>
{
    /// Create new, empty vector.
    fn new() -> Self;

    /// Create new, empty vector with preallocated, uninitialized storage.
    fn with_capacity(capacity: usize) -> Self;

    /// Get the capacity of the underlying storage.
    fn capacity(&self) -> usize;

    /// Reserve additional capacity for the collection.
    fn reserve(&mut self, capacity: usize);

    /// Reserve minimal additional capacity for the collection.
    fn reserve_exact(&mut self, additional: usize);

    /// Shrink capacity to fit data size.
    fn shrink_to_fit(&mut self);

    /// Truncate vector to new length, dropping any items after `len`.
    fn truncate(&mut self, len: usize);

    /// Set the buffer length (unsafe).
    unsafe fn set_len(&mut self, new_len: usize);

    /// Remove element from vector and return it, replacing it with the last item in the vector.
    fn swap_remove(&mut self, index: usize) -> T;

    /// Insert element at index, shifting all elements after.
    fn insert(&mut self, index: usize, element: T);

    /// Remove element from vector at index, shifting all elements after.
    fn remove(&mut self, index: usize) -> T;

    /// Append an element to the vector.
    fn push(&mut self, value: T);

    /// Pop an element from the end of the vector.
    fn pop(&mut self) -> Option<T>;

    /// Clear the buffer
    fn clear(&mut self);

    /// Insert many elements at index, pushing everything else to the back.
    fn insert_many<I: iter::IntoIterator<Item=T>>(&mut self, index: usize, iterable: I);

    /// Remove many elements from range.
    fn remove_many<R: ops::RangeBounds<usize>>(&mut self, range: R);
}

#[cfg(all(feature = "correct", feature = "radix"))]
impl<T> VecLike<T> for Vec<T> {
    #[inline]
    fn new() -> Vec<T> {
        Vec::new()
    }

    #[inline]
    fn with_capacity(capacity: usize) -> Vec<T> {
        Vec::with_capacity(capacity)
    }

    #[inline]
    fn capacity(&self) -> usize {
        Vec::capacity(self)
    }

    #[inline]
    fn reserve(&mut self, capacity: usize) {
        Vec::reserve(self, capacity)
    }

    #[inline]
    fn reserve_exact(&mut self, capacity: usize) {
        Vec::reserve_exact(self, capacity)
    }

    #[inline]
    fn shrink_to_fit(&mut self) {
        Vec::shrink_to_fit(self)
    }

    #[inline]
    fn truncate(&mut self, len: usize) {
        Vec::truncate(self, len)
    }

    #[inline]
    unsafe fn set_len(&mut self, new_len: usize) {
        Vec::set_len(self, new_len);
    }

    #[inline]
    fn swap_remove(&mut self, index: usize) -> T {
        Vec::swap_remove(self, index)
    }

    #[inline]
    fn insert(&mut self, index: usize, element: T) {
        Vec::insert(self, index, element)
    }

    #[inline]
    fn remove(&mut self, index: usize) -> T {
        Vec::remove(self, index)
    }

    #[inline]
    fn push(&mut self, value: T) {
        Vec::push(self, value);
    }

    #[inline]
    fn pop(&mut self) -> Option<T> {
        Vec::pop(self)
    }

    #[inline]
    fn clear(&mut self) {
        Vec::clear(self);
    }

    #[inline]
    fn insert_many<I: iter::IntoIterator<Item=T>>(&mut self, index: usize, iterable: I) {
        self.splice(index..index, iterable);
    }

    #[inline]
    fn remove_many<R: ops::RangeBounds<usize>>(&mut self, range: R) {
        remove_many(self, range)
    }
}

impl<A: arrayvec::Array> VecLike<A::Item> for arrayvec::ArrayVec<A> {
    #[inline]
    fn new() -> arrayvec::ArrayVec<A> {
        arrayvec::ArrayVec::new()
    }

    #[inline]
    fn with_capacity(capacity: usize) -> arrayvec::ArrayVec<A> {
        let mut v = arrayvec::ArrayVec::new();
        v.reserve(capacity);
        v
    }

    #[inline]
    fn capacity(&self) -> usize {
        arrayvec::ArrayVec::capacity(self)
    }

    #[inline]
    fn reserve(&mut self, capacity: usize) {
        assert!(self.len() + capacity <= self.capacity());
    }

    #[inline]
    fn reserve_exact(&mut self, capacity: usize) {
        assert!(self.len() + capacity <= self.capacity());
    }

    #[inline]
    fn shrink_to_fit(&mut self) {
    }

    #[inline]
    fn truncate(&mut self, len: usize) {
        arrayvec::ArrayVec::truncate(self, len)
    }

    #[inline]
    unsafe fn set_len(&mut self, new_len: usize) {
        arrayvec::ArrayVec::set_len(self, new_len);
    }

    #[inline]
    fn swap_remove(&mut self, index: usize) -> A::Item {
        arrayvec::ArrayVec::swap_remove(self, index)
    }

    #[inline]
    fn insert(&mut self, index: usize, element: A::Item) {
        arrayvec::ArrayVec::insert(self, index, element)
    }

    #[inline]
    fn remove(&mut self, index: usize) -> A::Item {
        arrayvec::ArrayVec::remove(self, index)
    }

    #[inline]
    fn push(&mut self, value: A::Item) {
        arrayvec::ArrayVec::push(self, value);
    }

    #[inline]
    fn pop(&mut self) -> Option<A::Item> {
        arrayvec::ArrayVec::pop(self)
    }

    #[inline]
    fn clear(&mut self) {
        arrayvec::ArrayVec::clear(self);
    }

    #[inline]
    fn insert_many<I: iter::IntoIterator<Item=A::Item>>(&mut self, index: usize, iterable: I) {
        insert_many(self, index, iterable)
    }

    #[inline]
    fn remove_many<R: ops::RangeBounds<usize>>(&mut self, range: R) {
        remove_many(self, range)
    }
}

// CLONEABLE VECLIKE

/// Vector-like container with cloneable values.
///
/// Implemented for Vec, SmallVec, and StackVec.
pub trait CloneableVecLike<T: Clone + Copy + Send>: Send + VecLike<T>
{
    /// Extend collection from slice.
    fn extend_from_slice(&mut self, other: &[T]);

    /// Resize container to new length, with a default value if adding elements.
    fn resize(&mut self, len: usize, value: T);
}

#[cfg(all(feature = "correct", feature = "radix"))]
impl<T> CloneableVecLike<T> for Vec<T>
    where T: Clone + Copy + Send
{
    #[inline]
    fn extend_from_slice(&mut self, other: &[T]) {
        Vec::extend_from_slice(self, other)
    }

    #[inline]
    fn resize(&mut self, len: usize, value: T) {
        Vec::resize(self, len, value)
    }
}

impl<A: arrayvec::Array> CloneableVecLike<A::Item> for arrayvec::ArrayVec<A>
    where A: Send,
          A::Index: Send,
          A::Item: Clone + Copy + Send
{
    #[inline]
    fn extend_from_slice(&mut self, other: &[A::Item]) {
        self.extend(other.iter().cloned())
    }

    #[inline]
    fn resize(&mut self, len: usize, value: A::Item) {
        assert!(len <= self.capacity());
        let old_len = self.len();
        if len > old_len {
            self.extend(iter::repeat(value).take(len - old_len));
        } else {
            self.truncate(len);
        }
    }
}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_many() {
        type V = arrayvec::ArrayVec<[u8; 8]>;
        let mut v: V = V::new();
        for x in 0..4 {
            v.push(x);
        }
        assert_eq!(v.len(), 4);
        v.insert_many(1, [5, 6].iter().cloned());
        assert_eq!(&v[..], &[0, 5, 6, 1, 2, 3]);
    }

    #[cfg(all(feature = "correct", feature = "radix"))]
    #[test]
    fn remove_many_test() {
        let mut x = vec![0, 1, 2, 3, 4, 5];
        x.remove_many(0..3);
        assert_eq!(x, vec![3, 4, 5]);
        assert_eq!(x.len(), 3);

        let mut x = vec![0, 1, 2, 3, 4, 5];
        x.remove_many(..);
        assert_eq!(x, vec![]);

        let mut x = vec![0, 1, 2, 3, 4, 5];
        x.remove_many(3..);
        assert_eq!(x, vec![0, 1, 2]);

        let mut x = vec![0, 1, 2, 3, 4, 5];
        x.remove_many(..3);
        assert_eq!(x, vec![3, 4, 5]);
    }
}
