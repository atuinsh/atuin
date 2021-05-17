//! Rust cast utilities.

// We have a lot of high-level casts that make the type-system work.
// Don't delete them, fake they're being used.
#![allow(dead_code)]

use super::primitive::AsPrimitive;
use super::num::{Integer};

// AS CAST

/// Allows the high-level conversion of generic types as if `as` was used.
#[inline]
pub(crate) fn as_cast<U: AsCast, T: AsCast>(t: T) -> U {
    AsCast::as_cast(t)
}

/// An interface for casting between machine scalars.
pub trait AsCast: AsPrimitive {
    /// Creates a number from another value that can be converted into
    /// a primitive via the `AsPrimitive` trait.
    fn as_cast<N: AsPrimitive>(n: N) -> Self;
}

macro_rules! as_cast {
    ($t:ty, $meth:ident) => {
        impl AsCast for $t {
            #[inline]
            fn as_cast<N: AsPrimitive>(n: N) -> $t {
                n.$meth()
            }
        }
    };
}

as_cast!(u8, as_u8);
as_cast!(u16, as_u16);
as_cast!(u32, as_u32);
as_cast!(u64, as_u64);
as_cast!(u128, as_u128);
as_cast!(usize, as_usize);
as_cast!(i8, as_i8);
as_cast!(i16, as_i16);
as_cast!(i32, as_i32);
as_cast!(i64, as_i64);
as_cast!(i128, as_i128);
as_cast!(isize, as_isize);
as_cast!(f32, as_f32);
as_cast!(f64, as_f64);

// TRY CAST
// Analogous to TryInto.

/// High-level conversion of types using TryCast.
#[inline]
pub(crate) fn try_cast<U, T: TryCast<U>>(t: T) -> Option<U> {
    TryCast::try_cast(t)
}

/// Non-lossy cast between types. Returns None if new type cannot represent value.
pub trait TryCast<T>: Sized {
    /// Consume self and return the cast value (or None).
    fn try_cast(self) -> Option<T>;
}

macro_rules! try_cast {
    // CHECK

    // Checked conversion
    (@check $v:ident, $cond:expr) => (if $cond { Some(as_cast($v)) } else { None });

    // INTEGER

    // Widen type,so no checks required, both are signed/unsigned.
    (@widen $src:tt, $($dst:tt),*) => ($(
        impl TryCast<$dst> for $src {
            #[inline]
            fn try_cast(self) -> Option<$dst> {
                try_cast!(@check self, true)
            }
        }
    )*);

    // Above zero check, for a signed to unsigned conversion of same width.
    (@positive $src:tt, $($dst:tt),*) => ($(
        impl TryCast<$dst> for $src {
            #[inline]
            fn try_cast(self) -> Option<$dst> {
                try_cast!(@check self, self >= 0)
            }
        }
    )*);

    // Check below some upper bound (for narrowing of an unsigned value).
    (@below $src:tt, $($dst:tt),*) => ($(
        impl TryCast<$dst> for $src {
            #[inline]
            fn try_cast(self) -> Option<$dst> {
                const MAX: $src = $dst::max_value() as $src;
                try_cast!(@check self, self <= MAX)
            }
        }
    )*);

    // Check within min and max bounds (for narrowing of a signed value).
    (@within $src:tt, $($dst:tt),*) => ($(
        impl TryCast<$dst> for $src {
            #[inline]
            fn try_cast(self) -> Option<$dst> {
                const MIN: $src = $dst::min_value() as $src;
                const MAX: $src = $dst::max_value() as $src;
                try_cast!(@check self, self >= MIN && self <= MAX)
            }
        }
    )*);

    // FLOAT

    // Cannot be done without implementation defined behavior.
    (@into_float $src:tt, $($dst:tt),*) => ($(
        impl TryCast<$dst> for $src {
            #[inline]
            fn try_cast(self) -> Option<$dst> {
                unreachable!()
            }
        }
    )*);

    // Cannot be done without implementation defined behavior.
    (@from_float $src:tt, $($dst:tt),*) => ($(
        impl TryCast<$dst> for $src {
            #[inline]
            fn try_cast(self) -> Option<$dst> {
                unreachable!()
            }
        }
    )*);
}

// u8
try_cast! { @widen u8, u8, u16, u32, u64, u128 }
try_cast! { @below u8, i8 }
try_cast! { @widen u8, i16, i32, i64, i128 }
try_cast! { @widen u8, f32, f64 }

// u16
try_cast! { @below u16, u8 }
try_cast! { @widen u16, u16, u32, u64, u128 }
try_cast! { @below u16, i8, i16 }
try_cast! { @widen u16, i32, i64, i128 }
try_cast! { @widen u16, f32, f64 }

// u32
try_cast! { @below u32, u8, u16 }
try_cast! { @widen u32, u32, u64, u128 }
try_cast! { @below u32, i8, i16, i32 }
try_cast! { @widen u32, i64, i128 }
try_cast! { @into_float u32, f32 }
try_cast! { @widen u32, f64 }

