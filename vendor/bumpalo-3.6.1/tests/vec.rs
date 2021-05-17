#![cfg(feature = "collections")]
use bumpalo::{collections::Vec, vec, Bump};
use std::cell::Cell;

#[test]
fn push_a_bunch_of_items() {
    let b = Bump::new();
    let mut v = Vec::new_in(&b);
    for x in 0..10_000 {
        v.push(x);
    }
}

#[test]
fn trailing_comma_in_vec_macro() {
    let b = Bump::new();
    let v = vec![in &b; 1, 2, 3,];
    assert_eq!(v, [1, 2, 3]);
}

#[test]
fn recursive_vecs() {
    // The purpose of this test is to see if the data structures with
    // self references are allowed without causing a compile error
    // because of the dropck
    let b = Bump::new();

    struct Node<'a> {
        myself: Cell<Option<&'a Node<'a>>>,
        edges: Cell<Vec<'a, &'a Node<'a>>>,
    }

    let node1: &Node = b.alloc(Node {
        myself: Cell::new(None),
        edges: Cell::new(Vec::new_in(&b)),
    });
    let node2: &Node = b.alloc(Node {
        myself: Cell::new(None),
        edges: Cell::new(Vec::new_in(&b)),
    });

    node1.myself.set(Some(node1));
    node1.edges.set(bumpalo::vec![in &b; node1, node1, node2]);

    node2.myself.set(Some(node2));
    node2.edges.set(bumpalo::vec![in &b; node1, node2]);
}

#[test]
fn test_into_bump_slice_mut() {
    let b = Bump::new();
    let v = bumpalo::vec![in &b; 1, 2, 3];
    let slice = v.into_bump_slice_mut();

    slice[0] = 3;
    slice[2] = 1;

    assert_eq!(slice, [3, 2, 1]);
}

quickcheck::quickcheck! {
    fn vec_resizes_causing_reallocs(sizes: std::vec::Vec<usize>) -> () {
        // Exercise `realloc` by doing a bunch of `resize`s followed by
        // `shrink_to_fit`s.

        let b = Bump::new();
        let mut v = bumpalo::vec![in &b];

        for len in sizes {
            // We don't want to get too big and OOM.
            const MAX_SIZE: usize = 1 << 15;

            // But we want allocations to get fairly close to the minimum chunk
            // size, so that we are exercising both realloc'ing within a chunk
            // and when we need new chunks.
            const MIN_SIZE: usize = 1 << 7;

            let len = std::cmp::min(len, MAX_SIZE);
            let len = std::cmp::max(len, MIN_SIZE);

            v.resize(len, 0);
            v.shrink_to_fit();
        }
    }
}
