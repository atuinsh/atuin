use std::borrow::Borrow;
use std::collections::{hash_map, HashMap};
use std::fmt::{self, Debug};
use std::hash::{BuildHasher, Hash};
use std::iter::FromIterator;
use std::ops::{Deref, DerefMut, Index};
use std::panic::UnwindSafe;

/// A [`HashMap`](std::collections::HashMap) using [`RandomState`](crate::RandomState) to hash the items.
/// Requires the `std` feature to be enabled.
#[derive(Clone)]
pub struct AHashMap<K, V, S = crate::RandomState>(HashMap<K, V, S>);

impl<K, V, S> AHashMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher + Default,
{
    pub fn new() -> Self {
        AHashMap(HashMap::with_hasher(S::default()))
    }

    pub fn with_capacity(capacity: usize) -> Self {
        AHashMap(HashMap::with_capacity_and_hasher(capacity, S::default()))
    }
}

impl<K, V, S> AHashMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    pub fn with_hasher(hash_builder: S) -> Self {
        AHashMap(HashMap::with_hasher(hash_builder))
    }

    pub fn with_capacity_and_hasher(capacity: usize, hash_builder: S) -> Self {
        AHashMap(HashMap::with_capacity_and_hasher(capacity, hash_builder))
    }
}

impl<K, V, S> Deref for AHashMap<K, V, S> {
    type Target = HashMap<K, V, S>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V, S> DerefMut for AHashMap<K, V, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<K, V, S> UnwindSafe for AHashMap<K, V, S>
where
    K: UnwindSafe,
    V: UnwindSafe,
{
}

impl<K, V, S> PartialEq for AHashMap<K, V, S>
where
    K: Eq + Hash,
    V: PartialEq,
    S: BuildHasher,
{
    fn eq(&self, other: &AHashMap<K, V, S>) -> bool {
        self.0.eq(&other.0)
    }
}

impl<K, V, S> Eq for AHashMap<K, V, S>
where
    K: Eq + Hash,
    V: Eq,
    S: BuildHasher,
{
}

impl<K, Q: ?Sized, V, S> Index<&Q> for AHashMap<K, V, S>
where
    K: Eq + Hash + Borrow<Q>,
    Q: Eq + Hash,
    S: BuildHasher,
{
    type Output = V;

    /// Returns a reference to the value corresponding to the supplied key.
    ///
    /// # Panics
    ///
    /// Panics if the key is not present in the `HashMap`.
    #[inline]
    fn index(&self, key: &Q) -> &V {
        self.0.index(key)
    }
}

impl<K, V, S> Debug for AHashMap<K, V, S>
where
    K: Eq + Hash + Debug,
    V: Debug,
    S: BuildHasher,
{
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(fmt)
    }
}

impl<K, V, S> FromIterator<(K, V)> for AHashMap<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher + Default,
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        AHashMap(HashMap::from_iter(iter))
    }
}

impl<'a, K, V, S> IntoIterator for &'a AHashMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = hash_map::Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        (&self.0).iter()
    }
}

impl<'a, K, V, S> IntoIterator for &'a mut AHashMap<K, V, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = hash_map::IterMut<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        (&mut self.0).iter_mut()
    }
}

impl<K, V, S> IntoIterator for AHashMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = hash_map::IntoIter<K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<K, V, S> Extend<(K, V)> for AHashMap<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    #[inline]
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        self.0.extend(iter)
    }
}

impl<'a, K, V, S> Extend<(&'a K, &'a V)> for AHashMap<K, V, S>
where
    K: Eq + Hash + Copy + 'a,
    V: Copy + 'a,
    S: BuildHasher,
{
    #[inline]
    fn extend<T: IntoIterator<Item = (&'a K, &'a V)>>(&mut self, iter: T) {
        self.0.extend(iter)
    }
}

impl<K, V, S> Default for AHashMap<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher + Default,
{
    #[inline]
    fn default() -> AHashMap<K, V, S> {
        AHashMap::with_hasher(Default::default())
    }
}
