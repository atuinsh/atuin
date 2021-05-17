//! Simple, shared algorithm utilities.

use crate::lib::convert::AsRef;
use crate::lib::{mem, ptr, slice};

// ALGORITHMS

/// Calculate the difference between two pointers.
#[inline]
pub fn distance<T>(first: *const T, last: *const T)
    -> usize
{
    debug_assert!(last >= first, "range must be positive.");
    let f = first as usize;
    let l = last as usize;
    l - f
}

/// Check if two slices are equal to each other.
#[inline]
pub fn equal_to_slice(l: &[u8], r: &[u8])
    -> bool
{
    l == r
}

/// Check if left iter starts with right iter.
#[inline]
#[cfg(feature = "format")]
pub fn starts_with_iter<'a, Iter1, Iter2>(mut l: Iter1, mut r: Iter2)
    -> (bool, Iter1)
    where Iter1: Iterator<Item=&'a u8>,
          Iter2: Iterator<Item=&'a u8>
{
    loop {
        // Only call `next()` on l if r is not None, otherwise,
        // we may incorrectly consume an l character.
        let ri = r.next();
        if ri.is_none() {
            return (true, l);
        } else if l.next() != ri {
            return (false, l);
        }
    }
}

/// Check if left iter starts with right iter without case-sensitivity.
#[inline]
pub fn case_insensitive_starts_with_iter<'a, Iter1, Iter2>(mut l: Iter1, mut r: Iter2)
    -> (bool, Iter1)
    where Iter1: Iterator<Item=&'a u8>,
          Iter2: Iterator<Item=&'a u8>
{
    loop {
        let ri = r.next().map(|x| x.to_ascii_lowercase());
        if ri.is_none() {
            return (true, l);
        } else if l.next().map(|x| x.to_ascii_lowercase()) != ri {
            return (false, l);
        }
    }
}

/// Check if left slice ends with right slice.
#[inline]
pub fn ends_with_slice(l: &[u8], r: &[u8])
    -> bool
{
    // This cannot be out-of-bounds, since we check `l.len() >= r.len()`
    // previous to extracting the subslice, so `l.len() - r.len()` must
    // also be <= l.len() and >= 0.
    let rget = move || unsafe {l.get_unchecked(l.len()-r.len()..)};
    l.len() >= r.len() && equal_to_slice(rget(), r)
}

