use std::collections::{hash_set, HashSet};
use std::fmt::{self, Debug};
use std::hash::{BuildHasher, Hash};
use std::iter::FromIterator;
use std::ops::{BitAnd, BitOr, BitXor, Deref, DerefMut, Sub};

/// A [`HashSet`](std::collections::HashSet) using [`RandomState`](crate::RandomState) to hash the items.
/// Requires the `std` feature to be enabled.
#[derive(Clone)]
pub struct AHashSet<T, S = crate::RandomState>(HashSet<T, S>);

impl<T, S> AHashSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher + Default,
{
    pub fn new() -> Self {
        AHashSet(HashSet::with_hasher(S::default()))
    }

    pub fn with_capacity(capacity: usize) -> Self {
        AHashSet(HashSet::with_capacity_and_hasher(capacity, S::default()))
    }
}

impl<T, S> AHashSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher,
{
    pub fn with_hasher(hash_builder: S) -> Self {
        AHashSet(HashSet::with_hasher(hash_builder))
    }

    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        AHashSet(HashSet::with_capacity_and_hasher(capacity, hash_builder))
    }
}

impl<T, S> Deref for AHashSet<T, S> {
    type Target = HashSet<T, S>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, S> DerefMut for AHashSet<T, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T, S> PartialEq for AHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    fn eq(&self, other: &AHashSet<T, S>) -> bool {
        self.0.eq(&other.0)
    }
}

impl<T, S> Eq for AHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
}

impl<T, S> BitOr<&AHashSet<T, S>> for &AHashSet<T, S>
where
    T: Eq + Hash + Clone,
    S: BuildHasher + Default,
{
    type Output = AHashSet<T, S>;

    /// Returns the union of `self` and `rhs` as a new `AHashSet<T, S>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ahash::AHashSet;
    ///
    /// let a: AHashSet<_> = vec![1, 2, 3].into_iter().collect();
    /// let b: AHashSet<_> = vec![3, 4, 5].into_iter().collect();
    ///
    /// let set = &a | &b;
    ///
    /// let mut i = 0;
    /// let expected = [1, 2, 3, 4, 5];
    /// for x in &set {
    ///     assert!(expected.contains(x));
    ///     i += 1;
    /// }
    /// assert_eq!(i, expected.len());
    /// ```
    fn bitor(self, rhs: &AHashSet<T, S>) -> AHashSet<T, S> {
        AHashSet(self.0.bitor(&rhs.0))
    }
}

impl<T, S> BitAnd<&AHashSet<T, S>> for &AHashSet<T, S>
where
    T: Eq + Hash + Clone,
    S: BuildHasher + Default,
{
    type Output = AHashSet<T, S>;

    /// Returns the intersection of `self` and `rhs` as a new `AHashSet<T, S>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ahash::AHashSet;
    ///
    /// let a: AHashSet<_> = vec![1, 2, 3].into_iter().collect();
    /// let b: AHashSet<_> = vec![2, 3, 4].into_iter().collect();
    ///
    /// let set = &a & &b;
    ///
    /// let mut i = 0;
    /// let expected = [2, 3];
    /// for x in &set {
    ///     assert!(expected.contains(x));
    ///     i += 1;
    /// }
    /// assert_eq!(i, expected.len());
    /// ```
    fn bitand(self, rhs: &AHashSet<T, S>) -> AHashSet<T, S> {
        AHashSet(self.0.bitand(&rhs.0))
    }
}

impl<T, S> BitXor<&AHashSet<T, S>> for &AHashSet<T, S>
where
    T: Eq + Hash + Clone,
    S: BuildHasher + Default,
{
    type Output = AHashSet<T, S>;

    /// Returns the symmetric difference of `self` and `rhs` as a new `AHashSet<T, S>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ahash::AHashSet;
    ///
    /// let a: AHashSet<_> = vec![1, 2, 3].into_iter().collect();
    /// let b: AHashSet<_> = vec![3, 4, 5].into_iter().collect();
    ///
    /// let set = &a ^ &b;
    ///
    /// let mut i = 0;
    /// let expected = [1, 2, 4, 5];
    /// for x in &set {
    ///     assert!(expected.contains(x));
    ///     i += 1;
    /// }
    /// assert_eq!(i, expected.len());
    /// ```
    fn bitxor(self, rhs: &AHashSet<T, S>) -> AHashSet<T, S> {
        AHashSet(self.0.bitxor(&rhs.0))
    }
}

impl<T, S> Sub<&AHashSet<T, S>> for &AHashSet<T, S>
where
    T: Eq + Hash + Clone,
    S: BuildHasher + Default,
{
    type Output = AHashSet<T, S>;

    /// Returns the difference of `self` and `rhs` as a new `AHashSet<T, S>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ahash::AHashSet;
    ///
    /// let a: AHashSet<_> = vec![1, 2, 3].into_iter().collect();
    /// let b: AHashSet<_> = vec![3, 4, 5].into_iter().collect();
    ///
    /// let set = &a - &b;
    ///
    /// let mut i = 0;
    /// let expected = [1, 2];
    /// for x in &set {
    ///     assert!(expected.contains(x));
    ///     i += 1;
    /// }
    /// assert_eq!(i, expected.len());
    /// ```
    fn sub(self, rhs: &AHashSet<T, S>) -> AHashSet<T, S> {
        AHashSet(self.0.sub(&rhs.0))
    }
}

impl<T, S> Debug for AHashSet<T, S>
where
    T: Eq + Hash + Debug,
    S: BuildHasher,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(fmt)
    }
}

impl<T, S> FromIterator<T> for AHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher + Default,
{
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> AHashSet<T, S> {
        AHashSet(HashSet::from_iter(iter))
    }
}

impl<'a, T, S> IntoIterator for &'a AHashSet<T, S> {
    type Item = &'a T;
    type IntoIter = hash_set::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        (&self.0).iter()
    }
}

impl<T, S> IntoIterator for AHashSet<T, S> {
    type Item = T;
    type IntoIter = hash_set::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T, S> Extend<T> for AHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher,
{
    #[inline]
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.0.extend(iter)
    }
}

impl<'a, T, S> Extend<&'a T> for AHashSet<T, S>
where
    T: 'a + Eq + Hash + Copy,
    S: BuildHasher,
{
    #[inline]
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        self.0.extend(iter)
    }
}

impl<T, S> Default for AHashSet<T, S>
where
    T: Eq + Hash,
    S: BuildHasher + Default,
{
    /// Creates an empty `AHashSet<T, S>` with the `Default` value for the hasher.
    #[inline]
    fn default() -> AHashSet<T, S> {
        AHashSet(HashSet::default())
    }
}
