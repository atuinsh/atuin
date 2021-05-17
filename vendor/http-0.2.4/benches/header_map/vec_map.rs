#![allow(dead_code)]

#[derive(Clone)]
pub struct VecMap<K, V> {
    vec: Vec<(K, V)>,
}

impl<K: PartialEq, V> VecMap<K, V> {
    #[inline]
    pub fn with_capacity(cap: usize) -> VecMap<K, V> {
        VecMap {
            vec: Vec::with_capacity(cap),
        }
    }

    #[inline]
    pub fn insert(&mut self, key: K, value: V) {
        match self.find(&key) {
            Some(pos) => self.vec[pos] = (key, value),
            None => self.vec.push((key, value)),
        }
    }

    #[inline]
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        match self.find(&key) {
            Some(pos) => Entry::Occupied(OccupiedEntry {
                vec: self,
                pos: pos,
            }),
            None => Entry::Vacant(VacantEntry {
                vec: self,
                key: key,
            }),
        }
    }

    #[inline]
    pub fn get<K2: PartialEq<K> + ?Sized>(&self, key: &K2) -> Option<&V> {
        self.find(key).map(move |pos| &self.vec[pos].1)
    }

    #[inline]
    pub fn get_mut<K2: PartialEq<K> + ?Sized>(&mut self, key: &K2) -> Option<&mut V> {
        self.find(key).map(move |pos| &mut self.vec[pos].1)
    }

    #[inline]
    pub fn contains_key<K2: PartialEq<K> + ?Sized>(&self, key: &K2) -> bool {
        self.find(key).is_some()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    #[inline]
    pub fn iter(&self) -> ::std::slice::Iter<'_, (K, V)> {
        self.vec.iter()
    }
    #[inline]
    pub fn remove<K2: PartialEq<K> + ?Sized>(&mut self, key: &K2) -> Option<V> {
        self.find(key)
            .map(|pos| self.vec.remove(pos))
            .map(|(_, v)| v)
    }
    #[inline]
    pub fn clear(&mut self) {
        self.vec.clear();
    }

    #[inline]
    fn find<K2: PartialEq<K> + ?Sized>(&self, key: &K2) -> Option<usize> {
        self.vec.iter().position(|entry| key == &entry.0)
    }
}

pub enum Entry<'a, K: 'a, V: 'a> {
    Vacant(VacantEntry<'a, K, V>),
    Occupied(OccupiedEntry<'a, K, V>),
}

pub struct VacantEntry<'a, K, V> {
    vec: &'a mut VecMap<K, V>,
    key: K,
}

impl<'a, K, V> VacantEntry<'a, K, V> {
    pub fn insert(self, val: V) -> &'a mut V {
        let vec = self.vec;
        vec.vec.push((self.key, val));
        let pos = vec.vec.len() - 1;
        &mut vec.vec[pos].1
    }
}

pub struct OccupiedEntry<'a, K, V> {
    vec: &'a mut VecMap<K, V>,
    pos: usize,
}

impl<'a, K, V> OccupiedEntry<'a, K, V> {
    pub fn into_mut(self) -> &'a mut V {
        &mut self.vec.vec[self.pos].1
    }
}
