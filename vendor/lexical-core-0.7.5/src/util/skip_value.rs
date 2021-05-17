//! An iterator that skips values equal to a provided value.
//!
//! SkipValueIterator iterates over a slice, returning all values
//! except for those matching the provided skip value.
//!
//! Example
//! -------
//!
//! ```text
//! let iter = SkipValueIterator(&[1, 2, 5, 2, 6, 7], 2);
//! assert!(iter.eq([1, 5, 6, 7].iter()));
//! ```

use crate::lib::slice;
use super::iterator::*;

/// Slice iterator that skips characters matching a given value.
pub(crate) struct SkipValueIterator<'a, T: 'a + PartialEq> {
    /// Slice iterator to wrap.
    iter: slice::Iter<'a, T>,
    /// Value to skip.
    skip: T
}

impl<'a, T: 'a + PartialEq> SkipValueIterator<'a, T> {
    #[inline]
    pub(crate) fn new(slc: &'a [T], skip: T) -> Self {
        SkipValueIterator {
            iter: slc.iter(),
            skip: skip
        }
    }
}

impl<'a, T: 'a + PartialEq + Clone> Clone for SkipValueIterator<'a, T> {
    #[inline]
    fn clone(&self) -> Self {
        SkipValueIterator {
            iter: self.iter.clone(),
            skip: self.skip.clone()
        }
    }
}

impl<'a, T: 'a + PartialEq> Iterator for SkipValueIterator<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let value = self.iter.next()?;
            if *value != self.skip {
                return Some(value);
            }
        }
    }
}

impl<'a, T: 'a + PartialEq> ConsumedIterator for SkipValueIterator<'a, T> {
    // Preconditions: The iterator cannot end with `skip` characters.
    // Use debug_assert to enforce this is removed successfully in test scenarios.
    #[inline]
    fn consumed(&self) -> bool {
        // This implementation is essentially a hack.
        // We rely on callers to ensure this is only ever called without
        // any trailing digit separators, otherwise, it will incorrectly
        // report if the iterator itself is consumed.
        debug_assert!(self.iter.as_slice().last() != Some(&self.skip));
        self.iter.len() == 0
    }
}

impl<'a, T: 'a + PartialEq> AsPtrIterator<'a, T> for SkipValueIterator<'a, T> {
    #[inline]
    fn as_ptr(&self) -> *const T {
        self.iter.as_slice().as_ptr()
    }
}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_value_test() {
        let slc = &[1, 2, 5, 2, 6, 7];
        let iter = SkipValueIterator::new(slc, 2);
        assert!(iter.eq([1, 5, 6, 7].iter()));

        let iter = SkipValueIterator::new(slc, 5);
        assert!(iter.eq([1, 2, 2, 6, 7].iter()));

        let iter = SkipValueIterator::new(slc, 1);
        assert!(iter.eq([2, 5, 2, 6, 7].iter()));
    }
}
