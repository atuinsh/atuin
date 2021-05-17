//! An optional implementation of serialization/deserialization.

extern crate serde;

use std::fmt::{Formatter, Result as FmtResult};
use std::marker::PhantomData;
use std::hash::{BuildHasher, Hash};

use super::LinkedHashMap;

use self::serde::{Serialize, Serializer, Deserialize, Deserializer};
use self::serde::ser::SerializeMap;
use self::serde::de::{Visitor, MapAccess, Error};

impl<K, V, S> Serialize for LinkedHashMap<K, V, S>
    where K: Serialize + Eq + Hash,
          V: Serialize,
          S: BuildHasher
{
    #[inline]
    fn serialize<T>(&self, serializer:T) -> Result<T::Ok, T::Error>
        where T: Serializer,
    {
        let mut map_serializer = try!(serializer.serialize_map(Some(self.len())));
        for (k, v) in self {
            try!(map_serializer.serialize_key(k));
            try!(map_serializer.serialize_value(v));
        }
        map_serializer.end()
    }
}

#[derive(Debug)]
/// `serde::de::Visitor` for a linked hash map.
pub struct LinkedHashMapVisitor<K, V> {
    marker: PhantomData<LinkedHashMap<K, V>>,
}

impl<K, V> LinkedHashMapVisitor<K, V> {
    /// Creates a new visitor for a linked hash map.
    pub fn new() -> Self {
        LinkedHashMapVisitor {
            marker: PhantomData,
        }
    }
}

impl<K, V> Default for LinkedHashMapVisitor<K, V> {
    fn default() -> Self {
        LinkedHashMapVisitor::new()
    }
}

impl<'de, K, V> Visitor<'de> for LinkedHashMapVisitor<K, V>
    where K: Deserialize<'de> + Eq + Hash,
          V: Deserialize<'de>,
{
    type Value = LinkedHashMap<K, V>;

    fn expecting(&self, formatter: &mut Formatter) -> FmtResult {
        write!(formatter, "a map")
    }

    #[inline]
    fn visit_unit<E>(self) -> Result<Self::Value, E>
        where E: Error,
    {
        Ok(LinkedHashMap::new())
    }

    #[inline]
    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where M: MapAccess<'de>,
    {
        let mut values = LinkedHashMap::with_capacity(map.size_hint().unwrap_or(0));

        while let Some((key, value)) = map.next_entry()? {
            values.insert(key, value);
        }

        Ok(values)
    }
}

impl<'de, K, V> Deserialize<'de> for LinkedHashMap<K, V>
    where K: Deserialize<'de> + Eq + Hash,
          V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<LinkedHashMap<K, V>, D::Error>
        where D: Deserializer<'de>,
    {
        deserializer.deserialize_map(LinkedHashMapVisitor::new())
    }
}