/// Trim character from the left-side of a slice.
#[inline]
pub fn ltrim_char_slice<'a>(slc: &'a [u8], c: u8)
    -> (&'a [u8], usize)
{
    let count = slc.iter().take_while(|&&si| si == c).count();
    //  This count cannot exceed the bounds of the slice, since it is
    // derived from an iterator using the standard library to generate it.
    debug_assert!(count <= slc.len());
    let slc = unsafe {slc.get_unchecked(count..)};
    (slc, count)
}

/// Trim characters from the left-side of a slice.
#[inline]
#[cfg(feature = "format")]
pub fn ltrim_char2_slice<'a>(slc: &'a [u8], c1: u8, c2: u8)
    -> (&'a [u8], usize)
{
    let count = slc.iter().take_while(|&&si| si == c1 || si == c2).count();
    //  This count cannot exceed the bounds of the slice, since it is
    // derived from an iterator using the standard library to generate it.
    debug_assert!(count <= slc.len());
    let slc = unsafe {slc.get_unchecked(count..)};
    (slc, count)
}

/// Trim character from the right-side of a slice.
#[inline]
pub fn rtrim_char_slice<'a>(slc: &'a [u8], c: u8)
    -> (&'a [u8], usize)
{
    let count = slc.iter().rev().take_while(|&&si| si == c).count();
    let index = slc.len() - count;
    // Count must be <= slc.len(), and therefore, slc.len() - count must
    // also be <= slc.len(), since this is derived from an iterator
    // in the standard library.
    debug_assert!(count <= slc.len());
    debug_assert!(index <= slc.len());
    let slc = unsafe {slc.get_unchecked(..index)};
    (slc, count)
}

/// Trim character from the right-side of a slice.
#[inline]
#[cfg(feature = "format")]
pub fn rtrim_char2_slice<'a>(slc: &'a [u8], c1: u8, c2: u8)
    -> (&'a [u8], usize)
{
    let count = slc.iter().rev().take_while(|&&si| si == c1 || si == c2).count();
    let index = slc.len() - count;
    // Count must be <= slc.len(), and therefore, slc.len() - count must
    // also be <= slc.len(), since this is derived from an iterator
    // in the standard library.
    debug_assert!(count <= slc.len());
    debug_assert!(index <= slc.len());
    let slc = unsafe {slc.get_unchecked(..index)};
    (slc, count)
}

/// Copy from source-to-dst.
#[inline]
pub fn copy_to_dst<'a, Bytes: AsRef<[u8]>>(dst: &'a mut [u8], src: Bytes)
    -> usize
{
    let src = src.as_ref();
    let dst = &mut index_mut!(dst[..src.len()]);

    unsafe {
        ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    src.len()
}

/// Length-check variant of ptr::write_bytes for a slice.
#[cfg(not(any(feature = "grisu3", feature = "ryu")))]
#[inline]
pub fn write_bytes(dst: &mut [u8], byte: u8)
{
    unsafe {
        ptr::write_bytes(dst.as_mut_ptr(), byte, dst.len());
    }
}

// TEST
// ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_test() {
        unsafe {
            let x: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
            let first: *const u8 = x.as_ptr();
            let last = first.add(x.len());
            assert_eq!(distance(first, last), 10);
        }
    }

    #[test]
    fn equal_to_test() {
        let x = "Hello";
        let y = "Hello";
        let z = "hello";

        assert!(equal_to_slice(x.as_bytes(), y.as_bytes()));
        assert!(!equal_to_slice(x.as_bytes(), z.as_bytes()));
        assert!(!equal_to_slice(y.as_bytes(), z.as_bytes()));
    }

    #[test]
    #[cfg(feature = "format")]
    fn starts_with_test() {
        let w = b"Hello";
        let x = b"H";
        let y = b"h";
        let z = b"a";

        // forward
        assert!(starts_with_iter(w.iter(), x.iter()).0);
        assert!(!starts_with_iter(w.iter(), y.iter()).0);
        assert!(!starts_with_iter(x.iter(), y.iter()).0);
        assert!(!starts_with_iter(w.iter(), z.iter()).0);
        assert!(!starts_with_iter(x.iter(), z.iter()).0);
        assert!(!starts_with_iter(y.iter(), z.iter()).0);

        // back
        assert!(!starts_with_iter(x.iter(), w.iter()).0);
        assert!(!starts_with_iter(y.iter(), w.iter()).0);
        assert!(!starts_with_iter(z.iter(), w.iter()).0);
    }

    #[test]
    fn case_insensitive_starts_with_test() {
        let w = b"Hello";
        let x = b"H";
        let y = b"h";
        let z = b"a";

        // forward
        assert!(case_insensitive_starts_with_iter(w.iter(), x.iter()).0);
        assert!(case_insensitive_starts_with_iter(w.iter(), y.iter()).0);
        assert!(case_insensitive_starts_with_iter(x.iter(), y.iter()).0);
        assert!(!case_insensitive_starts_with_iter(w.iter(), z.iter()).0);
        assert!(!case_insensitive_starts_with_iter(x.iter(), z.iter()).0);
        assert!(!case_insensitive_starts_with_iter(y.iter(), z.iter()).0);

        // back
        assert!(!case_insensitive_starts_with_iter(x.iter(), w.iter()).0);
        assert!(!case_insensitive_starts_with_iter(y.iter(), w.iter()).0);
        assert!(!case_insensitive_starts_with_iter(z.iter(), w.iter()).0);
    }

    #[test]
    fn ends_with_test() {
        let w = "Hello";
        let x = "lO";
        let y = "lo";
        let z = "o";

        // forward
        assert!(!ends_with_slice(w.as_bytes(), x.as_bytes()));
        assert!(ends_with_slice(w.as_bytes(), y.as_bytes()));
        assert!(ends_with_slice(w.as_bytes(), z.as_bytes()));
        assert!(!ends_with_slice(x.as_bytes(), y.as_bytes()));
        assert!(!ends_with_slice(x.as_bytes(), z.as_bytes()));
        assert!(ends_with_slice(y.as_bytes(), z.as_bytes()));

        // back
        assert!(!ends_with_slice(z.as_bytes(), y.as_bytes()));
        assert!(!ends_with_slice(z.as_bytes(), x.as_bytes()));
        assert!(!ends_with_slice(z.as_bytes(), w.as_bytes()));
        assert!(!ends_with_slice(y.as_bytes(), x.as_bytes()));
        assert!(!ends_with_slice(y.as_bytes(), w.as_bytes()));
        assert!(!ends_with_slice(x.as_bytes(), w.as_bytes()));
    }

    #[test]
    fn ltrim_char_test() {
        let w = "0001";
        let x = "1010";
        let y = "1.00";
        let z = "1e05";

        assert_eq!(ltrim_char_slice(w.as_bytes(), b'0').1, 3);
        assert_eq!(ltrim_char_slice(x.as_bytes(), b'0').1, 0);
        assert_eq!(ltrim_char_slice(x.as_bytes(), b'1').1, 1);
        assert_eq!(ltrim_char_slice(y.as_bytes(), b'0').1, 0);
        assert_eq!(ltrim_char_slice(y.as_bytes(), b'1').1, 1);
        assert_eq!(ltrim_char_slice(z.as_bytes(), b'0').1, 0);
        assert_eq!(ltrim_char_slice(z.as_bytes(), b'1').1, 1);
    }

    #[test]
    #[cfg(feature = "format")]
    fn ltrim_char2_test() {
        let w = "0001";
        let x = "1010";
        let y = "1.00";
        let z = "1e05";
        let a = "0_01";

        assert_eq!(ltrim_char2_slice(w.as_bytes(), b'0', b'_').1, 3);
        assert_eq!(ltrim_char2_slice(x.as_bytes(), b'0', b'_').1, 0);
        assert_eq!(ltrim_char2_slice(x.as_bytes(), b'1', b'_').1, 1);
        assert_eq!(ltrim_char2_slice(y.as_bytes(), b'0', b'_').1, 0);
        assert_eq!(ltrim_char2_slice(y.as_bytes(), b'1', b'_').1, 1);
        assert_eq!(ltrim_char2_slice(z.as_bytes(), b'0', b'_').1, 0);
        assert_eq!(ltrim_char2_slice(z.as_bytes(), b'1', b'_').1, 1);
        assert_eq!(ltrim_char2_slice(a.as_bytes(), b'0', b'_').1, 3);
        assert_eq!(ltrim_char2_slice(a.as_bytes(), b'1', b'_').1, 0);
    }

    #[test]
    fn rtrim_char_test() {
        let w = "0001";
        let x = "1010";
        let y = "1.00";
        let z = "1e05";

        assert_eq!(rtrim_char_slice(w.as_bytes(), b'0').1, 0);
        assert_eq!(rtrim_char_slice(x.as_bytes(), b'0').1, 1);
        assert_eq!(rtrim_char_slice(x.as_bytes(), b'1').1, 0);
        assert_eq!(rtrim_char_slice(y.as_bytes(), b'0').1, 2);
        assert_eq!(rtrim_char_slice(y.as_bytes(), b'1').1, 0);
        assert_eq!(rtrim_char_slice(z.as_bytes(), b'0').1, 0);
        assert_eq!(rtrim_char_slice(z.as_bytes(), b'5').1, 1);
    }

    #[test]
    #[cfg(feature = "format")]
    fn rtrim_char2_test() {
        let w = "0001";
        let x = "1010";
        let y = "1.00";
        let z = "1e05";
        let a = "0_01";

        assert_eq!(rtrim_char2_slice(w.as_bytes(), b'0', b'_').1, 0);
        assert_eq!(rtrim_char2_slice(x.as_bytes(), b'0', b'_').1, 1);
        assert_eq!(rtrim_char2_slice(x.as_bytes(), b'1', b'_').1, 0);
        assert_eq!(rtrim_char2_slice(y.as_bytes(), b'0', b'_').1, 2);
        assert_eq!(rtrim_char2_slice(y.as_bytes(), b'1', b'_').1, 0);
        assert_eq!(rtrim_char2_slice(z.as_bytes(), b'0', b'_').1, 0);
        assert_eq!(rtrim_char2_slice(z.as_bytes(), b'1', b'_').1, 0);
        assert_eq!(rtrim_char2_slice(a.as_bytes(), b'0', b'_').1, 0);
        assert_eq!(rtrim_char2_slice(a.as_bytes(), b'1', b'_').1, 1);
    }
}
