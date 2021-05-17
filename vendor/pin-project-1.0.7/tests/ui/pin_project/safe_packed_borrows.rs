#![deny(renamed_and_removed_lints)]
#![deny(safe_packed_borrows)] //~ ERROR has been renamed to `unaligned_references`
#![allow(unaligned_references)]

// This lint was removed in https://github.com/rust-lang/rust/pull/82525 (nightly-2021-03-28).
// Refs:
// - https://github.com/rust-lang/rust/pull/82525
// - https://github.com/rust-lang/rust/issues/46043

#[repr(packed)]
struct Packed {
    f: u32,
}

#[repr(packed(2))]
struct PackedN {
    f: u32,
}

fn main() {
    let a = Packed { f: 1 };
    &a.f;
    let _ = &a.f;

    let b = PackedN { f: 1 };
    &b.f;
    let _ = &b.f;
}
