#![feature(allocator_api)]
#![cfg(feature = "allocator_api")]
use bumpalo::Bump;

use std::alloc::{AllocError, Allocator, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

#[derive(Debug)]
struct AllocatorDebug {
    bump: Bump,
    grows: AtomicUsize,
    shrinks: AtomicUsize,
    allocs: AtomicUsize,
    deallocs: AtomicUsize,
}

impl AllocatorDebug {
    fn new(bump: Bump) -> AllocatorDebug {
        AllocatorDebug {
            bump,
            grows: AtomicUsize::new(0),
            shrinks: AtomicUsize::new(0),
            allocs: AtomicUsize::new(0),
            deallocs: AtomicUsize::new(0),
        }
    }
}

unsafe impl Allocator for AllocatorDebug {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.allocs.fetch_add(1, Relaxed);
        let ref bump = self.bump;
        bump.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.deallocs.fetch_add(1, Relaxed);
        let ref bump = self.bump;
        bump.deallocate(ptr, layout)
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.shrinks.fetch_add(1, Relaxed);
        let ref bump = self.bump;
        bump.shrink(ptr, old_layout, new_layout)
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        self.grows.fetch_add(1, Relaxed);
        let ref bump = self.bump;
        bump.grow(ptr, old_layout, new_layout)
    }
}

#[test]
fn allocator_api_push_a_bunch_of_items() {
    let b = AllocatorDebug::new(Bump::new());
    let mut v = Vec::with_capacity_in(1024, &b);
    assert_eq!(b.allocs.load(Relaxed), 1);

    for x in 0..1024 {
        v.push(x);
    }

    // Ensure we trigger a grow
    assert_eq!(b.grows.load(Relaxed), 0);
    for x in 1024..2048 {
        v.push(x);
    }
    assert_ne!(b.grows.load(Relaxed), 0);

    // Ensure we trigger a shrink
    v.truncate(1024);
    v.shrink_to_fit();
    assert_eq!(b.shrinks.load(Relaxed), 1);

    // Ensure we trigger a deallocation
    assert_eq!(b.deallocs.load(Relaxed), 0);
    drop(v);
    assert_eq!(b.deallocs.load(Relaxed), 1);
}

#[test]
fn allocator_grow_zeroed() {
    // Create a new bump arena.
    let ref bump = Bump::new();

    // Make an initial allocation.
    let first_layout = Layout::from_size_align(4, 4).expect("create a layout");
    let mut p = bump
        .allocate_zeroed(first_layout)
        .expect("allocate a first chunk");
    let allocated = bump.allocated_bytes();
    unsafe { p.as_mut().fill(42) };
    let p = p.cast();

    // Grow the last allocation. This should just reserve a few more bytes
    // within the current chunk, not allocate a whole new memory block within a
    // new chunk.
    let second_layout = Layout::from_size_align(8, 4).expect("create a expanded layout");
    let p = unsafe { bump.grow_zeroed(p, first_layout, second_layout) }
        .expect("should grow_zeroed okay");
    assert!(bump.allocated_bytes() <= allocated * 2);
    assert_eq!(unsafe { p.as_ref() }, [42, 42, 42, 42, 0, 0, 0, 0]);
}
