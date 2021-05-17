// The code in this file is based on Rust's compiler-builtins crate. The Rust
// compiler automatically links programs against this crate for target-specific
// runtime support. We have copied the implementation of `__udivmodti4()` which
// is an intrinsic implementing division with remainder for architectures
// without 128-bit integers. This implementation works around some poor codegen
// by LLVM (https://github.com/rust-lang/rust/issues/44545) and allows for
// inlining which does not happen with the intrinsic.
//
// The compiler-builtins crate carries the following license, which is available
// in full at:
// https://github.com/rust-lang-nursery/compiler-builtins/blob/master/LICENSE.TXT
//
// ---
//
// Copyright 2009-2016 compiler-builtins Developers
//
// The compiler-builtins crate is dual licensed under both the University of
// Illinois "BSD-Like" license and the MIT license. As a user of this code you
// may choose to use it under either license. As a contributor, you agree to
// allow your code to be used under both.

#[inline]
pub fn udivmod_1e19(n: u128) -> (u128, u64) {
    let d = 10_000_000_000_000_000_000_u64; // 10^19

    let high = (n >> 64) as u64;
    if high == 0 {
        let low = n as u64;
        return ((low / d) as u128, low % d);
    }

    let sr = 65 - high.leading_zeros();

    // 2 <= sr <= 65
    let mut q: u128 = n << (128 - sr);
    let mut r: u128 = n >> sr;
    let mut carry: u64 = 0;

    // Don't use a range because they may generate references to memcpy in unoptimized code
    //
    // Loop invariants:  r < d; carry is 0 or 1
    let mut i = 0;
    while i < sr {
        i += 1;

        // r:q = ((r:q) << 1) | carry
        r = (r << 1) | (q >> 127);
        q = (q << 1) | carry as u128;

        // carry = 0
        // if r >= d {
        //     r -= d;
        //     carry = 1;
        // }
        let s = (d as u128).wrapping_sub(r).wrapping_sub(1) as i128 >> 127;
        carry = (s & 1) as u64;
        r -= (d as u128) & s as u128;
    }

    ((q << 1) | carry as u128, r as u64)
}
