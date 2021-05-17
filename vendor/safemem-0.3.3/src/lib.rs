//! Safe wrappers for memory-accessing functions like `std::ptr::copy()`.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate core as std;
use std::ptr;

macro_rules! idx_check (
    ($slice:expr, $idx:expr) => {
        assert!($idx < $slice.len(),
            concat!("`", stringify!($idx), "` ({}) out of bounds. Length: {}"),
            $idx, $slice.len());
    }
);

macro_rules! len_check (
    ($slice:expr, $start:ident, $len:ident) => {
        assert!(
            $start.checked_add($len)
                .expect(concat!("Overflow evaluating ", stringify!($start + $len)))
                <= $slice.len(),
            "Length {} starting at {} is out of bounds (slice len {}).", $len, $start, $slice.len()
        )
    }
);

/// Copy `len` elements from `src_idx` to `dest_idx`. Ranges may overlap.
///
/// Safe wrapper for `memmove()`/`std::ptr::copy()`.
///
/// ###Panics
/// * If either `src_idx` or `dest_idx` are out of bounds, or if either of these plus `len` is out of
/// bounds.
/// * If `src_idx + len` or `dest_idx + len` overflows.
pub fn copy_over<T: Copy>(slice: &mut [T], src_idx: usize, dest_idx: usize, len: usize) {
    if slice.len() == 0 { return; }

    idx_check!(slice, src_idx);
    idx_check!(slice, dest_idx);
    len_check!(slice, src_idx, len);
    len_check!(slice, dest_idx, len);

    // At any point a Rust reference exists, the compiler is free to do this.
    // So we explicitely add it to be caught by miri.
    #[cfg(miri)]
    slice.iter().copied().for_each(drop);

    let ptr = slice.as_mut_ptr();

    unsafe {
        ptr::copy(ptr.offset(src_idx as isize), ptr.offset(dest_idx as isize), len);
    }
}

/// Safe wrapper for `std::ptr::write_bytes()`/`memset()`.
pub fn write_bytes(slice: &mut [u8], byte: u8) {
    unsafe {
        ptr::write_bytes(slice.as_mut_ptr(), byte, slice.len());
    }
}

/// Prepend `elems` to `vec`, resizing if necessary.
///
/// ### Panics
///
/// If `vec.len() + elems.len()` overflows.
#[cfg(feature = "std")]
pub fn prepend<T: Copy>(elems: &[T], vec: &mut Vec<T>) {
    let elems_len = elems.len(); // `<= isize::MAX as usize`
    if elems_len == 0 { return; }

    let old_len = vec.len(); // `<= isize::MAX as usize`
    if old_len == 0 {
        // Prepend = append: delegate to Rust's stdlib implementation.
        vec.extend_from_slice(elems);
    } else {
        // Our overflow check occurs here, no need to do it ourselves.
        vec.reserve(elems_len);
        let ptr = vec.as_mut_ptr();
        unsafe {
            // Move the old elements down to the end.
            ptr::copy(
                ptr,
                ptr.offset(elems_len as isize),
                old_len,
            );
            // Copy the input elements to the start
            ptr::copy_nonoverlapping(
                elems.as_ptr(),
                ptr,
                elems_len,
            );
            // Set the len *after* having initialized the elements.
            vec.set_len(old_len + elems_len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn bounds_check() {
        let mut arr = [0i32, 1, 2, 3, 4, 5];

        copy_over(&mut arr, 2, 1, 7);
    }

    #[test]
    fn copy_empty() {
        let mut arr: [i32; 0] = [];

        copy_over(&mut arr, 0, 0, 0);
    }

    #[test]
    #[cfg(feature = "std")]
    fn prepend_empty() {
        let mut vec: Vec<i32> = vec![];
        prepend(&[1, 2, 3], &mut vec);
    }

    #[test]
    #[cfg(feature = "std")]
    fn prepend_i32() {
        let mut vec = vec![3, 4, 5];
        prepend(&[1, 2], &mut vec);
        assert_eq!(vec, &[1, 2, 3, 4, 5]);
    }

    /// Detect potential uninit values when running miri
    #[test]
    #[cfg(all(
        feature = "std",
        miri,
    ))]
    fn prepend_bool() {
        prepend(&[true], &mut vec![false]);
    }
}
