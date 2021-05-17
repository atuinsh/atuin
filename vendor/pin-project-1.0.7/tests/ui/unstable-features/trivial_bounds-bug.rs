// Note: If you change this test, change 'trivial_bounds-feature-gate.rs' at the same time.

// trivial_bounds
// Tracking issue: https://github.com/rust-lang/rust/issues/48214
#![feature(trivial_bounds)]

mod phantom_pinned {
    use std::marker::{PhantomData, PhantomPinned};

    struct A(PhantomPinned);

    // bug of trivial_bounds?
    impl Unpin for A where PhantomPinned: Unpin {} //~ ERROR E0277

    struct Wrapper<T>(T);

    impl<T> Unpin for Wrapper<T> where T: Unpin {}

    struct B(PhantomPinned);

    impl Unpin for B where Wrapper<PhantomPinned>: Unpin {} // Ok

    struct WrapperWithLifetime<'a, T>(PhantomData<&'a ()>, T);

    impl<T> Unpin for WrapperWithLifetime<'_, T> where T: Unpin {}

    struct C(PhantomPinned);

    // Ok
    impl<'a> Unpin for C where WrapperWithLifetime<'a, PhantomPinned>: Unpin {}
}

fn main() {}
