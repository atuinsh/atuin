// Copyright 2018 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Low-level API for sampling indices

#[cfg(feature = "alloc")] use core::slice;

#[cfg(all(feature = "alloc", not(feature = "std")))]
use crate::alloc::vec::{self, Vec};
#[cfg(feature = "std")] use std::vec;
// BTreeMap is not as fast in tests, but better than nothing.
#[cfg(all(feature = "alloc", not(feature = "std")))]
use crate::alloc::collections::BTreeSet;
#[cfg(feature = "std")] use std::collections::HashSet;

#[cfg(feature = "alloc")]
use crate::distributions::{uniform::SampleUniform, Distribution, Uniform};
use crate::Rng;

/// A vector of indices.
///
/// Multiple internal representations are possible.
#[derive(Clone, Debug)]
pub enum IndexVec {
    #[doc(hidden)]
    U32(Vec<u32>),
    #[doc(hidden)]
    USize(Vec<usize>),
}

impl IndexVec {
    /// Returns the number of indices
    #[inline]
    pub fn len(&self) -> usize {
        match *self {
            IndexVec::U32(ref v) => v.len(),
            IndexVec::USize(ref v) => v.len(),
        }
    }

    /// Returns `true` if the length is 0.
    #[inline]
    pub fn is_empty(&self) -> bool {
        match *self {
            IndexVec::U32(ref v) => v.is_empty(),
            IndexVec::USize(ref v) => v.is_empty(),
        }
    }

    /// Return the value at the given `index`.
    ///
    /// (Note: we cannot implement [`std::ops::Index`] because of lifetime
    /// restrictions.)
    #[inline]
    pub fn index(&self, index: usize) -> usize {
        match *self {
            IndexVec::U32(ref v) => v[index] as usize,
            IndexVec::USize(ref v) => v[index],
        }
    }

    /// Return result as a `Vec<usize>`. Conversion may or may not be trivial.
    #[inline]
    pub fn into_vec(self) -> Vec<usize> {
        match self {
            IndexVec::U32(v) => v.into_iter().map(|i| i as usize).collect(),
            IndexVec::USize(v) => v,
        }
    }

    /// Iterate over the indices as a sequence of `usize` values
    #[inline]
    pub fn iter(&self) -> IndexVecIter<'_> {
        match *self {
            IndexVec::U32(ref v) => IndexVecIter::U32(v.iter()),
            IndexVec::USize(ref v) => IndexVecIter::USize(v.iter()),
        }
    }

    /// Convert into an iterator over the indices as a sequence of `usize` values
    #[inline]
    pub fn into_iter(self) -> IndexVecIntoIter {
        match self {
            IndexVec::U32(v) => IndexVecIntoIter::U32(v.into_iter()),
            IndexVec::USize(v) => IndexVecIntoIter::USize(v.into_iter()),
        }
    }
}

impl PartialEq for IndexVec {
    fn eq(&self, other: &IndexVec) -> bool {
        use self::IndexVec::*;
        match (self, other) {
            (&U32(ref v1), &U32(ref v2)) => v1 == v2,
            (&USize(ref v1), &USize(ref v2)) => v1 == v2,
            (&U32(ref v1), &USize(ref v2)) => {
                (v1.len() == v2.len()) && (v1.iter().zip(v2.iter()).all(|(x, y)| *x as usize == *y))
            }
            (&USize(ref v1), &U32(ref v2)) => {
                (v1.len() == v2.len()) && (v1.iter().zip(v2.iter()).all(|(x, y)| *x == *y as usize))
            }
        }
    }
}

impl From<Vec<u32>> for IndexVec {
    #[inline]
    fn from(v: Vec<u32>) -> Self {
        IndexVec::U32(v)
    }
}

impl From<Vec<usize>> for IndexVec {
    #[inline]
    fn from(v: Vec<usize>) -> Self {
        IndexVec::USize(v)
    }
}