// u64
try_cast! { @below u64, u8, u16, u32 }
try_cast! { @widen u64, u64, u128 }
try_cast! { @below u64, i8, i16, i32, i64 }
try_cast! { @widen u64, i128}
try_cast! { @into_float u64, f32, f64 }

// u128
try_cast! { @below u128, u8, u16, u32, u64 }
try_cast! { @widen u128, u128 }
try_cast! { @below u128, i8, i16, i32, i64, i128 }
try_cast! { @into_float u128, f32, f64 }

// i8
try_cast! { @positive i8, u8, u16, u32, u64, u128 }
try_cast! { @widen i8, i8, i16, i32, i64, i128 }
try_cast! { @widen i8, f32, f64 }

// i16
try_cast! { @within i16, u8 }
try_cast! { @positive i16, u16, u32, u64, u128 }
try_cast! { @within i16, i8 }
try_cast! { @widen i16, i16, i32, i64, i128 }
try_cast! { @widen i16, f32, f64 }

// i32
try_cast! { @within i32, u8, u16 }
try_cast! { @positive i32, u32, u64, u128 }
try_cast! { @within i32, i8, i16 }
try_cast! { @widen i32, i32, i64, i128 }
try_cast! { @into_float i32, f32 }
try_cast! { @widen i32, f64 }

// i64
try_cast! { @within i64, u8, u16, u32 }
try_cast! { @positive i64, u64, u128 }
try_cast! { @within i64, i8, i16, i32 }
try_cast! { @widen i64, i64, i128 }
try_cast! { @into_float i64, f32, f64 }

// i128
try_cast! { @within i128, u8, u16, u32, u64 }
try_cast! { @positive i128, u128 }
try_cast! { @within i128, i8, i16, i32, i64 }
try_cast! { @widen i128, i128 }
try_cast! { @into_float i128, f32, f64 }

// f32
try_cast! { @from_float f32, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64 }

// f64
try_cast! { @from_float f64, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64 }

// usize/isize shared
try_cast! { @from_float f32, usize }
try_cast! { @from_float f64, usize }
try_cast! { @from_float f32, isize }
try_cast! { @from_float f64, isize }

cfg_if! {
if #[cfg(target_pointer_width = "16")] {
    // 16-bit usize
    try_cast! { @below usize, u8 }
    try_cast! { @widen usize, u16, u32, u64, u128, usize }
    try_cast! { @below usize, i8, i16, isize }
    try_cast! { @widen usize, i32, i64, i128 }
    try_cast! { @widen usize, f32, f64 }
    try_cast! { @widen u8, usize }
    try_cast! { @widen u16, usize }
    try_cast! { @below u32, usize }
    try_cast! { @below u64, usize }
    try_cast! { @below u128, usize }
    try_cast! { @positive i8, usize }
    try_cast! { @positive i16, usize }
    try_cast! { @within i32, usize }
    try_cast! { @within i64, usize }
    try_cast! { @within i128, usize }

    // 16-bit isize
    try_cast! { @within isize, u8 }
    try_cast! { @positive isize, u16, u32, u64, u128, usize }
    try_cast! { @within isize, i8 }
    try_cast! { @widen isize, i16, i32, i64, i128, isize }
    try_cast! { @widen isize, f32, f64 }
    try_cast! { @widen u8, isize }
    try_cast! { @below u16, isize }
    try_cast! { @below u32, isize }
    try_cast! { @below u64, isize }
    try_cast! { @below u128, isize }
    try_cast! { @widen i8, isize }
    try_cast! { @widen i16, isize }
    try_cast! { @within i32, isize }
    try_cast! { @within i64, isize }
    try_cast! { @within i128, isize }
} else if #[cfg(target_pointer_width = "32")] {
    try_cast! { @below usize, u8, u16 }
    try_cast! { @widen usize, u32, u64, u128, usize }
    try_cast! { @below usize, i8, i16, i32, isize }
    try_cast! { @widen usize, i64, i128 }
    try_cast! { @into_float usize, f32 }
    try_cast! { @widen usize, f64 }
    try_cast! { @widen u8, usize }
    try_cast! { @widen u16, usize }
    try_cast! { @widen u32, usize }
    try_cast! { @below u64, usize }
    try_cast! { @below u128, usize }
    try_cast! { @positive i8, usize }
    try_cast! { @positive i16, usize }
    try_cast! { @positive i32, usize }
    try_cast! { @within i64, usize }
    try_cast! { @within i128, usize }

    // 32-bit isize
    try_cast! { @within isize, u8, u16 }
    try_cast! { @positive isize, u32, u64, u128, usize }
    try_cast! { @within isize, i8, i16 }
    try_cast! { @widen isize, i32, i64, i128, isize }
    try_cast! { @into_float isize, f32 }
    try_cast! { @widen isize, f64 }
    try_cast! { @widen u8, isize }
    try_cast! { @widen u16, isize }
    try_cast! { @below u32, isize }
    try_cast! { @below u64, isize }
    try_cast! { @below u128, isize }
    try_cast! { @widen i8, isize }
    try_cast! { @widen i16, isize }
    try_cast! { @widen i32, isize }
    try_cast! { @within i64, isize }
    try_cast! { @within i128, isize }
} else if #[cfg(target_pointer_width = "64")] {
    // 64-bit usize
    try_cast! { @below usize, u8, u16, u32 }
    try_cast! { @widen usize, u64, u128, usize }
    try_cast! { @below usize, i8, i16, i32, i64, isize }
    try_cast! { @widen usize, i128 }
    try_cast! { @into_float usize, f32, f64 }
    try_cast! { @widen u8, usize }
    try_cast! { @widen u16, usize }
    try_cast! { @widen u32, usize }
    try_cast! { @widen u64, usize }
    try_cast! { @below u128, usize }
    try_cast! { @positive i8, usize }
    try_cast! { @positive i16, usize }
    try_cast! { @positive i32, usize }
    try_cast! { @positive i64, usize }
    try_cast! { @within i128, usize }

    // 64-bit isize
    try_cast! { @within isize, u8, u16, u32 }
    try_cast! { @positive isize, u64, u128, usize }
    try_cast! { @within isize, i8, i16, i32 }
    try_cast! { @widen isize, i64, i128, isize }
    try_cast! { @into_float isize, f32, f64 }
    try_cast! { @widen u8, isize }
    try_cast! { @widen u16, isize }
    try_cast! { @widen u32, isize }
    try_cast! { @below u64, isize }
    try_cast! { @below u128, isize }
    try_cast! { @widen i8, isize }
    try_cast! { @widen i16, isize }
    try_cast! { @widen i32, isize }
    try_cast! { @widen i64, isize }
    try_cast! { @within i128, isize }
}}  // cfg_if

