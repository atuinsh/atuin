#![cfg(feature = "heapsize_impl")]

extern crate heapsize;
extern crate linked_hash_map;

use linked_hash_map::LinkedHashMap;
use heapsize::HeapSizeOf;

#[test]
fn empty() {
    assert_eq!(LinkedHashMap::<String, String>::new().heap_size_of_children(), 0);
}

#[test]
fn nonempty() {
    let mut map = LinkedHashMap::new();
    map.insert("hello".to_string(), "world".to_string());
    map.insert("hola".to_string(), "mundo".to_string());
    map.insert("bonjour".to_string(), "monde".to_string());
    map.remove("hello");

    assert!(map.heap_size_of_children() != 0);
}