/// Return type of `IndexVec::iter`.
#[derive(Debug)]
pub enum IndexVecIter<'a> {
    #[doc(hidden)]
    U32(slice::Iter<'a, u32>),
    #[doc(hidden)]
    USize(slice::Iter<'a, usize>),
}

impl<'a> Iterator for IndexVecIter<'a> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        use self::IndexVecIter::*;
        match *self {
            U32(ref mut iter) => iter.next().map(|i| *i as usize),
            USize(ref mut iter) => iter.next().cloned(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match *self {
            IndexVecIter::U32(ref v) => v.size_hint(),
            IndexVecIter::USize(ref v) => v.size_hint(),
        }
    }
}

impl<'a> ExactSizeIterator for IndexVecIter<'a> {}

/// Return type of `IndexVec::into_iter`.
#[derive(Clone, Debug)]
pub enum IndexVecIntoIter {
    #[doc(hidden)]
    U32(vec::IntoIter<u32>),
    #[doc(hidden)]
    USize(vec::IntoIter<usize>),
}

impl Iterator for IndexVecIntoIter {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        use self::IndexVecIntoIter::*;
        match *self {
            U32(ref mut v) => v.next().map(|i| i as usize),
            USize(ref mut v) => v.next(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        use self::IndexVecIntoIter::*;
        match *self {
            U32(ref v) => v.size_hint(),
            USize(ref v) => v.size_hint(),
        }
    }
}

impl ExactSizeIterator for IndexVecIntoIter {}


/// Randomly sample exactly `amount` distinct indices from `0..length`, and
/// return them in random order (fully shuffled).
///
/// This method is used internally by the slice sampling methods, but it can
/// sometimes be useful to have the indices themselves so this is provided as
/// an alternative.
///
/// The implementation used is not specified; we automatically select the
/// fastest available algorithm for the `length` and `amount` parameters
/// (based on detailed profiling on an Intel Haswell CPU). Roughly speaking,
/// complexity is `O(amount)`, except that when `amount` is small, performance
/// is closer to `O(amount^2)`, and when `length` is close to `amount` then
/// `O(length)`.
///
/// Note that performance is significantly better over `u32` indices than over
/// `u64` indices. Because of this we hide the underlying type behind an
/// abstraction, `IndexVec`.
///
/// If an allocation-free `no_std` function is required, it is suggested
/// to adapt the internal `sample_floyd` implementation.
///
/// Panics if `amount > length`.
pub fn sample<R>(rng: &mut R, length: usize, amount: usize) -> IndexVec
where R: Rng + ?Sized {
    if amount > length {
        panic!("`amount` of samples must be less than or equal to `length`");
    }
    if length > (::core::u32::MAX as usize) {
        // We never want to use inplace here, but could use floyd's alg
        // Lazy version: always use the cache alg.
        return sample_rejection(rng, length, amount);
    }
    let amount = amount as u32;
    let length = length as u32;

    // Choice of algorithm here depends on both length and amount. See:
    // https://github.com/rust-random/rand/pull/479
    // We do some calculations with f32. Accuracy is not very important.

    if amount < 163 {
        const C: [[f32; 2]; 2] = [[1.6, 8.0 / 45.0], [10.0, 70.0 / 9.0]];
        let j = if length < 500_000 { 0 } else { 1 };
        let amount_fp = amount as f32;
        let m4 = C[0][j] * amount_fp;
        // Short-cut: when amount < 12, floyd's is always faster
        if amount > 11 && (length as f32) < (C[1][j] + m4) * amount_fp {
            sample_inplace(rng, length, amount)
        } else {
            sample_floyd(rng, length, amount)
        }
    } else {
        const C: [f32; 2] = [270.0, 330.0 / 9.0];
        let j = if length < 500_000 { 0 } else { 1 };
        if (length as f32) < C[j] * (amount as f32) {
            sample_inplace(rng, length, amount)
        } else {
            sample_rejection(rng, length, amount)
        }
    }
}

/// Randomly sample exactly `amount` indices from `0..length`, using Floyd's
/// combination algorithm.
///
/// The output values are fully shuffled. (Overhead is under 50%.)
///
/// This implementation uses `O(amount)` memory and `O(amount^2)` time.
fn sample_floyd<R>(rng: &mut R, length: u32, amount: u32) -> IndexVec
where R: Rng + ?Sized {
    // For small amount we use Floyd's fully-shuffled variant. For larger
    // amounts this is slow due to Vec::insert performance, so we shuffle
    // afterwards. Benchmarks show little overhead from extra logic.
    let floyd_shuffle = amount < 50;

    debug_assert!(amount <= length);
    let mut indices = Vec::with_capacity(amount as usize);
    for j in length - amount..length {
        let t = rng.gen_range(0, j + 1);
        if floyd_shuffle {
            if let Some(pos) = indices.iter().position(|&x| x == t) {
                indices.insert(pos, j);
                continue;
            }
        } else if indices.contains(&t) {
            indices.push(j);
            continue;
        }
        indices.push(t);
    }
    if !floyd_shuffle {
        // Reimplement SliceRandom::shuffle with smaller indices
        for i in (1..amount).rev() {
            // invariant: elements with index > i have been locked in place.
            indices.swap(i as usize, rng.gen_range(0, i + 1) as usize);
        }
    }
    IndexVec::from(indices)
}

/// Randomly sample exactly `amount` indices from `0..length`, using an inplace
/// partial Fisher-Yates method.
/// Sample an amount of indices using an inplace partial fisher yates method.
///
/// This allocates the entire `length` of indices and randomizes only the first `amount`.
/// It then truncates to `amount` and returns.
///
/// This method is not appropriate for large `length` and potentially uses a lot
/// of memory; because of this we only implement for `u32` index (which improves
/// performance in all cases).
///
/// Set-up is `O(length)` time and memory and shuffling is `O(amount)` time.
fn sample_inplace<R>(rng: &mut R, length: u32, amount: u32) -> IndexVec
where R: Rng + ?Sized {
    debug_assert!(amount <= length);
    let mut indices: Vec<u32> = Vec::with_capacity(length as usize);
    indices.extend(0..length);
    for i in 0..amount {
        let j: u32 = rng.gen_range(i, length);
        indices.swap(i as usize, j as usize);
    }
    indices.truncate(amount as usize);
    debug_assert_eq!(indices.len(), amount as usize);
    IndexVec::from(indices)
}

trait UInt: Copy + PartialOrd + Ord + PartialEq + Eq + SampleUniform + core::hash::Hash {
    fn zero() -> Self;
    fn as_usize(self) -> usize;
}
impl UInt for u32 {
    #[inline]
    fn zero() -> Self {
        0
    }

    #[inline]
    fn as_usize(self) -> usize {
        self as usize
    }
}
impl UInt for usize {
    #[inline]
    fn zero() -> Self {
        0
    }

    #[inline]
    fn as_usize(self) -> usize {
        self
    }
}

/// Randomly sample exactly `amount` indices from `0..length`, using rejection
/// sampling.
///
/// Since `amount <<< length` there is a low chance of a random sample in
/// `0..length` being a duplicate. We test for duplicates and resample where
/// necessary. The algorithm is `O(amount)` time and memory.
///
/// This function  is generic over X primarily so that results are value-stable
/// over 32-bit and 64-bit platforms.
fn sample_rejection<X: UInt, R>(rng: &mut R, length: X, amount: X) -> IndexVec
where
    R: Rng + ?Sized,
    IndexVec: From<Vec<X>>,
{
    debug_assert!(amount < length);
    #[cfg(feature = "std")]
    let mut cache = HashSet::with_capacity(amount.as_usize());
    #[cfg(not(feature = "std"))]
    let mut cache = BTreeSet::new();
    let distr = Uniform::new(X::zero(), length);
    let mut indices = Vec::with_capacity(amount.as_usize());
    for _ in 0..amount.as_usize() {
        let mut pos = distr.sample(rng);
        while !cache.insert(pos) {
            pos = distr.sample(rng);
        }
        indices.push(pos);
    }

    debug_assert_eq!(indices.len(), amount.as_usize());
    IndexVec::from(indices)
}

#[cfg(test)]
mod test {
    use super::*;
    #[cfg(all(feature = "alloc", not(feature = "std")))] use crate::alloc::vec;
    #[cfg(feature = "std")] use std::vec;

    #[test]
    fn test_sample_boundaries() {
        let mut r = crate::test::rng(404);

        assert_eq!(sample_inplace(&mut r, 0, 0).len(), 0);
        assert_eq!(sample_inplace(&mut r, 1, 0).len(), 0);
        assert_eq!(sample_inplace(&mut r, 1, 1).into_vec(), vec![0]);

        assert_eq!(sample_rejection(&mut r, 1u32, 0).len(), 0);

        assert_eq!(sample_floyd(&mut r, 0, 0).len(), 0);
        assert_eq!(sample_floyd(&mut r, 1, 0).len(), 0);
        assert_eq!(sample_floyd(&mut r, 1, 1).into_vec(), vec![0]);

        // These algorithms should be fast with big numbers. Test average.
        let sum: usize = sample_rejection(&mut r, 1 << 25, 10u32).into_iter().sum();
        assert!(1 << 25 < sum && sum < (1 << 25) * 25);

        let sum: usize = sample_floyd(&mut r, 1 << 25, 10).into_iter().sum();
        assert!(1 << 25 < sum && sum < (1 << 25) * 25);
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri is too slow
    fn test_sample_alg() {
        let seed_rng = crate::test::rng;

        // We can't test which algorithm is used directly, but Floyd's alg
        // should produce different results from the others. (Also, `inplace`
        // and `cached` currently use different sizes thus produce different results.)

        // A small length and relatively large amount should use inplace
        let (length, amount): (usize, usize) = (100, 50);
        let v1 = sample(&mut seed_rng(420), length, amount);
        let v2 = sample_inplace(&mut seed_rng(420), length as u32, amount as u32);
        assert!(v1.iter().all(|e| e < length));
        assert_eq!(v1, v2);

        // Test Floyd's alg does produce different results
        let v3 = sample_floyd(&mut seed_rng(420), length as u32, amount as u32);
        assert!(v1 != v3);

        // A large length and small amount should use Floyd
        let (length, amount): (usize, usize) = (1 << 20, 50);
        let v1 = sample(&mut seed_rng(421), length, amount);
        let v2 = sample_floyd(&mut seed_rng(421), length as u32, amount as u32);
        assert!(v1.iter().all(|e| e < length));
        assert_eq!(v1, v2);

        // A large length and larger amount should use cache
        let (length, amount): (usize, usize) = (1 << 20, 600);
        let v1 = sample(&mut seed_rng(422), length, amount);
        let v2 = sample_rejection(&mut seed_rng(422), length as u32, amount as u32);
        assert!(v1.iter().all(|e| e < length));
        assert_eq!(v1, v2);
    }
}
