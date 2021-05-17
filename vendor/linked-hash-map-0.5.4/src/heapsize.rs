extern crate heapsize;

use self::heapsize::{HeapSizeOf, heap_size_of};
use std::hash::{Hash, BuildHasher};

use {LinkedHashMap, KeyRef, Node};

impl<K> HeapSizeOf for KeyRef<K> {
    fn heap_size_of_children(&self) -> usize {
        0
    }
}

impl<K, V> HeapSizeOf for Node<K, V>
    where K: HeapSizeOf,
          V: HeapSizeOf
{
    fn heap_size_of_children(&self) -> usize {
        self.key.heap_size_of_children() + self.value.heap_size_of_children()
    }
}

impl<K, V, S> HeapSizeOf for LinkedHashMap<K, V, S>
    where K: HeapSizeOf + Hash + Eq,
          V: HeapSizeOf,
          S: BuildHasher
{
    fn heap_size_of_children(&self) -> usize {
        unsafe {
            let mut size = self.map.heap_size_of_children();
            for &value in self.map.values() {
                size += (*value).heap_size_of_children();
                size += heap_size_of(value as *const _ as *const _);
            }

            if !self.head.is_null() {
                size += heap_size_of(self.head as *const _ as *const _);
            }

            let mut free = self.free;
            while !free.is_null() {
                size += heap_size_of(free as *const _ as *const _);
                free = (*free).next
            }

            size
        }
    }
}