// TEST
// ----

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::num::{Integer, Float};

    fn check_as_cast<T: AsCast>(t: T) {
        let _: i8 = as_cast(t);
        let _: i16 = as_cast(t);
        let _: i32 = as_cast(t);
        let _: i64 = as_cast(t);
        let _: i128 = as_cast(t);
        let _: isize = as_cast(t);
        let _: u8 = as_cast(t);
        let _: u16 = as_cast(t);
        let _: u32 = as_cast(t);
        let _: u64 = as_cast(t);
        let _: u128 = as_cast(t);
        let _: usize = as_cast(t);
        let _: f32 = as_cast(t);
        let _: f64 = as_cast(t);
    }

    #[test]
    fn as_cast_test() {
        check_as_cast(1u8);
        check_as_cast(1u16);
        check_as_cast(1u32);
        check_as_cast(1u64);
        check_as_cast(1u128);
        check_as_cast(1usize);
        check_as_cast(1i8);
        check_as_cast(1i16);
        check_as_cast(1i32);
        check_as_cast(1i64);
        check_as_cast(1i128);
        check_as_cast(1isize);
        check_as_cast(1f32);
        check_as_cast(1f64);
    }

    fn try_cast_u8<T: TryCast<u8>>(t: T) -> Option<u8> {
        t.try_cast()
    }

    fn try_cast_u16<T: TryCast<u16>>(t: T) -> Option<u16> {
        t.try_cast()
    }

    fn try_cast_u32<T: TryCast<u32>>(t: T) -> Option<u32> {
        t.try_cast()
    }

    fn try_cast_u64<T: TryCast<u64>>(t: T) -> Option<u64> {
        t.try_cast()
    }

    fn try_cast_u128<T: TryCast<u128>>(t: T) -> Option<u128> {
        t.try_cast()
    }

    fn try_cast_usize<T: TryCast<usize>>(t: T) -> Option<usize> {
        t.try_cast()
    }

    fn try_cast_i8<T: TryCast<i8>>(t: T) -> Option<i8> {
        t.try_cast()
    }

    fn try_cast_i16<T: TryCast<i16>>(t: T) -> Option<i16> {
        t.try_cast()
    }

    fn try_cast_i32<T: TryCast<i32>>(t: T) -> Option<i32> {
        t.try_cast()
    }

    fn try_cast_i64<T: TryCast<i64>>(t: T) -> Option<i64> {
        t.try_cast()
    }

    fn try_cast_i128<T: TryCast<i128>>(t: T) -> Option<i128> {
        t.try_cast()
    }

    fn try_cast_isize<T: TryCast<isize>>(t: T) -> Option<isize> {
        t.try_cast()
    }

    fn try_cast_f32<T: TryCast<f32>>(t: T) -> Option<f32> {
        t.try_cast()
    }

    fn try_cast_f64<T: TryCast<f64>>(t: T) -> Option<f64> {
        t.try_cast()
    }

    #[test]
    fn try_cast_test() {
        // u8
        assert!(try_cast_u8(u8::min_value()).is_some());
        assert!(try_cast_u16(u8::min_value()).is_some());
        assert!(try_cast_u32(u8::min_value()).is_some());
        assert!(try_cast_u64(u8::min_value()).is_some());
        assert!(try_cast_u128(u8::min_value()).is_some());
        assert!(try_cast_i8(u8::min_value()).is_some());
        assert!(try_cast_i16(u8::min_value()).is_some());
        assert!(try_cast_i32(u8::min_value()).is_some());
        assert!(try_cast_i64(u8::min_value()).is_some());
        assert!(try_cast_i128(u8::min_value()).is_some());
        assert!(try_cast_u8(u8::max_value()).is_some());
        assert!(try_cast_u16(u8::max_value()).is_some());
        assert!(try_cast_u32(u8::max_value()).is_some());
        assert!(try_cast_u64(u8::max_value()).is_some());
        assert!(try_cast_u128(u8::max_value()).is_some());
        assert!(try_cast_i8(u8::max_value()).is_none());
        assert!(try_cast_i16(u8::max_value()).is_some());
        assert!(try_cast_i32(u8::max_value()).is_some());
        assert!(try_cast_i64(u8::max_value()).is_some());
        assert!(try_cast_i128(u8::max_value()).is_some());
        assert!(try_cast_usize(u8::min_value()).is_some());
        assert!(try_cast_isize(u8::min_value()).is_some());
        assert!(try_cast_usize(u8::max_value()).is_some());
        assert!(try_cast_isize(u8::max_value()).is_some());

        // u16
        assert!(try_cast_u8(u16::min_value()).is_some());
        assert!(try_cast_u16(u16::min_value()).is_some());
        assert!(try_cast_u32(u16::min_value()).is_some());
        assert!(try_cast_u64(u16::min_value()).is_some());
        assert!(try_cast_u128(u16::min_value()).is_some());
        assert!(try_cast_i8(u16::min_value()).is_some());
        assert!(try_cast_i16(u16::min_value()).is_some());
        assert!(try_cast_i32(u16::min_value()).is_some());
        assert!(try_cast_i64(u16::min_value()).is_some());
        assert!(try_cast_i128(u16::min_value()).is_some());
        assert!(try_cast_u8(u16::max_value()).is_none());
        assert!(try_cast_u16(u16::max_value()).is_some());
        assert!(try_cast_u32(u16::max_value()).is_some());
        assert!(try_cast_u64(u16::max_value()).is_some());
        assert!(try_cast_u128(u16::max_value()).is_some());
        assert!(try_cast_i8(u16::max_value()).is_none());
        assert!(try_cast_i16(u16::max_value()).is_none());
        assert!(try_cast_i32(u16::max_value()).is_some());
        assert!(try_cast_i64(u16::max_value()).is_some());
        assert!(try_cast_i128(u16::max_value()).is_some());
        assert!(try_cast_usize(u16::min_value()).is_some());
        assert!(try_cast_isize(u16::min_value()).is_some());
        assert!(try_cast_usize(u16::max_value()).is_some());
        // Discard the results, since platform dependent. Just check it compiles.
        try_cast_isize(u16::max_value());

        // u32
        assert!(try_cast_u8(u32::min_value()).is_some());
        assert!(try_cast_u16(u32::min_value()).is_some());
        assert!(try_cast_u32(u32::min_value()).is_some());
        assert!(try_cast_u64(u32::min_value()).is_some());
        assert!(try_cast_u128(u32::min_value()).is_some());
        assert!(try_cast_i8(u32::min_value()).is_some());
        assert!(try_cast_i16(u32::min_value()).is_some());
        assert!(try_cast_i32(u32::min_value()).is_some());
        assert!(try_cast_i64(u32::min_value()).is_some());
        assert!(try_cast_i128(u32::min_value()).is_some());
        assert!(try_cast_u8(u32::max_value()).is_none());
        assert!(try_cast_u16(u32::max_value()).is_none());
        assert!(try_cast_u32(u32::max_value()).is_some());
        assert!(try_cast_u64(u32::max_value()).is_some());
        assert!(try_cast_u128(u32::max_value()).is_some());
        assert!(try_cast_i8(u32::max_value()).is_none());
        assert!(try_cast_i16(u32::max_value()).is_none());
        assert!(try_cast_i32(u32::max_value()).is_none());
        assert!(try_cast_i64(u32::max_value()).is_some());
        assert!(try_cast_i128(u32::max_value()).is_some());
        assert!(try_cast_usize(u32::min_value()).is_some());
        assert!(try_cast_isize(u32::min_value()).is_some());
        // Discard the results, since platform dependent. Just check it compiles.
        try_cast_usize(u32::max_value());
        try_cast_isize(u32::max_value());

        // u64
        assert!(try_cast_u8(u64::min_value()).is_some());
        assert!(try_cast_u16(u64::min_value()).is_some());
        assert!(try_cast_u32(u64::min_value()).is_some());
        assert!(try_cast_u64(u64::min_value()).is_some());
        assert!(try_cast_u128(u64::min_value()).is_some());
        assert!(try_cast_i8(u64::min_value()).is_some());
        assert!(try_cast_i16(u64::min_value()).is_some());
        assert!(try_cast_i32(u64::min_value()).is_some());
        assert!(try_cast_i64(u64::min_value()).is_some());
        assert!(try_cast_i128(u64::min_value()).is_some());
        assert!(try_cast_u8(u64::max_value()).is_none());
        assert!(try_cast_u16(u64::max_value()).is_none());
        assert!(try_cast_u32(u64::max_value()).is_none());
        assert!(try_cast_u64(u64::max_value()).is_some());
        assert!(try_cast_u128(u64::max_value()).is_some());
        assert!(try_cast_i8(u64::max_value()).is_none());
        assert!(try_cast_i16(u64::max_value()).is_none());
        assert!(try_cast_i32(u64::max_value()).is_none());
        assert!(try_cast_i64(u64::max_value()).is_none());
        assert!(try_cast_i128(u64::max_value()).is_some());
        assert!(try_cast_usize(u64::min_value()).is_some());
        assert!(try_cast_isize(u64::min_value()).is_some());
        // Discard the results, since platform dependent. Just check it compiles.
        try_cast_usize(u64::max_value());
        try_cast_isize(u64::max_value());

        // u128
        assert!(try_cast_u8(u128::min_value()).is_some());
        assert!(try_cast_u16(u128::min_value()).is_some());
        assert!(try_cast_u32(u128::min_value()).is_some());
        assert!(try_cast_u64(u128::min_value()).is_some());
        assert!(try_cast_u128(u128::min_value()).is_some());
        assert!(try_cast_i8(u128::min_value()).is_some());
        assert!(try_cast_i16(u128::min_value()).is_some());
        assert!(try_cast_i32(u128::min_value()).is_some());
        assert!(try_cast_i64(u128::min_value()).is_some());
        assert!(try_cast_i128(u128::min_value()).is_some());
        assert!(try_cast_u8(u128::max_value()).is_none());
        assert!(try_cast_u16(u128::max_value()).is_none());
        assert!(try_cast_u32(u128::max_value()).is_none());
        assert!(try_cast_u64(u128::max_value()).is_none());
        assert!(try_cast_u128(u128::max_value()).is_some());
        assert!(try_cast_i8(u128::max_value()).is_none());
        assert!(try_cast_i16(u128::max_value()).is_none());
        assert!(try_cast_i32(u128::max_value()).is_none());
        assert!(try_cast_i64(u128::max_value()).is_none());
        assert!(try_cast_i128(u128::max_value()).is_none());
        assert!(try_cast_usize(u128::min_value()).is_some());
        assert!(try_cast_isize(u128::min_value()).is_some());
        assert!(try_cast_usize(u128::max_value()).is_none());
        assert!(try_cast_isize(u128::max_value()).is_none());

        // i8
        assert!(try_cast_u8(i8::min_value()).is_none());
        assert!(try_cast_u16(i8::min_value()).is_none());
        assert!(try_cast_u32(i8::min_value()).is_none());
        assert!(try_cast_u64(i8::min_value()).is_none());
        assert!(try_cast_u128(i8::min_value()).is_none());
        assert!(try_cast_i8(i8::min_value()).is_some());
        assert!(try_cast_i16(i8::min_value()).is_some());
        assert!(try_cast_i32(i8::min_value()).is_some());
        assert!(try_cast_i64(i8::min_value()).is_some());
        assert!(try_cast_i128(i8::min_value()).is_some());
        assert!(try_cast_u8(i8::zero()).is_some());
        assert!(try_cast_u16(i8::zero()).is_some());
        assert!(try_cast_u32(i8::zero()).is_some());
        assert!(try_cast_u64(i8::zero()).is_some());
        assert!(try_cast_u128(i8::zero()).is_some());
        assert!(try_cast_i8(i8::zero()).is_some());
        assert!(try_cast_i16(i8::zero()).is_some());
        assert!(try_cast_i32(i8::zero()).is_some());
        assert!(try_cast_i64(i8::zero()).is_some());
        assert!(try_cast_i128(i8::zero()).is_some());
        assert!(try_cast_u8(i8::max_value()).is_some());
        assert!(try_cast_u16(i8::max_value()).is_some());
        assert!(try_cast_u32(i8::max_value()).is_some());
        assert!(try_cast_u64(i8::max_value()).is_some());
        assert!(try_cast_u128(i8::max_value()).is_some());
        assert!(try_cast_i8(i8::max_value()).is_some());
        assert!(try_cast_i16(i8::max_value()).is_some());
        assert!(try_cast_i32(i8::max_value()).is_some());
        assert!(try_cast_i64(i8::max_value()).is_some());
        assert!(try_cast_i128(i8::max_value()).is_some());
        assert!(try_cast_usize(i8::min_value()).is_none());
        assert!(try_cast_isize(i8::min_value()).is_some());
        assert!(try_cast_usize(i8::zero()).is_some());
        assert!(try_cast_isize(i8::zero()).is_some());
        assert!(try_cast_usize(i8::max_value()).is_some());
        assert!(try_cast_isize(i8::max_value()).is_some());

        // i16
        assert!(try_cast_u8(i16::min_value()).is_none());
        assert!(try_cast_u16(i16::min_value()).is_none());
        assert!(try_cast_u32(i16::min_value()).is_none());
        assert!(try_cast_u64(i16::min_value()).is_none());
        assert!(try_cast_u128(i16::min_value()).is_none());
        assert!(try_cast_i8(i16::min_value()).is_none());
        assert!(try_cast_i16(i16::min_value()).is_some());
        assert!(try_cast_i32(i16::min_value()).is_some());
        assert!(try_cast_i64(i16::min_value()).is_some());
        assert!(try_cast_i128(i16::min_value()).is_some());
        assert!(try_cast_u8(i16::zero()).is_some());
        assert!(try_cast_u16(i16::zero()).is_some());
        assert!(try_cast_u32(i16::zero()).is_some());
        assert!(try_cast_u64(i16::zero()).is_some());
        assert!(try_cast_u128(i16::zero()).is_some());
        assert!(try_cast_i8(i16::zero()).is_some());
        assert!(try_cast_i16(i16::zero()).is_some());
        assert!(try_cast_i32(i16::zero()).is_some());
        assert!(try_cast_i64(i16::zero()).is_some());
        assert!(try_cast_i128(i16::zero()).is_some());
        assert!(try_cast_u8(i16::max_value()).is_none());
        assert!(try_cast_u16(i16::max_value()).is_some());
        assert!(try_cast_u32(i16::max_value()).is_some());
        assert!(try_cast_u64(i16::max_value()).is_some());
        assert!(try_cast_u128(i16::max_value()).is_some());
        assert!(try_cast_i8(i16::max_value()).is_none());
        assert!(try_cast_i16(i16::max_value()).is_some());
        assert!(try_cast_i32(i16::max_value()).is_some());
        assert!(try_cast_i64(i16::max_value()).is_some());
        assert!(try_cast_i128(i16::max_value()).is_some());
        assert!(try_cast_usize(i16::min_value()).is_none());
        assert!(try_cast_isize(i16::min_value()).is_some());
        assert!(try_cast_usize(i16::zero()).is_some());
        assert!(try_cast_isize(i16::zero()).is_some());
        assert!(try_cast_usize(i16::max_value()).is_some());
        assert!(try_cast_isize(i16::max_value()).is_some());

        // i32
        assert!(try_cast_u8(i32::min_value()).is_none());
        assert!(try_cast_u16(i32::min_value()).is_none());
        assert!(try_cast_u32(i32::min_value()).is_none());
        assert!(try_cast_u64(i32::min_value()).is_none());
        assert!(try_cast_u128(i32::min_value()).is_none());
        assert!(try_cast_i8(i32::min_value()).is_none());
        assert!(try_cast_i16(i32::min_value()).is_none());
        assert!(try_cast_i32(i32::min_value()).is_some());
        assert!(try_cast_i64(i32::min_value()).is_some());
        assert!(try_cast_i128(i32::min_value()).is_some());
        assert!(try_cast_u8(i32::zero()).is_some());
        assert!(try_cast_u16(i32::zero()).is_some());
        assert!(try_cast_u32(i32::zero()).is_some());
        assert!(try_cast_u64(i32::zero()).is_some());
        assert!(try_cast_u128(i32::zero()).is_some());
        assert!(try_cast_i8(i32::zero()).is_some());
        assert!(try_cast_i16(i32::zero()).is_some());
        assert!(try_cast_i32(i32::zero()).is_some());
        assert!(try_cast_i64(i32::zero()).is_some());
        assert!(try_cast_i128(i32::zero()).is_some());
        assert!(try_cast_u8(i32::max_value()).is_none());
        assert!(try_cast_u16(i32::max_value()).is_none());
        assert!(try_cast_u32(i32::max_value()).is_some());
        assert!(try_cast_u64(i32::max_value()).is_some());
        assert!(try_cast_u128(i32::max_value()).is_some());
        assert!(try_cast_i8(i32::max_value()).is_none());
        assert!(try_cast_i16(i32::max_value()).is_none());
        assert!(try_cast_i32(i32::max_value()).is_some());
        assert!(try_cast_i64(i32::max_value()).is_some());
        assert!(try_cast_i128(i32::max_value()).is_some());
        assert!(try_cast_usize(i32::zero()).is_some());
        assert!(try_cast_isize(i32::zero()).is_some());
        // Discard the results, since platform dependent. Just check it compiles.
        try_cast_usize(i32::min_value());
        try_cast_isize(i32::min_value());
        try_cast_usize(i32::max_value());
        try_cast_isize(i32::max_value());

        // i64
        assert!(try_cast_u8(i64::min_value()).is_none());
        assert!(try_cast_u16(i64::min_value()).is_none());
        assert!(try_cast_u32(i64::min_value()).is_none());
        assert!(try_cast_u64(i64::min_value()).is_none());
        assert!(try_cast_u128(i64::min_value()).is_none());
        assert!(try_cast_i8(i64::min_value()).is_none());
        assert!(try_cast_i16(i64::min_value()).is_none());
        assert!(try_cast_i32(i64::min_value()).is_none());
        assert!(try_cast_i64(i64::min_value()).is_some());
        assert!(try_cast_i128(i64::min_value()).is_some());
        assert!(try_cast_u8(i64::zero()).is_some());
        assert!(try_cast_u16(i64::zero()).is_some());
        assert!(try_cast_u32(i64::zero()).is_some());
        assert!(try_cast_u64(i64::zero()).is_some());
        assert!(try_cast_u128(i64::zero()).is_some());
        assert!(try_cast_i8(i64::zero()).is_some());
        assert!(try_cast_i16(i64::zero()).is_some());
        assert!(try_cast_i32(i64::zero()).is_some());
        assert!(try_cast_i64(i64::zero()).is_some());
        assert!(try_cast_i128(i64::zero()).is_some());
        assert!(try_cast_u8(i64::max_value()).is_none());
        assert!(try_cast_u16(i64::max_value()).is_none());
        assert!(try_cast_u32(i64::max_value()).is_none());
        assert!(try_cast_u64(i64::max_value()).is_some());
        assert!(try_cast_u128(i64::max_value()).is_some());
        assert!(try_cast_i8(i64::max_value()).is_none());
        assert!(try_cast_i16(i64::max_value()).is_none());
        assert!(try_cast_i32(i64::max_value()).is_none());
        assert!(try_cast_i64(i64::max_value()).is_some());
        assert!(try_cast_i128(i64::max_value()).is_some());
        assert!(try_cast_usize(i64::zero()).is_some());
        assert!(try_cast_isize(i64::zero()).is_some());
        // Discard the results, since platform dependent. Just check it compiles.
        try_cast_usize(i64::min_value());
        try_cast_isize(i64::min_value());
        try_cast_usize(i64::max_value());
        try_cast_isize(i64::max_value());

        // i128
        assert!(try_cast_u8(i128::min_value()).is_none());
        assert!(try_cast_u16(i128::min_value()).is_none());
        assert!(try_cast_u32(i128::min_value()).is_none());
        assert!(try_cast_u64(i128::min_value()).is_none());
        assert!(try_cast_u128(i128::min_value()).is_none());
        assert!(try_cast_i8(i128::min_value()).is_none());
        assert!(try_cast_i16(i128::min_value()).is_none());
        assert!(try_cast_i32(i128::min_value()).is_none());
        assert!(try_cast_i64(i128::min_value()).is_none());
        assert!(try_cast_i128(i128::min_value()).is_some());
        assert!(try_cast_u8(i128::zero()).is_some());
        assert!(try_cast_u16(i128::zero()).is_some());
        assert!(try_cast_u32(i128::zero()).is_some());
        assert!(try_cast_u64(i128::zero()).is_some());
        assert!(try_cast_u128(i128::zero()).is_some());
        assert!(try_cast_i8(i128::zero()).is_some());
        assert!(try_cast_i16(i128::zero()).is_some());
        assert!(try_cast_i32(i128::zero()).is_some());
        assert!(try_cast_i64(i128::zero()).is_some());
        assert!(try_cast_i128(i128::zero()).is_some());
        assert!(try_cast_u8(i128::max_value()).is_none());
        assert!(try_cast_u16(i128::max_value()).is_none());
        assert!(try_cast_u32(i128::max_value()).is_none());
        assert!(try_cast_u64(i128::max_value()).is_none());
        assert!(try_cast_u128(i128::max_value()).is_some());
        assert!(try_cast_i8(i128::max_value()).is_none());
        assert!(try_cast_i16(i128::max_value()).is_none());
        assert!(try_cast_i32(i128::max_value()).is_none());
        assert!(try_cast_i64(i128::max_value()).is_none());
        assert!(try_cast_i128(i128::max_value()).is_some());
        assert!(try_cast_usize(i128::min_value()).is_none());
        assert!(try_cast_isize(i128::min_value()).is_none());
        assert!(try_cast_usize(i128::zero()).is_some());
        assert!(try_cast_isize(i128::zero()).is_some());
        assert!(try_cast_usize(i128::max_value()).is_none());
        assert!(try_cast_isize(i128::max_value()).is_none());

        // usize
        assert!(try_cast_u8(usize::min_value()).is_some());
        assert!(try_cast_u16(usize::min_value()).is_some());
        assert!(try_cast_u32(usize::min_value()).is_some());
        assert!(try_cast_u64(usize::min_value()).is_some());
        assert!(try_cast_u128(usize::min_value()).is_some());
        assert!(try_cast_i8(usize::min_value()).is_some());
        assert!(try_cast_i16(usize::min_value()).is_some());
        assert!(try_cast_i32(usize::min_value()).is_some());
        assert!(try_cast_i64(usize::min_value()).is_some());
        assert!(try_cast_i128(usize::min_value()).is_some());
        assert!(try_cast_u8(usize::max_value()).is_none());
        assert!(try_cast_u64(usize::max_value()).is_some());
        assert!(try_cast_u128(usize::max_value()).is_some());
        assert!(try_cast_i8(usize::max_value()).is_none());
        assert!(try_cast_i16(usize::max_value()).is_none());
        assert!(try_cast_i128(usize::max_value()).is_some());
        assert!(try_cast_usize(usize::min_value()).is_some());
        assert!(try_cast_isize(usize::min_value()).is_some());
        assert!(try_cast_usize(usize::max_value()).is_some());
        assert!(try_cast_isize(usize::max_value()).is_none());
        // Discard the results, since platform dependent. Just check it compiles.
        try_cast_u16(usize::max_value());
        try_cast_u32(usize::max_value());
        try_cast_i32(usize::max_value());
        try_cast_i64(usize::max_value());

        // isize
        assert!(try_cast_u8(isize::min_value()).is_none());
        assert!(try_cast_u16(isize::min_value()).is_none());
        assert!(try_cast_u32(isize::min_value()).is_none());
        assert!(try_cast_u64(isize::min_value()).is_none());
        assert!(try_cast_u128(isize::min_value()).is_none());
        assert!(try_cast_i8(isize::min_value()).is_none());
        assert!(try_cast_i64(isize::min_value()).is_some());
        assert!(try_cast_i128(isize::min_value()).is_some());
        assert!(try_cast_u8(isize::zero()).is_some());
        assert!(try_cast_u16(isize::zero()).is_some());
        assert!(try_cast_u32(isize::zero()).is_some());
        assert!(try_cast_u64(isize::zero()).is_some());
        assert!(try_cast_u128(isize::zero()).is_some());
        assert!(try_cast_i8(isize::zero()).is_some());
        assert!(try_cast_i16(isize::zero()).is_some());
        assert!(try_cast_i32(isize::zero()).is_some());
        assert!(try_cast_i64(isize::zero()).is_some());
        assert!(try_cast_i128(isize::zero()).is_some());
        assert!(try_cast_u8(isize::max_value()).is_none());
        assert!(try_cast_u64(isize::max_value()).is_some());
        assert!(try_cast_u128(isize::max_value()).is_some());
        assert!(try_cast_i8(isize::max_value()).is_none());
        assert!(try_cast_i64(isize::max_value()).is_some());
        assert!(try_cast_i128(isize::max_value()).is_some());
        assert!(try_cast_usize(isize::min_value()).is_none());
        assert!(try_cast_isize(isize::min_value()).is_some());
        assert!(try_cast_usize(isize::zero()).is_some());
        assert!(try_cast_isize(isize::zero()).is_some());
        assert!(try_cast_usize(isize::max_value()).is_some());
        assert!(try_cast_isize(isize::max_value()).is_some());

        // Discard the results, since platform dependent. Just check it compiles.
        try_cast_i16(isize::min_value());
        try_cast_i32(isize::min_value());
        try_cast_u16(isize::max_value());
        try_cast_u32(isize::max_value());
        try_cast_i16(isize::max_value());
        try_cast_i32(isize::max_value());
    }

    #[allow(dead_code)]     // Compile-only
    fn try_float_cast_test() {
        // from f32
        try_cast_u8(f32::MIN);
        try_cast_u16(f32::MIN);
        try_cast_u32(f32::MIN);
        try_cast_u64(f32::MIN);
        try_cast_u128(f32::MIN);
        try_cast_usize(f32::MIN);
        try_cast_i8(f32::MIN);
        try_cast_i16(f32::MIN);
        try_cast_i32(f32::MIN);
        try_cast_i64(f32::MIN);
        try_cast_i128(f32::MIN);
        try_cast_isize(f32::MIN);

        // from f64
        try_cast_u8(f64::MIN);
        try_cast_u16(f64::MIN);
        try_cast_u32(f64::MIN);
        try_cast_u64(f64::MIN);
        try_cast_u128(f64::MIN);
        try_cast_usize(f64::MIN);
        try_cast_i8(f64::MIN);
        try_cast_i16(f64::MIN);
        try_cast_i32(f64::MIN);
        try_cast_i64(f64::MIN);
        try_cast_i128(f64::MIN);
        try_cast_isize(f64::MIN);

        // into f32
        try_cast_f32(u8::min_value());
        try_cast_f32(u16::min_value());
        try_cast_f32(u32::min_value());
        try_cast_f32(u64::min_value());
        try_cast_f32(u128::min_value());
        try_cast_f32(usize::min_value());
        try_cast_f32(i8::min_value());
        try_cast_f32(i16::min_value());
        try_cast_f32(i32::min_value());
        try_cast_f32(i64::min_value());
        try_cast_f32(i128::min_value());
        try_cast_f32(isize::min_value());
        try_cast_f32(f32::MIN);
        try_cast_f32(f64::MIN);

        // into f64
        try_cast_f64(u8::min_value());
        try_cast_f64(u16::min_value());
        try_cast_f64(u32::min_value());
        try_cast_f64(u64::min_value());
        try_cast_f64(u128::min_value());
        try_cast_f64(usize::min_value());
        try_cast_f64(i8::min_value());
        try_cast_f64(i16::min_value());
        try_cast_f64(i32::min_value());
        try_cast_f64(i64::min_value());
        try_cast_f64(i128::min_value());
        try_cast_f64(isize::min_value());
        try_cast_f64(f32::MIN);
        try_cast_f64(f64::MIN);
    }
}
