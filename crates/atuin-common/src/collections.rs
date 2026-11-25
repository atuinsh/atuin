//! Collections that the standard library lacks.

use std::borrow::Borrow;
use std::collections::{HashMap, hash_map};
use std::hash::Hash;

/// A hash map that forgets every entry once it holds `capacity` entries and a new key arrives.
///
/// It bounds the memory of a cache whose keys are never reported as gone, such as the handles
/// of windows that closed. Forgetting everything at once costs each key that comes back one
/// more miss, and needs no bookkeeping per entry.
#[derive(Debug, Clone)]
pub struct ForgetfulMap<K, V> {
    entries: HashMap<K, V>,
    capacity: usize,
}

impl<K: Eq + Hash, V> ForgetfulMap<K, V> {
    /// An empty map that holds `capacity` entries at most.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.get(key)
    }

    /// Returns the stored key that equals `key`, with its value.
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.get_key_value(key)
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.get_mut(key)
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.contains_key(key)
    }

    /// Inserts `value` under `key`, and returns the value that `key` had.
    ///
    /// A new key forgets every other entry first when the map is full.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.make_room_for(&key);
        self.entries.insert(key, value)
    }

    /// Returns the value of `key`, which `value` makes when `key` has none.
    ///
    /// A new key forgets every other entry first when the map is full.
    pub fn get_or_insert_with(&mut self, key: K, value: impl FnOnce() -> V) -> &mut V {
        self.make_room_for(&key);
        self.entries.entry(key).or_insert_with(value)
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.entries.remove(key)
    }

    /// Forgets every entry, and keeps the memory that the entries took.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[must_use]
    pub fn iter(&self) -> hash_map::Iter<'_, K, V> {
        self.entries.iter()
    }

    fn make_room_for(&mut self, key: &K) {
        if self.entries.len() >= self.capacity && !self.entries.contains_key(key) {
            self.entries.clear();
        }
    }
}

/// A map of counters. Looking up a key that the map already holds doesn't allocate.
///
/// The values can be counters themselves, so that a tally counts by several keys at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally<K: Eq + Hash, V>(HashMap<K, V>);

impl<K: Eq + Hash, V> Default for Tally<K, V> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

impl<K: Eq + Hash, V: Default> Tally<K, V> {
    /// Returns the value for `key`, inserting a default value when `key` is new.
    pub fn slot<Q>(&mut self, key: &Q) -> &mut V
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ToOwned<Owned = K> + ?Sized,
    {
        // Returning the reference from `get_mut` and inserting after a miss would need one
        // lookup less, but the borrow checker rejects it, because the borrow of a returned
        // reference lasts to the end of the function on every path. `entry` would allocate
        // the owned key for every call.
        if self.0.contains_key(key) {
            return self.0.get_mut(key).expect("the key is present");
        }
        self.0.entry(key.to_owned()).or_default()
    }
}

impl<K: Eq + Hash, V> IntoIterator for Tally<K, V> {
    type IntoIter = hash_map::IntoIter<K, V>;
    type Item = (K, V);

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn a_new_key_in_a_full_map_forgets_the_others() {
        let mut map = ForgetfulMap::new(2);
        map.insert("a", 1);
        map.insert("b", 2);
        assert_eq!(map.insert("c", 3), None);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a"), None);
        assert_eq!(map.get("c"), Some(&3));
    }

    #[rstest]
    fn a_known_key_in_a_full_map_keeps_the_others() {
        let mut map = ForgetfulMap::new(2);
        map.insert("a", 1);
        map.insert("b", 2);
        assert_eq!(map.insert("a", 10), Some(1));
        *map.get_or_insert_with("b", || 0) += 1;
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("a"), Some(&10));
        assert_eq!(map.get("b"), Some(&3));
    }

    #[rstest]
    fn clear_forgets_every_entry() {
        let mut map = ForgetfulMap::new(2);
        map.insert("a", 1);
        map.insert("b", 2);
        map.clear();
        assert!(map.is_empty());
        map.insert("c", 3);
        map.insert("d", 4);
        assert_eq!(map.len(), 2);
    }

    #[rstest]
    fn get_or_insert_with_makes_a_value_only_for_a_new_key() {
        let mut map = ForgetfulMap::new(1);
        assert_eq!(*map.get_or_insert_with("a", || 1), 1);
        assert_eq!(*map.get_or_insert_with("a", || 2), 1);
        assert_eq!(*map.get_or_insert_with("b", || 3), 3);
        assert!(!map.contains_key("a"));
    }

    #[rstest]
    fn nested_slots_accumulate() {
        let mut tally: Tally<String, Tally<String, u64>> = Tally::default();
        *tally.slot("app").slot("focus") += 1;
        *tally.slot("app").slot("focus") += 2;
        *tally.slot("app").slot("text") += 1;

        let mut counts: Vec<_> = tally.into_iter().flat_map(|(_, inner)| inner).collect();
        counts.sort();
        assert_eq!(counts, [("focus".to_owned(), 3), ("text".to_owned(), 1)]);
    }
}
